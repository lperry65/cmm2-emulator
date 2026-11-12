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

use crate::firmware::{FLASH_BASE, FLASH_SIZE, Firmware};
use crate::flash_ops::{FlashOp, FlashOps};
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

pub struct Cpu {
    processor: Processor,
    memory: Rc<RefCell<MemoryMap>>,
    irqs: Irqs,
    flash_ops: FlashOps,
    pc: Rc<Cell<u32>>,
}

impl Cpu {
    /// Puts the firmware in flash and resets the core from its vector table.
    ///
    /// Faults stop the run and are not delivered to the guest. That is for bring-up, to see
    /// the first thing that goes wrong; the guest's own fault handlers do not run.
    pub fn new(firmware: &Firmware, memory: MemoryMap, irqs: Irqs) -> Result<Cpu, CpuError> {
        let pc = memory.pc_cell();
        let flash_ops = memory.flash_ops();
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
            flash_ops,
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
    use crate::{FLASH_REGS_BASE, FLASH_REGS_SIZE, FlashController};
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
