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
use zmu_cortex_m::executor::Executor;
use zmu_cortex_m::peripheral::nvic::NVIC;

use crate::firmware::{FLASH_BASE, FLASH_SIZE, Firmware};
use crate::irq::Irqs;
use crate::memory::MemoryMap;

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

/// What stopped the CPU: the address of the instruction and the core's description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    pub pc: u32,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    InstructionLimit,
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

pub struct Cpu {
    processor: Processor,
    memory: Rc<RefCell<MemoryMap>>,
    irqs: Irqs,
    pc: Rc<Cell<u32>>,
}

impl Cpu {
    /// Puts the firmware in flash and resets the core from its vector table.
    ///
    /// Faults stop the run and are not delivered to the guest. That is for bring-up, to see
    /// the first thing that goes wrong; the guest's own fault handlers do not run.
    pub fn new(firmware: &Firmware, memory: MemoryMap, irqs: Irqs) -> Result<Cpu, CpuError> {
        let pc = memory.pc_cell();
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
        processor.vtor = FLASH_BASE;
        processor
            .reset()
            .map_err(|fault| CpuError::Reset(fault.to_string()))?;
        processor.running = true;

        Ok(Cpu {
            processor,
            memory,
            irqs,
            pc,
        })
    }

    /// Executes one instruction, then hands any raised interrupts to the interrupt
    /// controller, which takes them when they are enabled. Returns the fault if the
    /// instruction caused one.
    pub fn step(&mut self) -> Option<Fault> {
        self.pc.set(self.processor.get_pc());
        self.processor.step();
        if let Some(raised) = self.irqs.take() {
            for (index, bits) in raised.into_iter().enumerate() {
                self.processor.nvic_write_ispr(index, bits);
            }
        }
        self.processor.take_pending_fault_trap().map(|trap| Fault {
            pc: trap.pc,
            description: trap.to_string(),
        })
    }

    /// Steps until `max_instructions` more have run or a fault stops it.
    pub fn run(&mut self, max_instructions: u64) -> Stop {
        let limit = self.instruction_count().saturating_add(max_instructions);
        while self.instruction_count() < limit {
            if let Some(fault) = self.step() {
                return Stop::Fault(fault);
            }
        }
        Stop::InstructionLimit
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
}
