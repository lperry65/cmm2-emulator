//! The CPU: a thin wrapper around the zmu core, so nothing else depends on it. It runs the
//! firmware against the machine's `MemoryMap` and lets peripherals raise interrupts.

use std::cell::{Cell, RefCell, RefMut};
use std::fmt;
use std::rc::Rc;

use zmu_cortex_m::Processor;
use zmu_cortex_m::bus::Bus;
use zmu_cortex_m::core::fault;
use zmu_cortex_m::core::fault::FaultTrapMode;
use zmu_cortex_m::core::register::{BaseReg, Reg};
use zmu_cortex_m::core::reset::Reset;
use zmu_cortex_m::core::thumb::ThumbCode;
use zmu_cortex_m::decoder::Decoder;
use zmu_cortex_m::executor::Executor;
use zmu_cortex_m::peripheral::nvic::NVIC;

use crate::clock::Clock;
use crate::ds3231::{OffsetError, STATE_SIZE};
use crate::firmware::{FLASH_BASE, FLASH_SIZE, Firmware, USER_FLASH_BASE, USER_FLASH_SIZE};
use crate::flash_ops::{FlashOp, FlashOps};
use crate::irq::Irqs;
use crate::memory::MemoryMap;
use crate::peripheral::ResetCause;

/// Cortex-M7 r1p1.
const CPUID: u32 = 0x411F_C271;

#[derive(Debug)]
pub enum CpuError {
    /// The core could not start from the vector table; carries its message.
    Reset(String),
}

impl fmt::Display for CpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CpuError::Reset(message) => write!(f, "CPU reset failed: {message}"),
        }
    }
}

impl std::error::Error for CpuError {}

/// What a real board keeps with the power off: the user flash, then the clock's time and date
/// (TR and DR, a word each), which its battery keeps going. This is the size of the saved
/// state of a board whose DS3231 is as it was made.
pub const PERSISTENT_SIZE: usize = USER_FLASH_SIZE + 8;

/// The state with the DS3231's after it (`Ds3231Slot::saved`), once the guest has set its time or
/// cleared its status.
const WITH_DS3231_SIZE: usize = PERSISTENT_SIZE + STATE_SIZE;

/// The size of the state that the first version with a DS3231 saved: the DS3231's offset from the
/// host's local time, in seconds, an i64.
const VERSION_1_SIZE: usize = PERSISTENT_SIZE + 8;

/// Saved state that cannot be loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistentError {
    /// The wrong length.
    Size(PersistentSizeError),
    /// The DS3231's saved clock is too far from the host's.
    Offset(OffsetError),
}

impl fmt::Display for PersistentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PersistentError::Size(size) => size.fmt(f),
            PersistentError::Offset(offset) => offset.fmt(f),
        }
    }
}

impl std::error::Error for PersistentError {}

impl From<OffsetError> for PersistentError {
    fn from(offset: OffsetError) -> PersistentError {
        PersistentError::Offset(offset)
    }
}

/// Saved state of the wrong length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistentSizeError {
    pub got: usize,
}

impl fmt::Display for PersistentSizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the saved flash and clock are {PERSISTENT_SIZE} bytes, or {WITH_DS3231_SIZE} with the \
             DS3231's state ({VERSION_1_SIZE} from the first version that had it), not {}",
            self.got
        )
    }
}

impl std::error::Error for PersistentSizeError {}

/// What stopped the CPU: the address of the instruction and the core's description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    pub pc: u32,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    InstructionLimit,
    /// The next instruction is at this address and has not run.
    Address(u32),
    Fault(Fault),
}

