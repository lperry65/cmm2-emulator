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
use crate::firmware::{FLASH_BASE, FLASH_SIZE, Firmware, USER_FLASH_BASE, USER_FLASH_SIZE};
use crate::flash_ops::{FlashOp, FlashOps};
use crate::irq::Irqs;
use crate::memory::MemoryMap;
use crate::peripheral::ResetCause;
use crate::rtc;

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
/// (TR and DR, a word each), which its battery keeps going.
pub const PERSISTENT_SIZE: usize = USER_FLASH_SIZE + 8;

/// Saved state of the wrong length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistentSizeError {
    pub got: usize,
}

impl fmt::Display for PersistentSizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the saved flash and clock are {PERSISTENT_SIZE} bytes, not {}",
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
        for word in rtc::read_calendar(&mut self.memory.borrow_mut()) {
            bytes.extend(word.to_le_bytes());
        }
        bytes
    }

    /// Restores what `persistent` gave out. The firmware is not touched, so state saved from
    /// an older firmware still loads.
    pub fn load_persistent(&mut self, bytes: &[u8]) -> Result<(), PersistentSizeError> {
        if bytes.len() != PERSISTENT_SIZE {
            return Err(PersistentSizeError { got: bytes.len() });
        }
        let (flash, calendar) = bytes.split_at(USER_FLASH_SIZE);
        let (words, _) = flash.as_chunks::<4>();
        for (addr, word) in (USER_FLASH_BASE..).step_by(4).zip(words) {
            self.processor
                .write32(addr, u32::from_le_bytes(*word))
                .expect("the user flash is inside the core's flash");
        }
        let (words, _) = calendar.as_chunks::<4>();
        let calendar = [words[0], words[1]].map(u32::from_le_bytes);
        rtc::write_calendar(&mut self.memory.borrow_mut(), calendar);
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
mod tests {
    use super::*;
    use crate::{AccessKind, FLASH_BASE, Firmware, PeripheralAccess, Svd, UnmappedAccess};
    use crate::{Console, FLASH_REGS_BASE, FLASH_REGS_SIZE, FlashController, cmm2_memory};
    use crate::{GPIO_PORT_SIZE, Gpio, RCC_BASE, RCC_SIZE, RTC_BASE, RTC_SIZE, Rcc, Rtc};
    use crate::{SdSlot, TIM16_BASE, TIMER_SIZE, Timer};
    use std::cell::RefCell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::rc::Rc;

    const CODE: u32 = FLASH_BASE + 0x100;
    const SP: u32 = 0x2000_8000;

    // Thumb code assembled with binutils, as halfwords.
    const ARITH: [u16; 3] = [0x2005, 0x3002, 0xE7FE]; // movs r0,#5; adds r0,#2; b .
    const SPIN: [u16; 2] = [0x3401, 0xE7FD]; // adds r4,#1; b .-2
    // Stores 0xCAFEF00D to AXI SRAM, SDRAM and DTCM, then loads it back from DTCM into r4.
    const STORES: [u16; 12] = [
        0xF04F, 0x5110, 0xF04F, 0x4250, 0xF04F, 0x5300, 0x4802, 0x6008, 0x6010, 0x6018, 0x681C,
        0xE7FE,
    ];
    const STORES_POOL: [u16; 2] = [0xF00D, 0xCAFE];
    // movs r0,#1; str r0,[0x6000_0000] (the str is at +6)
    const UNMAPPED: [u16; 5] = [0xF04F, 0x41C0, 0x2001, 0x6008, 0xE7FE];
    // Writes 42 to 0x5802_4400 (at +4), then reads it back (at +6).
    const PERIPHERAL: [u16; 8] = [
        0x4902, 0x202A, 0x6008, 0x680A, 0xE7FE, 0x0000, 0x4400, 0x5802,
    ];
    // Enables IRQ 5 in the NVIC, then counts in r4.
    const ENABLE_AND_COUNT: [u16; 8] = [
        0x4902, 0x2020, 0x6008, 0xB662, 0x3401, 0xE7FD, 0xE100, 0xE000,
    ];
    // Adds one to the word at 0x2400_0000 and returns.
    const COUNT_IN_AXI: [u16; 6] = [0xF04F, 0x5110, 0x6808, 0x3001, 0x6008, 0x4770];
    const UNDEFINED: [u16; 1] = [0xDE00]; // udf #0
    // movs r0,#1; movs r1,#2; movs r2,#3; b .
    const THREE: [u16; 4] = [0x2001, 0x2102, 0x2203, 0xE7FE];
    // ldr r0,[pc,#4]; bx r0 into 0xE000_0000, which the core cannot fetch from.
    const JUMP_TO_NOWHERE: [u16; 6] = [0x4801, 0x4700, 0xE7FE, 0x0000, 0x0001, 0xE000];

    /// A vector table (initial SP, reset into `code`, then `handlers` as (irq, offset into
    /// `code`)) followed by the code at `CODE`.
    fn image(code: &[u16], handlers: &[(usize, u32)]) -> Firmware {
        let mut words = vec![0u32; 64];
        words[0] = SP;
        words[1] = CODE | 1;
        for &(irq, offset) in handlers {
            words[16 + irq] = (CODE + offset) | 1;
        }
        let mut bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        bytes.extend(code.iter().flat_map(|h| h.to_le_bytes()));
        Firmware::from_bytes(bytes).unwrap()
    }

    fn cpu(code: &[u16]) -> Cpu {
        Cpu::new(&image(code, &[]), MemoryMap::new(), Irqs::new()).unwrap()
    }

    #[test]
    fn starts_at_the_reset_handler() {
        let mut cpu = cpu(&ARITH);
        assert_eq!(cpu.pc(), CODE);
        assert_eq!(cpu.sp(), SP);
        assert_eq!(cpu.instruction_count(), 0);
    }

    #[test]
    fn steps_execute_instructions() {
        let mut cpu = cpu(&ARITH);
        assert!(cpu.step().is_none());
        assert!(cpu.step().is_none());
        assert_eq!(cpu.reg(0), 7);
        assert_eq!(cpu.instruction_count(), 2);
        assert_eq!(cpu.pc(), CODE + 4);
        assert!(cpu.cycle_count() >= 2);
    }

    #[test]
    fn run_stops_at_the_instruction_limit() {
        let mut cpu = cpu(&ARITH);
        cpu.run(2);
        assert!(matches!(cpu.run(10), Stop::InstructionLimit));
        assert_eq!(cpu.instruction_count(), 12);
    }

    #[test]
    fn loads_and_stores_reach_the_machine() {
        let code: Vec<u16> = [&STORES[..], &STORES_POOL[..]].concat();
        let mut cpu = cpu(&code);
        cpu.run(20);
        assert_eq!(cpu.memory().read32(0x2400_0000), 0xCAFE_F00D);
        assert_eq!(cpu.memory().read32(0xD000_0000), 0xCAFE_F00D);
        assert_eq!(cpu.reg(4), 0xCAFE_F00D);
        assert!(cpu.memory().take_unmapped().is_empty());
    }

    #[test]
    fn unmapped_access_records_the_pc() {
        let mut cpu = cpu(&UNMAPPED);
        cpu.run(10);
        assert_eq!(
            cpu.memory().take_unmapped(),
            [UnmappedAccess {
                addr: 0x6000_0000,
                size: 4,
                kind: AccessKind::Write(1),
                pc: CODE + 6,
            }]
        );
    }

    #[test]
    fn peripheral_trace_records_the_pc() {
        let mut memory = MemoryMap::new();
        memory.map_unmodelled_peripherals().unwrap();
        let log = Rc::new(RefCell::new(Vec::new()));
        let sink = log.clone();
        memory.set_peripheral_trace(Some(Box::new(move |a| sink.borrow_mut().push(*a))));
        let mut cpu = Cpu::new(&image(&PERIPHERAL, &[]), memory, Irqs::new()).unwrap();
        cpu.run(10);
        let access = |write, pc| PeripheralAccess {
            addr: 0x5802_4400,
            size: 4,
            write,
            value: 42,
            pc,
        };
        assert_eq!(
            log.borrow()[..2],
            [access(true, CODE + 4), access(false, CODE + 6)]
        );
        assert!(Svd::stm32h743().lookup(0x5802_4400).is_some());
    }

    fn with_handler(main: &[u16]) -> Cpu {
        let handler = 2 * main.len() as u32;
        let code: Vec<u16> = [main, &COUNT_IN_AXI[..]].concat();
        Cpu::new(
            &image(&code, &[(5, handler)]),
            MemoryMap::new(),
            Irqs::new(),
        )
        .unwrap()
    }

    #[test]
    fn raised_interrupt_runs_its_handler_once() {
        let mut cpu = with_handler(&ENABLE_AND_COUNT);
        cpu.run(20);
        let before = cpu.reg(4);
        cpu.irqs().raise(5);
        cpu.run(50);
        assert_eq!(cpu.memory().read32(0x2400_0000), 1);
        assert!(cpu.reg(4) > before);
        let after = cpu.reg(4);
        cpu.run(20);
        assert!(cpu.reg(4) > after);
    }

    /// Enables IRQ 5, then `cmp r2,r2` (Z set), `ite eq`, `movs r4,#1`, `movs r5,#2`. The
    /// handler sets r6 unconditionally and returns (r3 would be
    /// restored from the frame).
    fn it_block_cpu() -> Cpu {
        let mut code = assemble(
            &[
                Load(0, 0),
                Raw(0x2120), // movs r1,#0x20
                Raw(0x6001), // str r1,[r0]
                Raw(0xB662), // cpsie i
                Raw(0x4292), // cmp r2,r2
                Raw(0xBF0C), // ite eq
                Raw(0x2401), // moveq r4,#1
                Raw(0x2502), // movne r5,#2
                Raw(SPIN_HERE),
            ],
            &[0xE000_E100],
        );
        let handler = 2 * code.len() as u32;
        code.extend([0x2607, 0x4770]); // movs r6,#7; bx lr
        Cpu::new(
            &image(&code, &[(5, handler)]),
            MemoryMap::new(),
            Irqs::new(),
        )
        .unwrap()
    }

    /// Steps `before` instructions, raises the interrupt and runs on to the end. The line is
    /// pended after the next step and taken at the end of the one after it.
    fn run_it_block_with_interrupt_raised_after(before: usize) -> Cpu {
        let mut cpu = it_block_cpu();
        for _ in 0..before {
            cpu.step();
        }
        cpu.irqs().raise(5);
        for _ in 0..14 {
            cpu.step();
        }
        cpu
    }

    #[test]
    fn interrupt_right_after_ite_leaves_the_it_block_intact() {
        // Raised before cmp, so it is taken at the end of ite
        let mut cpu = run_it_block_with_interrupt_raised_after(4);
        assert_eq!(cpu.reg(6), 7);
        assert_eq!(cpu.reg(4), 1);
        assert_eq!(cpu.reg(5), 0);
        assert_eq!(cpu.pc(), CODE + 16);
    }

    #[test]
    fn interrupt_inside_the_it_block_leaves_the_it_block_intact() {
        // Raised before ite, so it is taken at the end of the first instruction of the block
        let mut cpu = run_it_block_with_interrupt_raised_after(5);
        assert_eq!(cpu.reg(6), 7);
        assert_eq!(cpu.reg(4), 1);
        assert_eq!(cpu.reg(5), 0);
        assert_eq!(cpu.pc(), CODE + 16);
    }

    #[test]
    fn interrupt_that_is_not_enabled_stays_pending() {
        let mut cpu = with_handler(&SPIN);
        cpu.irqs().raise(5);
        cpu.run(50);
        assert_eq!(cpu.memory().read32(0x2400_0000), 0);
    }

    #[test]
    fn undefined_instruction_stops_the_run() {
        let mut cpu = cpu(&UNDEFINED);
        let Stop::Fault(fault) = cpu.run(10) else {
            panic!("expected a fault");
        };
        assert_eq!(fault.pc, CODE);
        // No fault handlers are set up, so the usage fault escalates to a hard fault.
        assert!(fault.description.contains("UndefInstr"));
        assert!(fault.description.contains("HardFault"));
    }

    #[test]
    fn run_until_stops_before_the_instruction_at_the_address() {
        let mut cpu = cpu(&THREE);
        assert_eq!(cpu.run_until(100, Some(CODE + 4)), Stop::Address(CODE + 4));
        assert_eq!(cpu.instruction_count(), 2);
        assert_eq!(cpu.reg(0), 1);
        assert_eq!(cpu.reg(1), 2);
        assert_eq!(cpu.reg(2), 0);
    }

    #[test]
    fn run_until_ignores_the_thumb_bit_of_the_address() {
        let mut cpu = cpu(&THREE);
        assert_eq!(
            cpu.run_until(100, Some((CODE + 4) | 1)),
            Stop::Address(CODE + 4)
        );
        assert_eq!(cpu.instruction_count(), 2);
    }

    #[test]
    fn run_until_an_address_never_reached_hits_the_limit() {
        let mut cpu = cpu(&THREE);
        assert_eq!(cpu.run_until(10, Some(CODE + 0x80)), Stop::InstructionLimit);
        assert_eq!(cpu.instruction_count(), 10);
    }

    #[test]
    fn run_until_at_the_address_already_runs_nothing() {
        let mut cpu = cpu(&THREE);
        cpu.run_until(100, Some(CODE + 4));
        assert_eq!(cpu.run_until(100, Some(CODE + 4)), Stop::Address(CODE + 4));
        assert_eq!(cpu.instruction_count(), 2);
    }

    #[test]
    fn run_traced_reports_each_instruction_before_it_runs() {
        let mut cpu = cpu(&THREE);
        let mut seen = Vec::new();
        let stop = cpu.run_traced(4, None, |pc, text| seen.push((pc, text.to_string())));
        assert_eq!(stop, Stop::InstructionLimit);
        let pcs: Vec<u32> = seen.iter().map(|(pc, _)| *pc).collect();
        assert_eq!(pcs, [CODE, CODE + 2, CODE + 4, CODE + 6]);
        assert!(seen.iter().all(|(_, text)| !text.is_empty()));
        assert_eq!(seen[0].1, "mov r0, #1");
        assert_eq!(seen.len() as u64, cpu.instruction_count());
    }

    #[test]
    fn run_traced_stops_at_the_address_without_tracing_it() {
        let mut cpu = cpu(&THREE);
        let mut pcs = Vec::new();
        let stop = cpu.run_traced(100, Some(CODE + 4), |pc, _| pcs.push(pc));
        assert_eq!(stop, Stop::Address(CODE + 4));
        assert_eq!(pcs, [CODE, CODE + 2]);
    }

    #[test]
    fn run_traced_includes_the_interrupt_handler() {
        let mut cpu = with_handler(&ENABLE_AND_COUNT);
        cpu.run(20);
        cpu.irqs().raise(5);
        let handler = CODE + 2 * ENABLE_AND_COUNT.len() as u32;
        let mut pcs = Vec::new();
        cpu.run_traced(50, None, |pc, _| pcs.push(pc));
        assert!(pcs.contains(&handler), "{pcs:x?}");
        assert!(pcs.contains(&(handler + 10)), "{pcs:x?}");
        assert_eq!(pcs.len(), 50);
    }

    #[test]
    // zmu prints no "s" on the flag-setting forms, so this is `mov` for a `movs`.
    fn describe_next_reads_the_instruction_at_the_pc() {
        let mut cpu = cpu(&THREE);
        assert_eq!(cpu.describe_next(), "mov r0, #1");
        cpu.step();
        assert_eq!(cpu.describe_next(), "mov r1, #2");
    }

    #[test]
    fn describe_next_names_an_undefined_instruction() {
        let mut cpu = cpu(&UNDEFINED);
        assert!(cpu.describe_next().contains("udf"));
    }

    #[test]
    fn describe_next_survives_a_fetch_fault() {
        let mut cpu = cpu(&JUMP_TO_NOWHERE);
        cpu.run(2);
        assert_eq!(cpu.pc(), 0xE000_0000);
        assert_eq!(cpu.describe_next(), "fetch fault");
    }

    #[test]
    fn new_keeps_the_callers_unmapped_log() {
        let mut memory = MemoryMap::new();
        memory.read32(0x6000_0000);
        let cpu = Cpu::new(&image(&ARITH, &[]), memory, Irqs::new()).unwrap();
        assert_eq!(
            cpu.memory().take_unmapped(),
            [UnmappedAccess {
                addr: 0x6000_0000,
                size: 4,
                kind: AccessKind::Read,
                pc: 0,
            }]
        );
    }

    #[test]
    fn read32_reads_flash_dtcm_and_the_machine() {
        let code: Vec<u16> = [&STORES[..], &STORES_POOL[..]].concat();
        let mut cpu = cpu(&code);
        cpu.run(20);
        assert_eq!(cpu.read32(FLASH_BASE), SP);
        assert_eq!(cpu.read32(0x2000_0000), 0xCAFE_F00D);
        assert_eq!(cpu.read32(0x2400_0000), 0xCAFE_F00D);
    }

    /// A machine with the clock mapped, as the persistent state includes it.
    fn board(code: &[u16]) -> Cpu {
        Cpu::new(&image(code, &[]), cmm2_memory(&Irqs::new()), Irqs::new()).unwrap()
    }

    fn set_clock(cpu: &mut Cpu, tr: u32, dr: u32) {
        crate::rtc::write_calendar(&mut cpu.memory(), [tr, dr]);
    }

    #[test]
    fn the_persistent_state_is_the_end_of_the_flash_and_the_clock() {
        let mut cpu = board(&THREE);
        let state = cpu.persistent();
        assert_eq!(state.len(), PERSISTENT_SIZE);
        assert_eq!(USER_FLASH_BASE, 0x0812_0000);
        assert_eq!(PERSISTENT_SIZE, 0xE_0008);
        assert!(state[..USER_FLASH_SIZE].iter().all(|&byte| byte == 0xFF));
        // The clock's reset values: 00:00:00 on Monday 01/01/00.
        assert_eq!(state[USER_FLASH_SIZE..], [0, 0, 0, 0, 0x01, 0x21, 0, 0]);
    }

    #[test]
    fn loaded_state_reads_back_and_leaves_the_firmware_alone() {
        let mut cpu = board(&THREE);
        let mut state = vec![0xFF; PERSISTENT_SIZE];
        state[..4].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        let last = USER_FLASH_SIZE - 4;
        state[last..USER_FLASH_SIZE].copy_from_slice(&0xCAFE_F00Du32.to_le_bytes());
        state[USER_FLASH_SIZE..USER_FLASH_SIZE + 4].copy_from_slice(&0x12_00_00u32.to_le_bytes());
        state[USER_FLASH_SIZE + 4..].copy_from_slice(&0x26_91_12u32.to_le_bytes());

        cpu.load_persistent(&state).unwrap();

        assert_eq!(cpu.read32(USER_FLASH_BASE), 0x1234_5678);
        assert_eq!(cpu.read32(0x081E_0000 + 0x1_FFFC), 0xCAFE_F00D);
        assert_eq!(cpu.persistent(), state);
        assert_eq!(cpu.read32(FLASH_BASE), SP);
        assert_eq!(cpu.read32(USER_FLASH_BASE - 4), 0xFFFF_FFFF);
    }

    #[test]
    fn flash_the_guest_wrote_is_saved() {
        let code = [boot_program(1), COUNT_IN_AXI.to_vec()].concat();
        let mut cpu = Cpu::new(&image(&code, &[]), cmm2_memory(&Irqs::new()), Irqs::new()).unwrap();
        cpu.run(100);
        let state = cpu.persistent();
        let at = (FLASH_WORD - USER_FLASH_BASE) as usize;
        assert_eq!(state[at..at + 4], 1u32.to_le_bytes());
    }

    #[test]
    fn the_clock_is_saved_and_carries_on_in_a_new_machine() {
        let mut first = board(&THREE);
        set_clock(&mut first, 0x12_30_45, 0x26_91_12);
        let state = first.persistent();
        assert_eq!(
            state[USER_FLASH_SIZE..USER_FLASH_SIZE + 4],
            0x12_30_45u32.to_le_bytes()
        );
        assert_eq!(state[USER_FLASH_SIZE + 4..], 0x26_91_12u32.to_le_bytes());

        let mut second = board(&THREE);
        second.load_persistent(&state).unwrap();
        assert_eq!(second.persistent(), state);
    }

    #[test]
    fn state_of_the_wrong_length_is_refused() {
        let mut cpu = board(&THREE);
        for len in [0, USER_FLASH_SIZE, PERSISTENT_SIZE - 4, PERSISTENT_SIZE + 4] {
            assert_eq!(
                cpu.load_persistent(&vec![0; len]),
                Err(PersistentSizeError { got: len })
            );
        }
        assert!(
            cpu.persistent()[..USER_FLASH_SIZE]
                .iter()
                .all(|&byte| byte == 0xFF)
        );
    }

    /// An instruction that is either a halfword or a load of a pool word into a register.
    enum Op {
        Raw(u16),
        Load(u16, usize),
    }
    use Op::{Load, Raw};

    /// The instructions, then the pool they load from, which starts on a word boundary.
    fn assemble(ops: &[Op], pool: &[u32]) -> Vec<u16> {
        let pool_at = (2 * ops.len() as u32).next_multiple_of(4);
        let mut code: Vec<u16> = ops
            .iter()
            .enumerate()
            .map(|(i, op)| match *op {
                Raw(half) => half,
                Load(rt, entry) => {
                    let from = (2 * i as u32 + 4) & !3;
                    0x4800 | rt << 8 | ((pool_at + 4 * entry as u32 - from) / 4) as u16
                }
            })
            .collect();
        code.resize(pool_at as usize / 2, 0);
        code.extend(pool.iter().flat_map(|w| [*w as u16, (*w >> 16) as u16]));
        code
    }

    fn flash_machine() -> (MemoryMap, Irqs) {
        let mut memory = MemoryMap::new();
        let irqs = Irqs::new();
        let controller = FlashController::new(irqs.clone(), memory.flash_ops());
        memory
            .map_peripheral(FLASH_REGS_BASE, FLASH_REGS_SIZE, Box::new(controller))
            .unwrap();
        (memory, irqs)
    }

    const STR_R1_R0: u16 = 0x6001; // str r1,[r0]
    const STR_R2_R0: u16 = 0x6002; // str r2,[r0]
    const STR_R3_R0_8: u16 = 0x6083; // str r3,[r0,#8]
    const SPIN_HERE: u16 = 0xE7FE; // b .
    const KEYR2: u32 = 0x5200_2104;
    const KEY1: u32 = 0x4567_0123;
    const KEY2: u32 = 0xCDEF_89AB;
    const SR2: u32 = 0x5200_2110;

    #[test]
    fn a_flash_erase_clears_the_sector_in_the_core() {
        // Zeroes a word in sector 7 and one in sector 6 of bank 2, unlocks the bank and erases
        // sector 7 (CR2 = SER | START | SNB 7 | PSIZE 3).
        let code = assemble(
            &[
                Load(0, 0),
                Load(1, 1),
                Load(2, 2),
                Load(3, 3),
                Load(4, 4),
                Load(6, 5),
                Raw(0x2500), // movs r5,#0
                Raw(0x6025), // str r5,[r4]
                Raw(0x6035), // str r5,[r6]
                Raw(STR_R1_R0),
                Raw(STR_R2_R0),
                Raw(STR_R3_R0_8),
                Raw(SPIN_HERE),
            ],
            &[KEYR2, KEY1, KEY2, 0x7B4, 0x081E_0000, 0x081C_0000],
        );
        let (memory, irqs) = flash_machine();
        let mut cpu = Cpu::new(&image(&code, &[]), memory, irqs).unwrap();
        cpu.run(30);
        assert_eq!(cpu.read32(0x081E_0000), 0xFFFF_FFFF);
        assert_eq!(cpu.read32(0x081C_0000), 0);
    }

    #[test]
    fn the_flash_interrupt_runs_its_handler_once() {
        // Enables IRQ 4, unlocks bank 2 and starts an erase with EOPIE set. The handler counts
        // in AXI SRAM and clears EOP through FLASH_CCR2.
        let main = assemble(
            &[
                Load(0, 0),
                Raw(0x2110), // movs r1,#0x10
                Raw(0x6001), // str r1,[r0]
                Raw(0xB662), // cpsie i
                Load(0, 1),
                Load(1, 2),
                Load(2, 3),
                Load(3, 4),
                Raw(STR_R1_R0),
                Raw(STR_R2_R0),
                Raw(STR_R3_R0_8),
                Raw(SPIN_HERE),
            ],
            &[0xE000_E100, KEYR2, KEY1, KEY2, 0x1_07B4],
        );
        let handler = assemble(
            &[
                Load(0, 0),
                Raw(0x6801), // ldr r1,[r0]
                Raw(0x3101), // adds r1,#1
                Raw(0x6001), // str r1,[r0]
                Load(2, 1),
                Load(3, 2),
                Raw(0x6013), // str r3,[r2]
                Raw(0x4770), // bx lr
            ],
            &[0x2400_0000, 0x5200_2114, 0x1_0000],
        );
        let offset = 2 * main.len() as u32;
        let code = [main, handler].concat();
        let (memory, irqs) = flash_machine();
        let mut cpu = Cpu::new(&image(&code, &[(4, offset)]), memory, irqs).unwrap();
        cpu.run(200);
        assert_eq!(cpu.memory().read32(0x2400_0000), 1);
        assert_eq!(cpu.memory().read32(SR2), 0);
    }

    /// Enables USART1's interrupt (IRQ 37) in the NVIC, then stores `cr1` in USART1_CR1 and
    /// spins.
    fn usart_main(cr1: u16) -> Vec<u16> {
        assemble(
            &[
                Load(0, 0),
                Raw(0x2120), // movs r1,#0x20
                Raw(0x6001), // str r1,[r0]
                Raw(0xB662), // cpsie i
                Load(0, 1),
                Raw(0x2100 | cr1), // movs r1,#cr1
                Raw(0x6001),
                Raw(SPIN_HERE),
            ],
            &[0xE000_E104, 0x4001_1000],
        )
    }

    fn usart_cpu(main: Vec<u16>, handler: Vec<u16>) -> (Cpu, Console) {
        let irqs = Irqs::new();
        let memory = cmm2_memory(&irqs);
        let console = memory.console();
        let offset = 2 * main.len() as u32;
        let code = [main, handler].concat();
        let cpu = Cpu::new(&image(&code, &[(37, offset)]), memory, irqs).unwrap();
        (cpu, console)
    }

    #[test]
    fn the_usart1_transmit_interrupt_sends_a_byte() {
        // CR1 = UE | TE | TCIE. The handler stops the interrupt, then sends 'A'.
        let handler = assemble(
            &[
                Load(2, 0),
                Raw(0x2109), // movs r1,#9 (TCIE off)
                Raw(0x6011), // str r1,[r2]
                Load(0, 1),
                Raw(0x2141), // movs r1,#'A'
                Raw(0x6001), // str r1,[r0]
                Raw(0x4770), // bx lr
            ],
            &[0x4001_1000, 0x4001_1028],
        );
        let (mut cpu, console) = usart_cpu(usart_main(0x49), handler);
        cpu.run(200);
        assert_eq!(console.take_output(), b"A");
    }

    #[test]
    fn the_usart1_receive_interrupt_delivers_a_byte_once() {
        // CR1 = UE | RE | RXNEIE. The handler reads RDR into AXI SRAM and counts in the next
        // word.
        let handler = assemble(
            &[
                Load(0, 0),
                Raw(0x6801), // ldr r1,[r0]
                Load(2, 1),
                Raw(0x6011), // str r1,[r2]
                Raw(0x6853), // ldr r3,[r2,#4]
                Raw(0x3301), // adds r3,#1
                Raw(0x6053), // str r3,[r2,#4]
                Raw(0x4770), // bx lr
            ],
            &[0x4001_1024, 0x2400_0000],
        );
        let (mut cpu, console) = usart_cpu(usart_main(0x25), handler);
        cpu.run(100);
        assert_eq!(cpu.memory().read32(0x2400_0004), 0);
        console.push_input(b"1");
        cpu.run(200);
        assert_eq!(cpu.memory().read32(0x2400_0000), u32::from(b'1'));
        assert_eq!(cpu.memory().read32(0x2400_0004), 1);
    }

    #[test]
    fn orn_with_an_immediate_ors_the_complement() {
        // orn r3,r5,#127 and orn r4,r5,#0x00FF00FF, with r5 = 0x45.
        let code = assemble(
            &[
                Load(5, 0),
                Raw(0xF065),
                Raw(0x037F),
                Raw(0xF065),
                Raw(0x14FF),
                Raw(SPIN_HERE),
            ],
            &[0x45],
        );
        let mut cpu = cpu(&code);
        cpu.run(4);
        assert_eq!(cpu.reg(3), 0xFFFF_FFC5);
        assert_eq!(cpu.reg(4), 0xFF00_FF45);
    }

    // Runs `instruction`, a Thumb-2 halfword pair, from flash on a core with the FPU on.
    // d0 starts as the pair (r0, r1) = `operands[0..2]`, d1 as `operands[2..4]` and d2 as
    // `operands[4..6]`, so s0 is operands[0], s1 operands[1], s2 operands[2] and s3 operands[3].
    // The result is d0 in r2 (low word) and r3, s0 in r6 and the FPSCR in r7.
    fn run_vfp(instruction: [u16; 2], operands: [u32; 6]) -> Cpu {
        const CPACR: u32 = 0xE000_ED88;
        const FPU_ON: u32 = 0x00F0_0000;
        let ops = [
            Load(0, 0),
            Load(1, 1),
            Raw(STR_R1_R0), // str r1,[r0]
            Load(0, 2),
            Load(1, 3),
            Load(2, 4),
            Load(3, 5),
            Load(4, 6),
            Load(5, 7),
            Raw(0xEC41), // vmov d0, r0, r1
            Raw(0x0B10),
            Raw(0xEC43), // vmov d1, r2, r3
            Raw(0x2B11),
            Raw(0xEC45), // vmov d2, r4, r5
            Raw(0x4B12),
            Raw(instruction[0]),
            Raw(instruction[1]),
            Raw(0xEC53), // vmov r2, r3, d0
            Raw(0x2B10),
            Raw(0xEE10), // vmov r6, s0
            Raw(0x6A10),
            Raw(0xEEF1), // vmrs r7, fpscr
            Raw(0x7A10),
            Raw(SPIN_HERE),
        ];
        let mut pool = vec![CPACR, FPU_ON];
        pool.extend(operands);
        let mut cpu = cpu(&assemble(&ops, &pool));
        // The instruction is the thirteenth, and is one step of four bytes.
        cpu.run(12);
        let pc = cpu.pc();
        assert!(cpu.step().is_none());
        assert_eq!(cpu.pc(), pc + 4);
        assert_eq!(cpu.run(30), Stop::InstructionLimit);
        cpu
    }

    fn f64_pair(x: f64) -> [u32; 2] {
        [x.to_bits() as u32, (x.to_bits() >> 32) as u32]
    }

    fn d0(cpu: &Cpu) -> f64 {
        f64::from_bits(u64::from(cpu.reg(3)) << 32 | u64::from(cpu.reg(2)))
    }

    fn operands64(d0: f64, d1: f64, d2: f64) -> [u32; 6] {
        let [a, b, c, d, e, f] = [d0, d1, d2].map(f64_pair).concat().try_into().unwrap();
        [a, b, c, d, e, f]
    }

    const IXC: u32 = 1 << 4;

    #[test]
    fn vrint_in_every_rounding_runs_from_flash() {
        // vrinta.f64, vrintn.f64, vrintp.f64 and vrintm.f64 of d1 into d0, on 2.5 and -2.5.
        let cases = [
            ([0xFEB8, 0x0B41], [3.0, -3.0]),
            ([0xFEB9, 0x0B41], [2.0, -2.0]),
            ([0xFEBA, 0x0B41], [3.0, -2.0]),
            ([0xFEBB, 0x0B41], [2.0, -3.0]),
        ];
        for (instruction, results) in cases {
            for (x, expected) in [2.5, -2.5].into_iter().zip(results) {
                let cpu = run_vfp(instruction, operands64(9.0, x, 0.0));
                assert_eq!(d0(&cpu), expected, "{instruction:04x?} {x}");
                assert_eq!(cpu.reg(7) & IXC, 0);
            }
        }
    }

    #[test]
    fn vrint_in_single_precision_runs_from_flash() {
        // vrintp.f32 s0, s1 on 2.5, which is s1: the high word of d0.
        let operands = [0, 2.5f32.to_bits(), 0, 0, 0, 0];
        let cpu = run_vfp([0xFEBA, 0x0A60], operands);
        assert_eq!(f32::from_bits(cpu.reg(6)), 3.0);
    }

    #[test]
    fn vrintx_signals_inexact_and_vrintr_does_not() {
        // vrintx.f64 d0, d1 and vrintr.f32 s0, s1; the FPSCR rounding is to nearest.
        let cpu = run_vfp([0xEEB7, 0x0B41], operands64(9.0, 2.5, 0.0));
        assert_eq!(d0(&cpu), 2.0);
        assert_eq!(cpu.reg(7) & IXC, IXC);
        let cpu = run_vfp([0xEEB6, 0x0A60], [0, 3.5f32.to_bits(), 0, 0, 0, 0]);
        assert_eq!(f32::from_bits(cpu.reg(6)), 4.0);
        assert_eq!(cpu.reg(7) & IXC, 0);
    }

    #[test]
    fn vmaxnm_and_vminnm_run_from_flash() {
        // vmaxnm.f64 d0, d1, d2 and vminnm.f32 s0, s1, s2.
        let cpu = run_vfp([0xFE81, 0x0B02], operands64(9.0, -1.5, 4.5));
        assert_eq!(d0(&cpu), 4.5);
        let cpu = run_vfp([0xFE81, 0x0B02], operands64(9.0, f64::NAN, -7.0));
        assert_eq!(d0(&cpu), -7.0);
        let operands = [0, 1.5f32.to_bits(), (-2.5f32).to_bits(), 0, 0, 0];
        let cpu = run_vfp([0xFE80, 0x0AC1], operands);
        assert_eq!(f32::from_bits(cpu.reg(6)), -2.5);
    }

    #[test]
    fn vfnma_runs_from_flash() {
        // vfnma.f64 d0, d1, d2: d0 = -d0 + (-d1) * d2 = -10 + (-2) * 3. And the single
        // precision one, vfnma.f32 s0, s1, s2, the one the firmware has.
        let cpu = run_vfp([0xEE91, 0x0B42], operands64(10.0, 2.0, 3.0));
        assert_eq!(d0(&cpu), -16.0);
        let operands = [
            10.0f32.to_bits(),
            2.0f32.to_bits(),
            3.0f32.to_bits(),
            0,
            0,
            0,
        ];
        let cpu = run_vfp([0xEE90, 0x0AC1], operands);
        assert_eq!(f32::from_bits(cpu.reg(6)), -16.0);
    }

    #[test]
    fn vcvta_and_vcvtn_run_from_flash() {
        // vcvta.s32.f64 s0, d1 and vcvtn.u32.f32 s0, s1.
        let cpu = run_vfp([0xFEBC, 0x0BC1], operands64(0.0, 2.5, 0.0));
        assert_eq!(cpu.reg(6), 3);
        let cpu = run_vfp([0xFEBC, 0x0BC1], operands64(0.0, -2.5, 0.0));
        assert_eq!(cpu.reg(6) as i32, -3);
        let cpu = run_vfp([0xFEBD, 0x0A60], [0, 2.5f32.to_bits(), 0, 0, 0, 0]);
        assert_eq!(cpu.reg(6), 2);
    }

    #[test]
    fn vcvt_with_fraction_bits_runs_from_flash() {
        // vcvt.s32.f32 s0, s0, #16: 1.5 is 0x18000 with 16 fraction bits.
        let cpu = run_vfp([0xEEBE, 0x0AC8], [1.5f32.to_bits(), 0, 0, 0, 0, 0]);
        assert_eq!(cpu.reg(6), 0x1_8000);
        // vcvt.f32.s32 s0, s0, #13: the integer -0x6000 is -3 with 13 fraction bits.
        let cpu = run_vfp([0xEEBA, 0x0AE9], [(-0x6000i32) as u32, 0, 0, 0, 0, 0]);
        assert_eq!(f32::from_bits(cpu.reg(6)), -3.0);
        // vcvt.s16.f64 d0, d0, #5: 100.3 * 32 is 3209.6, truncated to 3209.
        let cpu = run_vfp([0xEEBE, 0x0B65], operands64(100.3, 0.0, 0.0));
        assert_eq!(cpu.reg(2), 3209);
        // vcvt.f64.u32 d0, d0, #32: the low word, as a number of 32 fraction bits.
        let cpu = run_vfp([0xEEBB, 0x0BC0], [0x8000_0000, 0xDEAD, 0, 0, 0, 0]);
        assert_eq!(d0(&cpu), 0.5);
    }

    // Runs `instruction` with r2 = 0x1234_8765 and r3 = 0xCAFE_F00D, and returns r4 and r2.
    fn run_packing(instruction: [u16; 2]) -> (u32, u32) {
        let code = assemble(
            &[
                Load(2, 0),
                Load(3, 1),
                Raw(instruction[0]),
                Raw(instruction[1]),
                Raw(SPIN_HERE),
            ],
            &[0x1234_8765, 0xCAFE_F00D],
        );
        let mut cpu = cpu(&code);
        cpu.run(2);
        let pc = cpu.pc();
        assert!(cpu.step().is_none());
        assert_eq!(cpu.pc(), pc + 4);
        (cpu.reg(4), cpu.reg(2))
    }

    #[test]
    fn the_extend_and_add_instructions_run_from_flash() {
        // uxtah r2, r2, r3; sxtah r2, r2, r3, ror #8; the sums are of r2 = 0x1234_8765 and the
        // halfword of r3 (0xF00D, or 0xFEF0 after the rotate, which is negative).
        assert_eq!(run_packing([0xFA12, 0xF283]).1, 0x1234_8765 + 0xF00D);
        assert_eq!(
            run_packing([0xFA02, 0xF293]).1,
            0x1234_8765u32.wrapping_add(0xFFFF_FEF0)
        );
        // sxtab r4, r2, r3: r2 plus the sign-extended byte 0x0D of r3. With ror #8 the byte is
        // 0xF0, which is negative, and uxtab r4, r2, r3 gives r2 plus the byte 0x0D.
        assert_eq!(run_packing([0xFA42, 0xF483]).0, 0x1234_8765 + 0x0D);
        assert_eq!(
            run_packing([0xFA42, 0xF493]).0,
            0x1234_8765u32.wrapping_add(0xFFFF_FFF0)
        );
        assert_eq!(run_packing([0xFA52, 0xF483]).0, 0x1234_8765 + 0x0D);
        // uxtab r4, r2, r3, ror #8: the byte 0xF0 is not extended by its sign.
        assert_eq!(run_packing([0xFA52, 0xF493]).0, 0x1234_8765 + 0xF0);
    }

    #[test]
    fn the_pack_halfword_instructions_run_from_flash() {
        // pkhbt r2, r2, r3, lsl #16: the bottom of r2 and the top of r3 << 16, which is
        // the bottom of r3.
        assert_eq!(run_packing([0xEAC2, 0x4203]).1, 0xF00D_8765);
        // pkhtb r4, r2, r3, asr #16: the top of r2 and the bottom of r3 >> 16.
        assert_eq!(run_packing([0xEAC2, 0x4423]).0, 0x1234_CAFE);
    }

    #[test]
    fn the_clock_is_the_core_cycle_count_at_the_start_of_the_instruction() {
        let mut cpu = cpu(&SPIN);
        cpu.run(10);
        let before = cpu.cycle_count();
        cpu.step();
        assert_eq!(cpu.memory().clock().now(), before);
        assert!(before > 0);
    }

    // Counts its ticks.
    struct Ticking(Rc<std::cell::Cell<u32>>);

    impl crate::Peripheral for Ticking {
        fn read(&mut self, _offset: u32, _size: u8) -> u32 {
            0
        }

        fn write(&mut self, _offset: u32, _size: u8, _value: u32) {}

        fn tick(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn the_memory_is_ticked_once_when_a_wake_comes_due() {
        let ticks = Rc::new(std::cell::Cell::new(0));
        let mut memory = MemoryMap::new();
        memory
            .map_peripheral(0x5000_0000, 0x100, Box::new(Ticking(ticks.clone())))
            .unwrap();
        memory.clock().wake_at(50);
        let mut cpu = Cpu::new(&image(&SPIN, &[]), memory, Irqs::new()).unwrap();
        while cpu.cycle_count() < 50 {
            assert_eq!(ticks.get(), 0);
            cpu.step();
        }
        cpu.step();
        assert_eq!(ticks.get(), 1);
        cpu.run(100);
        assert_eq!(ticks.get(), 1);
    }

    #[test]
    fn a_timer_counter_advances_with_the_instructions_between_two_reads() {
        // Reads TIM16_CNT into r1, runs 10 nops, reads it into r2.
        let mut ops = vec![Load(0, 0), Raw(0x6801)]; // ldr r1,[r0]
        ops.extend((0..10).map(|_| Raw(0xBF00)));
        ops.extend([Raw(0x6802), Raw(SPIN_HERE)]); // ldr r2,[r0]
        let code = assemble(&ops, &[TIM16_BASE + 0x24]);
        let mut memory = MemoryMap::new();
        let timer = Timer::new(TIM16_BASE, 16, memory.clock());
        memory
            .map_peripheral(TIM16_BASE, TIMER_SIZE, Box::new(timer))
            .unwrap();
        memory.write32(TIM16_BASE, 1); // CEN
        let mut cpu = Cpu::new(&image(&code, &[]), memory, Irqs::new()).unwrap();
        cpu.run(14);
        // Each instruction takes at least a cycle and a tick is two.
        assert!(
            cpu.reg(2) - cpu.reg(1) >= 5,
            "{} {}",
            cpu.reg(1),
            cpu.reg(2)
        );
    }

    #[test]
    fn core_register_matches_zmu() {
        // The core has to be reset before its exception registers can be written.
        let mut processor = Processor::new();
        processor.flash_memory_at(FLASH_BASE, 8, &[0; 8]);
        processor.vtor = FLASH_BASE;
        processor.reset().unwrap();
        let mut wrong = Vec::new();
        for addr in (0xE000_0000..0xE010_0000).step_by(4) {
            // Both sides are expected, except where zmu has only one of them.
            let read_only = matches!(addr,
                0xE000_E004 | 0xE000_E008 | 0xE000_E01C | 0xE000_E300..=0xE000_E33C
                | 0xE000_ED00 | 0xE000_EF40..=0xE000_EF48);
            let write_only = matches!(addr, 0xE000_0004..=0xE000_007C | 0xE000_1FB0 | 0xE000_EF00);
            let claimed = core_register(addr);
            let want = (claimed && !write_only, claimed && !read_only);
            let got = catch_unwind(AssertUnwindSafe(|| {
                (
                    processor.read32(addr).is_ok(),
                    processor.write32(addr, 0).is_ok(),
                )
            }))
            // zmu panics on a few of its own registers (NVIC active bits past its last
            // exception). For a claimed address that is expected; for any other it means
            // zmu's own register code ran, so it counts as a mismatch.
            .unwrap_or(if claimed { want } else { (true, true) });
            if got != want {
                wrong.push(addr);
            }
        }
        let shown: Vec<String> = wrong
            .iter()
            .take(20)
            .map(|a| format!("{a:#010X}"))
            .collect();
        assert!(
            wrong.is_empty(),
            "core_register is out of step with zmu at {shown:?}"
        );
    }

    const AIRCR: u32 = 0xE000_ED0C;
    const SYSRESETREQ: u32 = 0x05FA_0004;
    const FLASH_WORD: u32 = 0x081E_0000;
    const BOOTS: u32 = 0x2400_0100;
    // Where the AIRCR store and the spin loop are in `boot_program`.
    const RESET_REQUEST: u32 = CODE + 26;
    const BOOT_SPIN: u32 = CODE + 30;

    /// Counts its boots in AXI SRAM and stores the count of the first boot in flash. While the
    /// count is below `limit` it asks for a system reset, then it enables interrupts and spins.
    /// Until then they are masked.
    fn boot_program(limit: u16) -> Vec<u16> {
        assemble(
            &[
                Raw(0xB672), // cpsid i
                Load(0, 0),
                Raw(0x6801), // ldr r1,[r0]
                Raw(0x3101), // adds r1,#1
                Raw(0x6001), // str r1,[r0]
                Raw(0x2901), // cmp r1,#1
                Raw(0xD101), // bne +2
                Load(4, 1),
                Raw(0x6021), // str r1,[r4]
                Raw(0x2900 | limit),
                Raw(0xDA02), // bge spin
                Load(2, 2),
                Load(3, 3),
                Raw(0x6013), // str r3,[r2]
                Raw(0xB662), // cpsie i
                Raw(SPIN_HERE),
            ],
            &[BOOTS, FLASH_WORD, AIRCR, SYSRESETREQ],
        )
    }

    fn boot_cpu(limit: u16) -> Cpu {
        let code = [boot_program(limit), COUNT_IN_AXI.to_vec()].concat();
        let handler = 2 * boot_program(limit).len() as u32;
        let image = image(&code, &[(5, handler)]);
        Cpu::new(&image, MemoryMap::new(), Irqs::new()).unwrap()
    }

    #[test]
    fn a_program_that_resets_itself_boots_again() {
        let mut cpu = boot_cpu(3);
        cpu.run(200);
        assert_eq!(cpu.memory().read32(BOOTS), 3);
        assert_eq!(cpu.resets(), 2);
        assert_eq!(cpu.pc(), BOOT_SPIN);
    }

    #[test]
    fn a_reset_keeps_time_and_counts_going() {
        let mut cpu = boot_cpu(2);
        cpu.run_until(100, Some(RESET_REQUEST));
        let (instructions, cycles) = (cpu.instruction_count(), cpu.cycle_count());
        cpu.step();
        assert_eq!(cpu.resets(), 1);
        assert_eq!(cpu.instruction_count(), instructions + 1);
        assert!(cpu.cycle_count() > cycles);
        assert_eq!(cpu.sp(), SP);
        assert_eq!(cpu.pc(), CODE);
    }

    #[test]
    fn flash_survives_a_reset() {
        let mut cpu = boot_cpu(3);
        assert_eq!(cpu.read32(FLASH_WORD), 0xFFFF_FFFF);
        cpu.run(200);
        assert_eq!(cpu.resets(), 2);
        // Only the first boot stores it.
        assert_eq!(cpu.read32(FLASH_WORD), 1);
    }

    #[test]
    fn the_host_can_reset_the_machine() {
        let mut cpu = cpu(&SPIN);
        cpu.run(10);
        cpu.reset();
        assert_eq!(cpu.resets(), 1);
        assert_eq!(cpu.pc(), CODE);
        assert_eq!(cpu.reg(4), 0);
    }

    #[test]
    fn peripherals_reset_but_the_backup_domain_does_not() {
        const GPIOB_MODER: u32 = 0x5802_0400;
        const RCC_CR: u32 = 0x5802_4400;
        const RCC_BDCR: u32 = 0x5802_4470;
        const RCC_RSR: u32 = 0x5802_44D0;
        const RTC_PRER: u32 = 0x5800_4010;
        let code = assemble(
            &[
                Load(0, 0),
                Load(1, 1),
                Raw(STR_R1_R0),
                Load(0, 2),
                Load(1, 3),
                Raw(STR_R1_R0),
                Load(0, 4),
                Load(1, 5),
                Raw(STR_R1_R0),
                Load(0, 6),
                Load(1, 7),
                Raw(STR_R1_R0),
                Load(2, 8),
                Load(3, 9),
                Raw(0x6013), // str r3,[r2]
                Raw(SPIN_HERE),
            ],
            &[
                GPIOB_MODER,
                0x1234_5678,
                RCC_CR,
                0x0001_0083,
                RCC_BDCR,
                0x0000_8111,
                RTC_PRER,
                0x007F_00FF,
                AIRCR,
                SYSRESETREQ,
            ],
        );
        let mut memory = MemoryMap::new();
        memory
            .map_peripheral(
                GPIOB_MODER,
                GPIO_PORT_SIZE,
                Box::new(Gpio::new(1, SdSlot::new())),
            )
            .unwrap();
        memory
            .map_peripheral(RCC_BASE, RCC_SIZE, Box::new(Rcc::new()))
            .unwrap();
        memory
            .map_peripheral(RTC_BASE, RTC_SIZE, Box::new(Rtc::new(memory.clock())))
            .unwrap();
        let mut cpu = Cpu::new(&image(&code, &[]), memory, Irqs::new()).unwrap();
        while cpu.resets() == 0 {
            assert!(cpu.instruction_count() < 100);
            cpu.step();
        }
        let mut memory = cpu.memory();
        assert_eq!(memory.read32(GPIOB_MODER), 0xFFFF_FEBF);
        assert_eq!(memory.read32(RCC_CR), 0x0000_C1A7);
        assert_eq!(memory.read32(RCC_BDCR), 0x0000_8113);
        assert_eq!(memory.read32(RTC_PRER), 0x007F_00FF);
        assert_eq!(memory.read32(RCC_RSR), 0x0140_0000);
    }

    #[test]
    fn the_interrupt_controller_starts_clean_after_a_reset() {
        let mut cpu = boot_cpu(2);
        cpu.run_until(100, Some(RESET_REQUEST));
        cpu.processor.nvic_write_iser(0, 1 << 5);
        cpu.processor.nvic_write_ispr(0, 1 << 5);
        cpu.step();
        assert_eq!(cpu.resets(), 1);
        assert_eq!(cpu.processor.nvic_read_iser(0), 0);
        assert_eq!(cpu.processor.nvic_read_ispr(0), 0);

        // Not enabled, so the handler does not run.
        cpu.irqs().raise(5);
        cpu.run(100);
        assert_eq!(cpu.memory().read32(0x2400_0000), 0);

        cpu.processor.nvic_write_iser(0, 1 << 5);
        cpu.run(100);
        assert_eq!(cpu.memory().read32(0x2400_0000), 1);
    }

    #[test]
    fn systick_is_off_after_a_reset() {
        const SYST_CSR: u32 = 0xE000_E010;
        const SYST_RVR: u32 = 0xE000_E014;
        let mut cpu = boot_cpu(2);
        cpu.run_until(100, Some(RESET_REQUEST));
        cpu.processor.write32(SYST_RVR, 50).unwrap();
        cpu.processor.write32(SYST_CSR, 0b111).unwrap();
        cpu.step();
        assert_eq!(cpu.resets(), 1);
        assert_eq!(cpu.read32(SYST_CSR), 0);
        assert_eq!(cpu.read32(SYST_RVR), 0);
        // The vector table has no SysTick handler: one taken would fault.
        assert_eq!(cpu.run(1000), Stop::InstructionLimit);
        assert_eq!(cpu.pc(), BOOT_SPIN);
    }

    #[test]
    fn queued_flash_ops_and_raised_interrupts_are_dropped_by_a_reset() {
        let mut cpu = boot_cpu(2);
        cpu.run_until(100, Some(RESET_REQUEST));
        cpu.irqs().raise(5);
        cpu.memory().flash_ops().erase(FLASH_WORD, 0x2_0000);
        cpu.step();
        assert_eq!(cpu.resets(), 1);
        assert_eq!(cpu.read32(FLASH_WORD), 1);
        assert_eq!(cpu.processor.nvic_read_ispr(0), 0);
    }
}