/// The core registers in the private peripheral bus (0xE000_0000 up to 0xE010_0000) that zmu
/// implements itself: the NVIC, SysTick, most of the SCB and a few debug registers. The core
/// asks the external bus first, so these must be turned down here to reach zmu's own code.
/// The rest of that range (the MPU, cache maintenance, ID registers and so on) goes to the map.
fn core_register(addr: u32) -> bool {
    matches!(addr,
        0xE000_0000..=0xE000_007C
        | 0xE000_1000 | 0xE000_1004 | 0xE000_1FB0
        | 0xE000_E004 | 0xE000_E008
        | 0xE000_E010..=0xE000_E01C
        | 0xE000_E100..=0xE000_E13C
        | 0xE000_E180..=0xE000_E1BC
        | 0xE000_E200..=0xE000_E23C
        | 0xE000_E280..=0xE000_E2BC
        | 0xE000_E300..=0xE000_E33C
        | 0xE000_E400..=0xE000_E5EC
        | 0xE000_ED00..=0xE000_ED3C
        | 0xE000_ED88
        | 0xE000_EDFC
        | 0xE000_EF00
        | 0xE000_EF34..=0xE000_EF48)
}

/// The memory map as the core's external bus. The core keeps the flash and the DTCM itself
/// and sends everything else here. Unmapped accesses are logged by the map, never faulted.
struct Bridge(Rc<RefCell<MemoryMap>>);

impl Bus for Bridge {
    fn read32(&mut self, addr: u32) -> Result<u32, fault::Fault> {
        Ok(self.0.borrow_mut().read32(addr))
    }

    fn read16(&self, addr: u32) -> Result<u16, fault::Fault> {
        Ok(self.0.borrow_mut().read16(addr))
    }

    fn read8(&self, addr: u32) -> Result<u8, fault::Fault> {
        Ok(self.0.borrow_mut().read8(addr))
    }

    fn write32(&mut self, addr: u32, value: u32) -> Result<(), fault::Fault> {
        self.0.borrow_mut().write32(addr, value);
        Ok(())
    }

    fn write16(&mut self, addr: u32, value: u16) -> Result<(), fault::Fault> {
        self.0.borrow_mut().write16(addr, value);
        Ok(())
    }

    fn write8(&mut self, addr: u32, value: u8) -> Result<(), fault::Fault> {
        self.0.borrow_mut().write8(addr, value);
        Ok(())
    }

    fn in_range(&self, addr: u32) -> bool {
        !core_register(addr)
    }
}

/// Starts the core from the vector table at the start of flash.
fn boot(processor: &mut Processor) -> Result<(), CpuError> {
    processor.vtor = FLASH_BASE;
    processor
        .reset()
        .map_err(|fault| CpuError::Reset(fault.to_string()))?;
    processor.running = true;
    Ok(())
}

pub struct Cpu {
    processor: Processor,
    memory: Rc<RefCell<MemoryMap>>,
    irqs: Irqs,
    flash_ops: FlashOps,
    pc: Rc<Cell<u32>>,
    clock: Clock,
    resets: u32,
}

impl Cpu {
    /// Puts the firmware in flash and resets the core from its vector table.
    ///
    /// Faults stop the run and are not delivered to the guest. That is for bring-up, to see
    /// the first thing that goes wrong; the guest's own fault handlers do not run.
    pub fn new(firmware: &Firmware, memory: MemoryMap, irqs: Irqs) -> Result<Cpu, CpuError> {
        let pc = memory.pc_cell();
        let flash_ops = memory.flash_ops();
        let clock = memory.clock();
        let memory = Rc::new(RefCell::new(memory));

        let mut flash = vec![0xFF; FLASH_SIZE];
        flash[..firmware.image().len()].copy_from_slice(firmware.image());

        let mut processor = Processor::new();
        processor.flash_memory_at(FLASH_BASE, FLASH_SIZE, &flash);
        // Decoding the last halfword of flash reads past its end, so the cache is built
        // before the bus is attached, or that read would show up in the map's log.
        processor.cache_instructions();
        processor.device(Some(Box::new(Bridge(memory.clone()))));
        processor.fault_trap_mode(FaultTrapMode::all());
        processor.cpuid = CPUID;
        boot(&mut processor)?;

        Ok(Cpu {
            processor,
            memory,
            irqs,
            flash_ops,
            pc,
            clock,
            resets: 0,
        })
    }

    /// Executes one instruction, then hands any raised interrupts to the interrupt
    /// controller, which takes them when they are enabled. Returns the fault if the
    /// instruction caused one.
    pub fn step(&mut self) -> Option<Fault> {
        self.pc.set(self.processor.get_pc());
        // Before the instruction, so a register it reads sees the time at its start.
        self.clock.set(self.processor.cycle_count);
        if self.clock.take_due() {
            self.memory.borrow_mut().tick();
        }
        self.processor.step();
        if self.processor.system_reset_requested {
            self.reset();
        }
        if let Some(raised) = self.irqs.take() {
            for (index, bits) in raised.into_iter().enumerate() {
                self.processor.nvic_write_ispr(index, bits);
            }
        }
        if let Some(ops) = self.flash_ops.take() {
            self.apply(ops);
        }
        self.processor.take_pending_fault_trap().map(|trap| Fault {
            pc: trap.pc,
            description: trap.to_string(),
        })
    }

    /// Carries out the flash changes the controller asked for. They go through the core's own
    /// writes, which also refresh its decoded instructions.
    fn apply(&mut self, ops: Vec<FlashOp>) {
        for op in ops {
            match op {
                FlashOp::Erase { base, len } => {
                    for addr in (base..base + len).step_by(4) {
                        // The core only refuses addresses outside its flash.
                        let _ = self.processor.write32(addr, 0xFFFF_FFFF);
                    }
                }
            }
        }
    }

    /// Steps until `max_instructions` more have run, the next instruction is at `stop_at` (the
    /// Thumb bit is ignored) or a fault stops it.
    pub fn run_until(&mut self, max_instructions: u64, stop_at: Option<u32>) -> Stop {
        self.run_loop(max_instructions, stop_at, |_, _| {})
    }

    pub fn run(&mut self, max_instructions: u64) -> Stop {
        self.run_until(max_instructions, None)
    }

    /// Like `run_until`, and calls `trace` with the address and disassembly of each instruction
    /// just before it runs. Only this method pays for the disassembly.
    pub fn run_traced(
        &mut self,
        max_instructions: u64,
        stop_at: Option<u32>,
        mut trace: impl FnMut(u32, &str),
    ) -> Stop {
        self.run_loop(max_instructions, stop_at, |cpu, pc| {
            trace(pc, &cpu.describe_next());
        })
    }

    fn run_loop(
        &mut self,
        max_instructions: u64,
        stop_at: Option<u32>,
        mut before: impl FnMut(&mut Cpu, u32),
    ) -> Stop {
        let limit = self.instruction_count().saturating_add(max_instructions);
        while self.instruction_count() < limit {
            let pc = self.pc();
            if stop_at.is_some_and(|addr| addr & !1 == pc & !1) {
                return Stop::Address(pc);
            }
            before(self, pc);
            if let Some(fault) = self.step() {
                return Stop::Fault(fault);
            }
        }
        Stop::InstructionLimit
    }

    /// The disassembly of the instruction at the pc. Fetching goes through the memory map like
    /// the core's own fetch does, so a pc outside flash is logged there as an access.
    pub fn describe_next(&mut self) -> String {
        let pc = self.pc();
        match self.processor.fetch_code(pc) {
            Ok(ThumbCode::Undefined) => "undefined".to_string(),
            Ok(code) => self.processor.decode(code).to_string(),
            Err(_) => "fetch fault".to_string(),
        }
    }

    /// The address of the next instruction.
    pub fn pc(&mut self) -> u32 {
        self.processor.get_pc()
    }

    /// The stack pointer in use: the main stack pointer, or the process stack pointer when
    /// the guest has switched to it.
    pub fn sp(&self) -> u32 {
        self.processor.get_r(Reg::SP)
    }

    /// Register `n` from 0 to 15. Register 15 reads as the architecture defines it: 4 ahead
    /// of the instruction.
    pub fn reg(&self, n: usize) -> u32 {
        assert!(n < 16, "there is no register {n}");
        self.processor.get_r(Reg::from(n as u8))
    }

    pub fn instruction_count(&self) -> u64 {
        self.processor.instruction_count
    }

    /// An estimate of the core's clock cycles.
    pub fn cycle_count(&self) -> u64 {
        self.processor.cycle_count
    }

    /// Reads a word as the core would: flash and DTCM are the core's own, the rest comes from
    /// the machine, where reading a peripheral has that peripheral's read side effects. It is
    /// 0 where the core refuses the address.
    pub fn read32(&mut self, addr: u32) -> u32 {
        self.processor.read32(addr).unwrap_or(0)
    }

    /// How many system resets have happened since the CPU was made.
    pub fn resets(&self) -> u32 {
        self.resets
    }

    /// Resets the machine as a system reset does: the core restarts from the vector table,
    /// the peripherals go back to their reset state and anything they had raised or queued is
    /// dropped. Flash and RAM keep their contents, and time keeps running.
    pub fn reset(&mut self) {
        // The peripherals that restart their clocks read the time as it is now.
        self.clock.set(self.processor.cycle_count);
        self.memory
            .borrow_mut()
            .reset_peripherals(ResetCause::Software);
        self.irqs.clear();
        self.flash_ops.clear();
        boot(&mut self.processor).expect("the vector table is in flash, which always reads");
        self.resets += 1;
    }

    /// What the machine keeps with the power off: the user flash, the saved options and the
    /// stored program, as the guest has left it, then the clock's time and date. The firmware
    /// is not part of it.
    pub fn persistent(&mut self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(PERSISTENT_SIZE);
        for addr in (USER_FLASH_BASE..FLASH_BASE + FLASH_SIZE as u32).step_by(4) {
            bytes.extend(self.read32(addr).to_le_bytes());
        }
        // Straight from the clock, not over the bus, so that a trace of the peripherals does
        // not log the reads.
        let memory = self.memory.borrow();
        for word in memory.rtc().calendar() {
            bytes.extend(word.to_le_bytes());
        }
        // The DS3231's, if it is not as it was made, which keeps the file the size it was before
        // for a board whose clock was never set.
        bytes.extend(memory.ds3231().saved().into_iter().flatten());
        bytes
    }

    /// Restores what `persistent` gave out. The firmware is not touched, so state saved from
    /// an older firmware still loads. State saved without the DS3231's leaves it as it was
    /// made, and the state of the first version with a DS3231 is converted. Nothing is loaded
    /// if it is refused.
    pub fn load_persistent(&mut self, bytes: &[u8]) -> Result<(), PersistentError> {
        let (flash, rest) = bytes
            .split_at_checked(USER_FLASH_SIZE)
            .ok_or(PersistentSizeError { got: bytes.len() })
            .map_err(PersistentError::Size)?;
        let ds3231 = self.memory.borrow().ds3231();
        match bytes.len() {
            PERSISTENT_SIZE => ds3231.load_new(),
            VERSION_1_SIZE => ds3231.load_v1(i64::from_le_bytes(
                rest[8..].try_into().expect("eight bytes"),
            ))?,
            WITH_DS3231_SIZE => ds3231.load(rest[8..].try_into().expect("the state's size"))?,
            got => return Err(PersistentError::Size(PersistentSizeError { got })),
        }
        let (words, _) = flash.as_chunks::<4>();
        for (addr, word) in (USER_FLASH_BASE..).step_by(4).zip(words) {
            self.processor
                .write32(addr, u32::from_le_bytes(*word))
                .expect("the user flash is inside the core's flash");
        }
        let (words, _) = rest[..8].as_chunks::<4>();
        let calendar = [words[0], words[1]].map(u32::from_le_bytes);
        self.memory.borrow().rtc().set_calendar(calendar);
        Ok(())
    }

    pub fn memory(&self) -> RefMut<'_, MemoryMap> {
        self.memory.borrow_mut()
    }

    pub fn irqs(&self) -> Irqs {
        self.irqs.clone()
    }
}

#[cfg(test)]
mod tests;
