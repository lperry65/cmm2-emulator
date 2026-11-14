//!
//! Cortex System simulation
//!

use crate::DeviceBus;
use crate::MemoryMapConfig;
use crate::Processor;
use crate::core::exception::ExceptionHandling;
use crate::core::fault::{FaultContext, FaultTrapMode};
use crate::core::register::BaseReg;
use crate::core::reset::Reset;
use crate::executor::Executor;
use crate::semihosting::SemihostingCommand;
use crate::semihosting::SemihostingResponse;

///
/// Cortex System simulation framework
///
pub struct Simulation {
    /// processor state
    pub processor: Processor,
    /// Simulation Execution mode
    pub exec_mode: SimulationExecMode,
    /// Watchpoints
    pub watchpoints: Vec<u32>,
    /// Breakpoints
    pub breakpoints: Vec<u32>,
    /// The last step halted on a BKPT: the next one starts by taking a pending exception.
    after_halt: bool,
}

///
/// A simulation event.
///
pub enum SimulationEvent {
    /// Step done
    DoneStep,
    /// Processor is halted
    #[allow(dead_code)]
    Halted,
    /// A breakpoint was hit
    Break,
    /// A write watchpoint was hit
    #[allow(dead_code)]
    WatchWrite(u32),
    /// A read watchpoint was hit
    WatchRead(u32),
    /// Execution stopped because a fault trap was hit.
    FaultTrap(FaultContext),
    /// Simulation is finalized
    Finalized(u32),
}

///
/// Mode of execution for the simulation
///
pub enum SimulationExecMode {
    /// Step by Step execution (Single Step)
    Step,
    /// Continuous execution
    Continue,
    /// Range Step execution (from start to end)
    RangeStep(u32, u32),
}

impl Simulation {
    ///
    /// Prepare a new simulation instance
    ///
    pub fn new(
        code: &[u8],
        device: Option<DeviceBus>,
        semihost_func: Box<dyn FnMut(&SemihostingCommand) -> SemihostingResponse + 'static>,
        map: Option<MemoryMapConfig>,
        flash_size: usize,
        fault_trap_mode: FaultTrapMode,
    ) -> Result<Simulation, crate::system::simulation::SimulationError> {
        let mut processor = Processor::new();
        processor.device(device);
        processor.semihost(Some(semihost_func));
        processor.memory_map(map);
        processor.fault_trap_mode(fault_trap_mode);
        processor.flash_memory(flash_size, code);
        processor.cache_instructions();
        processor.running = true;
        processor.debugger_attached = true;
        match processor.reset() {
            Ok(()) => {}
            Err(fault) => {
                return Err(crate::system::simulation::SimulationError::from_fault(
                    &processor, fault,
                ));
            }
        }

        Ok(Simulation {
            processor,
            exec_mode: SimulationExecMode::Step,
            watchpoints: Vec::new(),
            breakpoints: Vec::new(),
            after_halt: false,
        })
    }

    /// Wrap a processor the caller has already built and reset.
    pub fn from_processor(mut processor: Processor) -> Simulation {
        processor.debugger_attached = true;
        Simulation {
            processor,
            exec_mode: SimulationExecMode::Step,
            watchpoints: Vec::new(),
            breakpoints: Vec::new(),
            after_halt: false,
        }
    }

    /// Run the simulation. This function will block until the simulation is complete.
    pub fn run(&mut self, mut poll_incoming_data: impl FnMut() -> bool) -> SimulationRunEvent {
        match self.exec_mode {
            SimulationExecMode::Step => SimulationRunEvent::Event(self.step()),
            SimulationExecMode::Continue => {
                let mut cycles = 0;
                loop {
                    // The poll_incoming_data function checks if there is any data available
                    // to be read from the connection to the GDB server.
                    // Currently, each call to poll_incoming_data blocks until either data
                    // is available or a timeout occurs. This provides a simple mechanism
                    // to detect if the GDB client has sent any commands to the server.
                    // However, this approach is inefficient because the simulation is
                    // effectively stalled until either the GDB client sends data or the
                    // timeout expires.
                    // To reduce the performance impact, we only poll for incoming data
                    // every 1024 cycles.
                    if cycles % 1024 == 0 && poll_incoming_data() {
                        return SimulationRunEvent::IncomingData;
                    }
                    cycles += 1;
                    let evt = self.step();
                    match evt {
                        SimulationEvent::DoneStep => {}
                        _ => return SimulationRunEvent::Event(evt),
                    }
                }
            }
            SimulationExecMode::RangeStep(start, end) => {
                let mut cycles = 0;
                loop {
                    if cycles % 1024 == 0 && poll_incoming_data() {
                        return SimulationRunEvent::IncomingData;
                    }
                    cycles += 1;

                    let evt = self.step();

                    match evt {
                        SimulationEvent::DoneStep => {
                            if !(start..end).contains(&self.processor.get_pc()) {
                                return SimulationRunEvent::Event(evt);
                            }
                        }
                        _ => return SimulationRunEvent::Event(evt),
                    }
                }
            }
        }
    }

    ///
    /// Reset the simulation
    ///
    pub fn reset(&mut self) -> Result<(), crate::system::simulation::SimulationError> {
        match self.processor.reset() {
            Ok(()) => Ok(()),
            Err(fault) => Err(crate::system::simulation::SimulationError::from_fault(
                &self.processor,
                fault,
            )),
        }
    }

    ///
    /// Single step the simulation
    ///
    pub fn step(&mut self) -> SimulationEvent {
        // Halting debug stepping (C1.5, p. C1-695): leaving Debug state takes a pending exception
        // of sufficient priority instead of executing the next instruction
        let mut entered = false;
        if std::mem::take(&mut self.after_halt) && !self.processor.is_locked_up() {
            // an exception that was selected counts, whether its entry worked or failed: only one
            // exception entry is taken in a step
            entered = self.processor.check_exceptions();
        }
        if entered {
            // what an exception entry takes (`handle_fault` counts the same)
            let cycles = self.processor.entry_cycles(false);
            self.processor.account_cycles(cycles);
        } else if self.processor.is_locked_up()
            || (self.processor.running && !self.processor.sleeping)
        {
            self.processor.step();
        } else if self.processor.running {
            // asleep: a WFI leaves running and sleeping both set (as `system/simulation.rs` has it)
            self.processor.step_sleep();
        }
        // a BKPT in the guest halts into the debugger; the halt is consumed here
        let halted = std::mem::take(&mut self.processor.debug_halt);
        self.after_halt = halted;
        if let Some(context) = self.processor.take_pending_fault_trap() {
            return SimulationEvent::FaultTrap(context);
        }
        if halted {
            return SimulationEvent::Break;
        }
        // whenever the core is reported to the debugger it leaves Debug state at the next step
        if self.breakpoints.contains(&self.processor.get_pc()) {
            self.after_halt = true;
            return SimulationEvent::Break;
        }
        if self.watchpoints.contains(&self.processor.get_pc()) {
            self.after_halt = true;
            return SimulationEvent::WatchRead(self.processor.get_pc());
        }
        if !self.processor.running && !self.processor.sleeping && !self.processor.is_locked_up() {
            SimulationEvent::Finalized(self.processor.exit_code)
        } else {
            SimulationEvent::DoneStep
        }
    }
}

///
/// A simulation run event.
///
pub enum SimulationRunEvent {
    /// Incoming data from the GDB server
    IncomingData,
    /// A simulation event
    Event(SimulationEvent),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::Bus;
    use crate::core::exception::Exception;
    use crate::core::register::{Ipsr, Reg};

    // vector table (SP, reset), then `bkpt #0`, `nop`, `bkpt #0xab`
    fn simulation_with_a_bkpt() -> Simulation {
        let mut code = Vec::new();
        code.extend_from_slice(&0x2000_0100_u32.to_le_bytes());
        code.extend_from_slice(&0x0000_0009_u32.to_le_bytes());
        code.extend_from_slice(&[0x00, 0xbe, 0x00, 0xbf]);
        let mut processor = Processor::new();
        processor.flash_memory(code.len(), &code);
        processor.cache_instructions();
        processor.running = true;
        processor.reset().unwrap();
        Simulation::from_processor(processor)
    }

    // C1.5: with a debugger attached (Halting debug) a BKPT halts into the debugger instead of
    // being a HardFault; the debugger sees a breakpoint stop and DFSR.BKPT is set. As on silicon
    // the PC stays on the BKPT, so a continue or a step hits it again until the debugger moves the
    // PC (GDB does, for a compiled-in breakpoint: `set $pc += 2`).
    #[test]
    fn test_a_bkpt_in_guest_code_halts_into_the_debugger_with_the_pc_on_it() {
        let mut simulation = simulation_with_a_bkpt();

        assert!(matches!(simulation.step(), SimulationEvent::Break));
        assert_eq!(simulation.processor.dfsr & (1 << 1), 1 << 1);
        assert_eq!(simulation.processor.psr.get_isr_number(), 0, "no HardFault");
        assert_eq!(simulation.processor.get_pc(), 0x08, "the PC is on the BKPT");
        // nothing moves it: the same BKPT halts again
        assert!(matches!(simulation.step(), SimulationEvent::Break));
        assert_eq!(simulation.processor.get_pc(), 0x08);
        // the debugger writes the PC past it, and execution goes on
        simulation.processor.set_pc(0x0a);
        assert!(matches!(simulation.step(), SimulationEvent::DoneStep));
        assert_eq!(simulation.processor.get_pc(), 0x0c);
    }

    // A halt does not take a pending exception in the same step: the debugger sees the PC on the
    // BKPT, and the exception is still pending.
    #[test]
    fn test_a_halt_does_not_take_a_pending_exception_in_the_same_step() {
        let mut simulation = simulation_with_a_bkpt();
        simulation
            .processor
            .set_exception_pending(crate::core::exception::Exception::PendSV);
        assert!(matches!(simulation.step(), SimulationEvent::Break));
        assert_eq!(simulation.processor.get_pc(), 0x08);
        assert_eq!(simulation.processor.psr.get_isr_number(), 0);
        assert!(
            simulation
                .processor
                .exception_pending(crate::core::exception::Exception::PendSV)
        );
    }

    // The halt is consumed when it is reported, also when a fault trap is reported with it.
    #[test]
    fn test_a_reported_halt_is_not_reported_again() {
        use crate::core::exception::Exception;
        use crate::core::fault::{Fault, FaultContext, FaultTrapReason};
        let mut simulation = simulation_with_a_bkpt();
        simulation.processor.debug_halt = true;
        simulation
            .processor
            .set_pending_fault_trap_for_test(FaultContext {
                trap_reason: FaultTrapReason::Fault,
                fault: Fault::UndefInstr,
                exception: Exception::UsageFault,
                pc: 0,
                active_exception: None,
            });
        assert!(matches!(simulation.step(), SimulationEvent::FaultTrap(_)));
        assert!(!simulation.processor.debug_halt);
    }

    // Vector table with HardFault, PendSV and SysTick (all to 0x4c), then `bkpt`, `cpsid i`, `nop`,
    // `svc #0` and `udf #0` at 0x40, and `bx lr` at 0x4c as the handler.
    fn simulation_with_a_pendsv_handler() -> Simulation {
        let mut code = vec![0_u8; 0x50];
        code[0..4].copy_from_slice(&0x2000_0100_u32.to_le_bytes());
        code[4..8].copy_from_slice(&0x0000_0041_u32.to_le_bytes());
        code[0x38..0x3c].copy_from_slice(&0x0000_004d_u32.to_le_bytes());
        code[0x0c..0x10].copy_from_slice(&0x0000_004d_u32.to_le_bytes());
        code[0x40..0x46].copy_from_slice(&[0x00, 0xbe, 0x72, 0xb6, 0x00, 0xbf]);
        code[0x3c..0x40].copy_from_slice(&0x0000_004d_u32.to_le_bytes());
        code[0x46..0x4a].copy_from_slice(&[0x00, 0xdf, 0x00, 0xde]); // svc #0, udf #0
        code[0x4c..0x4e].copy_from_slice(&[0x70, 0x47]); // bx lr
        let mut processor = Processor::new();
        processor.flash_memory(code.len(), &code);
        processor.cache_instructions();
        processor.running = true;
        processor.reset().unwrap();
        processor.execution_priority = processor.get_execution_priority();
        Simulation::from_processor(processor)
    }

    // C1.5 Halting debug stepping (p. C1-695): a step out of Debug state takes a pending exception
    // of sufficient priority instead of executing the next instruction: with PendSV pending, the
    // step after `set $pc` takes it before `cpsid i` runs.
    #[test]
    fn test_a_step_after_a_halt_takes_a_pending_exception_before_the_next_instruction() {
        let mut simulation = simulation_with_a_pendsv_handler();
        assert!(matches!(simulation.step(), SimulationEvent::Break));
        simulation
            .processor
            .set_exception_pending(Exception::PendSV);
        simulation.processor.set_pc(0x42); // the cpsid i
        assert!(matches!(simulation.step(), SimulationEvent::DoneStep));
        assert_eq!(simulation.processor.psr.get_isr_number(), 14);
        assert_eq!(simulation.processor.get_pc(), 0x4c);
        assert!(!simulation.processor.primask, "cpsid i has not run");
    }

    // and a continue with the PC still on the BKPT takes the exception, and does not halt again
    #[test]
    fn test_a_continue_from_a_halt_takes_a_pending_exception_and_does_not_halt_again() {
        let mut simulation = simulation_with_a_pendsv_handler();
        assert!(matches!(simulation.step(), SimulationEvent::Break));
        simulation
            .processor
            .set_exception_pending(Exception::PendSV);
        assert!(matches!(simulation.step(), SimulationEvent::DoneStep));
        assert_eq!(simulation.processor.psr.get_isr_number(), 14);
        assert_eq!(simulation.processor.get_pc(), 0x4c);
    }

    // An exception entry that fails while stacking is still the one exception entry of the step:
    // the derived HardFault is taken and the next instruction does not run in the same step.
    #[test]
    fn test_a_failed_entry_after_a_halt_does_not_run_the_next_instruction() {
        let mut simulation = simulation_with_a_pendsv_handler();
        assert!(matches!(simulation.step(), SimulationEvent::Break));
        simulation.processor.ccr = 0; // STKALIGN clear: the xPSR word is past the end of SRAM
        simulation.processor.set_msp(0x2002_0004);
        simulation
            .processor
            .set_exception_pending(Exception::PendSV);
        simulation.processor.set_pc(0x42); // the cpsid i
        assert!(matches!(simulation.step(), SimulationEvent::FaultTrap(_)));
        assert_eq!(simulation.processor.psr.get_isr_number(), 3);
        assert_eq!(simulation.processor.get_pc(), 0x4c);
        assert!(!simulation.processor.primask, "cpsid i has not run");
        assert!(simulation.processor.exception_pending(Exception::PendSV));
    }

    // C1-695 applies whenever the core leaves Debug state, after a breakpoint from the debugger's
    // list too: the step takes the pending exception before the next instruction, and it costs
    // what an exception entry costs (12 cycles).
    #[test]
    fn test_a_step_after_a_listed_breakpoint_takes_a_pending_exception_first() {
        let mut simulation = simulation_with_a_pendsv_handler();
        simulation.processor.set_pc(0x44); // the nop
        simulation.breakpoints.push(0x46);
        assert!(matches!(simulation.step(), SimulationEvent::Break));
        assert_eq!(simulation.processor.get_pc(), 0x46);
        simulation
            .processor
            .set_exception_pending(Exception::PendSV);
        let cycles = simulation.processor.cycle_count;
        assert!(matches!(simulation.step(), SimulationEvent::DoneStep));
        assert_eq!(simulation.processor.psr.get_isr_number(), 14);
        let frame = simulation.processor.get_msp();
        assert_eq!(simulation.processor.read32(frame + 0x18), Ok(0x46));
        assert_eq!(simulation.processor.cycle_count, cycles + 12);
    }

    // An exception taken at the end of an ordinary step costs the cycles of an exception entry
    // too, like on the path after a halt.
    #[test]
    fn test_an_exception_taken_after_an_instruction_counts_its_entry_cycles() {
        let mut simulation = simulation_with_a_pendsv_handler();
        simulation.processor.set_pc(0x44); // the nop
        simulation
            .processor
            .set_exception_pending(Exception::PendSV);
        let cycles = simulation.processor.cycle_count;
        simulation.step();
        assert_eq!(simulation.processor.psr.get_isr_number(), 14);
        assert_eq!(simulation.processor.cycle_count, cycles + 1 + 12);
    }

    // The same from sleep.
    #[test]
    fn test_an_exception_taken_from_sleep_counts_its_entry_cycles() {
        let mut simulation = simulation_with_a_pendsv_handler();
        // what a WFI leaves: sleeping, and still running
        simulation.processor.sleeping = true;
        simulation
            .processor
            .set_exception_pending(Exception::PendSV);
        let cycles = simulation.processor.cycle_count;
        simulation.step();
        assert_eq!(simulation.processor.psr.get_isr_number(), 14);
        // the cycle of the sleeping step, and the entry
        assert_eq!(simulation.processor.cycle_count, cycles + 1 + 12);
    }

    // The cycles of the step that returns from PendSV (`bx lr`), with and without SysTick pending
    // behind it.
    fn cycles_of_a_return_from_pendsv(systick_pending: bool) -> (u64, u32) {
        let mut simulation = simulation_with_a_pendsv_handler();
        simulation
            .processor
            .set_exception_priority(Exception::PendSV, 0);
        simulation
            .processor
            .set_exception_priority(Exception::SysTick, 0x40);
        simulation.processor.set_pc(0x44);
        simulation
            .processor
            .set_exception_pending(Exception::PendSV);
        simulation.step();
        assert_eq!(simulation.processor.psr.get_isr_number(), 14);
        if systick_pending {
            simulation
                .processor
                .set_exception_pending(Exception::SysTick);
        }
        let cycles = simulation.processor.cycle_count;
        simulation.step();
        let isr = simulation.processor.psr.get_isr_number();
        (simulation.processor.cycle_count - cycles, isr as u32)
    }

    // An exception taken straight after an exception return is tail-chained: it costs 6 cycles,
    // not the 12 of a full entry.
    #[test]
    fn test_a_tail_chained_exception_costs_the_tail_chain_cycles() {
        let (plain, isr) = cycles_of_a_return_from_pendsv(false);
        assert_eq!(isr, 0, "back in Thread mode");
        let (chained, isr) = cycles_of_a_return_from_pendsv(true);
        assert_eq!(isr, 15, "SysTick is tail-chained");
        assert_eq!(chained - plain, 6);
    }

    // The tail-chain flag is per step: a return in one step does not make the exception taken
    // after a later instruction a tail-chain.
    #[test]
    fn test_an_exception_after_a_later_instruction_is_not_tail_chained() {
        let mut simulation = simulation_with_a_pendsv_handler();
        simulation.processor.set_pc(0x44);
        simulation
            .processor
            .set_exception_pending(Exception::PendSV);
        simulation.step(); // entered
        simulation.step(); // bx lr: back in Thread mode, nothing pending
        assert_eq!(simulation.processor.psr.get_isr_number(), 0);
        simulation.processor.set_pc(0x44);
        simulation
            .processor
            .set_exception_pending(Exception::PendSV);
        let cycles = simulation.processor.cycle_count;
        simulation.step(); // the nop, and PendSV after it
        assert_eq!(simulation.processor.psr.get_isr_number(), 14);
        assert_eq!(simulation.processor.cycle_count, cycles + 1 + 12);
    }

    // A lockup is not an entry, on any path: the step counts the one cycle of the fetch, and the
    // core stops. Here VTOR is below SRAM, so no vector can be read.
    #[test]
    fn test_a_lockup_after_an_instruction_counts_one_cycle_and_no_entry() {
        let mut simulation = simulation_with_a_pendsv_handler();
        simulation.processor.vtor = 0x1fff_ff80;
        simulation.processor.set_pc(0x44);
        simulation
            .processor
            .set_exception_pending(Exception::PendSV);
        let cycles = simulation.processor.cycle_count;
        simulation.step();
        assert!(!simulation.processor.running);
        assert_eq!(simulation.processor.cycle_count, cycles + 1 + 1);
    }

    // A tail-chain that locks up counts the one cycle too, not the tail-chain's six.
    #[test]
    fn test_a_lockup_after_an_exception_return_counts_one_cycle() {
        let (plain, _) = cycles_of_a_return_from_pendsv(false);
        let mut simulation = simulation_with_a_pendsv_handler();
        simulation
            .processor
            .set_exception_priority(Exception::SysTick, 0x40);
        simulation.processor.set_pc(0x44);
        simulation
            .processor
            .set_exception_pending(Exception::PendSV);
        simulation.step();
        simulation.processor.vtor = 0x1fff_ff80;
        simulation
            .processor
            .set_exception_pending(Exception::SysTick);
        let cycles = simulation.processor.cycle_count;
        simulation.step();
        assert!(!simulation.processor.running);
        assert_eq!(simulation.processor.cycle_count - cycles, plain + 1);
    }

    // A pending Reset whose vectors cannot be read is a lockup at priority -1 with the lockup
    // address 0xFFFFFFFE (B1-556), with VECTTBL and without FORCED, and no entry. `reset()` loads
    // VTOR from the boot address (ResetSCSRegs resets it, INITVTOR on the Cortex-M7).
    #[test]
    fn test_a_failed_reset_is_a_lockup_at_the_lockup_address() {
        let mut simulation = simulation_with_a_pendsv_handler();
        simulation.processor.boot_address = 0x1fff_ff80; // reset loads it into VTOR
        simulation.processor.set_pc(0x44);
        simulation.processor.set_exception_pending(Exception::Reset);
        let cycles = simulation.processor.cycle_count;
        let event = simulation.step();
        let SimulationEvent::FaultTrap(trap) = event else {
            panic!("a lockup trap expected");
        };
        assert_eq!(
            trap.trap_reason,
            crate::core::fault::FaultTrapReason::Lockup
        );
        assert_eq!(trap.fault, crate::core::fault::Fault::VectorTable);
        assert!(simulation.processor.is_locked_up());
        assert!(!simulation.processor.running);
        assert_eq!(simulation.processor.get_pc(), 0xffff_fffe);
        assert_eq!(simulation.processor.hfsr & (1 << 30), 0, "no FORCED");
        assert_eq!(simulation.processor.hfsr & (1 << 1), 1 << 1, "VECTTBL");
        assert_eq!(simulation.processor.cycle_count, cycles + 1 + 1);
    }

    // ---- lockup through `step()` ---------------------------------------------------------------

    // The lockup of an instruction (SVC or UDF with the vectors unreadable) leaves its exception
    // pending, but the step does not retry the entry: one cycle for the step that found it, then
    // one per step of the fetch of the lockup address, and the trap once.
    #[test]
    fn test_a_lockup_through_step_is_counted_and_trapped_once() {
        for (pc, name) in [(0x46_u32, "svc"), (0x48, "udf")] {
            let mut simulation = simulation_with_a_pendsv_handler();
            simulation.processor.vtor = 0x1fff_ff80;
            simulation.processor.write32(0xE000_ED24, 1 << 18).unwrap();
            simulation.processor.set_pc(pc);
            let cycles = simulation.processor.cycle_count;
            assert!(
                matches!(simulation.step(), SimulationEvent::FaultTrap(_)),
                "{name}"
            );
            assert_eq!(simulation.processor.cycle_count, cycles + 1, "{name}");
            assert!(simulation.processor.is_locked_up(), "{name}");
            for _ in 0..3 {
                let cycles = simulation.processor.cycle_count;
                assert!(
                    matches!(simulation.step(), SimulationEvent::DoneStep),
                    "{name}: no new trap"
                );
                assert_eq!(simulation.processor.cycle_count, cycles + 1, "{name}");
                assert!(simulation.processor.is_locked_up(), "{name}");
            }
        }
    }

    // An NMI leaves the lockup of a HardFault handler (B1-555): the udf in the handler locks up,
    // and the next step takes the NMI, with a full entry, and the core runs again.
    #[test]
    fn test_an_nmi_leaves_a_hardfault_lockup() {
        let mut simulation = simulation_with_a_pendsv_handler();
        simulation
            .processor
            .test_set_active_exception(Exception::HardFault);
        simulation.processor.execution_priority = simulation.processor.get_execution_priority();
        simulation.processor.set_pc(0x48); // udf
        simulation.processor.set_exception_pending(Exception::NMI);
        let cycles = simulation.processor.cycle_count;
        simulation.step();
        assert!(simulation.processor.is_locked_up());
        assert_eq!(simulation.processor.cycle_count, cycles + 1);
        let cycles = simulation.processor.cycle_count;
        simulation.step();
        assert!(!simulation.processor.is_locked_up());
        assert!(simulation.processor.running);
        assert_eq!(simulation.processor.psr.get_isr_number(), 2);
        assert_eq!(simulation.processor.cycle_count, cycles + 12);
    }

    // A lockup found while the core sleeps (WFI leaves running and sleeping both set) counts one.
    #[test]
    fn test_a_lockup_from_sleep_counts_one_cycle() {
        let mut simulation = simulation_with_a_pendsv_handler();
        simulation.processor.vtor = 0x1fff_ff80;
        simulation.processor.sleeping = true;
        simulation
            .processor
            .set_exception_pending(Exception::PendSV);
        let cycles = simulation.processor.cycle_count;
        simulation.step();
        assert!(simulation.processor.is_locked_up());
        // the cycle of the sleeping step, and one for the lockup
        assert_eq!(simulation.processor.cycle_count, cycles + 1 + 1);
    }

    // A lockup on the entry that starts the step after a halt counts one.
    #[test]
    fn test_a_lockup_after_a_halt_counts_one_cycle() {
        let mut simulation = simulation_with_a_pendsv_handler();
        assert!(matches!(simulation.step(), SimulationEvent::Break));
        simulation.processor.vtor = 0x1fff_ff80;
        simulation
            .processor
            .set_exception_pending(Exception::PendSV);
        let cycles = simulation.processor.cycle_count;
        simulation.step();
        assert!(simulation.processor.is_locked_up());
        assert_eq!(simulation.processor.cycle_count, cycles + 1);
    }

    // A locked-up core after a halt goes through the lockup step, not the after-halt entry: an NMI
    // leaves the lockup properly.
    #[test]
    fn test_a_lockup_after_a_halt_is_left_by_an_nmi_through_the_lockup_step() {
        let mut simulation = simulation_with_a_pendsv_handler();
        assert!(matches!(simulation.step(), SimulationEvent::Break));
        simulation.processor.lock_up(-1);
        simulation.processor.set_exception_pending(Exception::NMI);
        simulation.step();
        assert!(!simulation.processor.is_locked_up());
        assert!(simulation.processor.running);
        assert_eq!(simulation.processor.psr.get_isr_number(), 2);
    }

    // Lock-ups that are not found by `step`: what the entry cycles are decided by is the lockup, not
    // `running`, which a semihosting exit clears too.
    #[test]
    fn test_the_entry_cycles_follow_the_lockup_and_not_running() {
        let mut processor = Processor::new();
        processor.running = false; // as after a semihosting exit
        assert_eq!(processor.entry_cycles(false), 12);
        assert_eq!(processor.entry_cycles(true), 6);
        processor.lock_up(-1);
        assert_eq!(processor.entry_cycles(false), 1);
    }

    // A sleeping core counts the cycles it sleeps through in the cycle counter, as in DWT and
    // SysTick (see NOTICE.md).
    #[test]
    fn test_a_sleeping_step_counts_its_cycle() {
        let mut simulation = simulation_with_a_pendsv_handler();
        simulation.processor.sleeping = true;
        let cycles = simulation.processor.cycle_count;
        simulation.step();
        assert_eq!(simulation.processor.cycle_count, cycles + 1);
    }

    // ---- lockup priorities, addresses and resets (B1.5.15, B1-555 to B1-558) ----------------

    // An NMI whose vector cannot be read locks up at priority -2 whatever the running priority
    // (B1-556), so the NMI that is pended again is not retried and traps once.
    #[test]
    fn test_an_nmi_vector_read_error_locks_up_at_minus_two() {
        let mut simulation = simulation_with_a_pendsv_handler();
        simulation.processor.vtor = 0x1fff_ff80;
        simulation.processor.set_pc(0x44);
        simulation.processor.set_exception_pending(Exception::NMI);
        assert!(matches!(simulation.step(), SimulationEvent::FaultTrap(_)));
        assert!(simulation.processor.is_locked_up());
        assert_eq!(simulation.processor.execution_priority, -2);
        for _ in 0..3 {
            assert!(
                matches!(simulation.step(), SimulationEvent::DoneStep),
                "the NMI is not retried"
            );
            assert!(simulation.processor.is_locked_up());
        }
    }

    // The same with the guard: an NMI pended behind an SVC that locks up is retried once, in the
    // locked step, and not in the step that found the lockup.
    #[test]
    fn test_an_nmi_pended_behind_a_lockup_is_tried_once_in_the_locked_step() {
        let mut simulation = simulation_with_a_pendsv_handler();
        simulation.processor.vtor = 0x1fff_ff80;
        simulation.processor.set_pc(0x46); // svc
        simulation.processor.set_exception_pending(Exception::NMI);
        let cycles = simulation.processor.cycle_count;
        assert!(matches!(simulation.step(), SimulationEvent::FaultTrap(_)));
        assert_eq!(simulation.processor.cycle_count, cycles + 1);
        // the NMI is tried, and its vector fails: a second lockup, at -2, and one more trap
        assert!(matches!(simulation.step(), SimulationEvent::FaultTrap(_)));
        for _ in 0..3 {
            assert!(matches!(simulation.step(), SimulationEvent::DoneStep));
        }
    }

    // INVPC on an NMI return to an execution priority of -1 locks up at -2 and at 0xFFFFFFFE
    // (B1-558): the NMI that is pended is not taken on top of the active one.
    #[test]
    fn test_invpc_on_an_nmi_return_locks_up_at_minus_two_at_the_lockup_address() {
        let mut simulation = simulation_with_a_pendsv_handler();
        simulation
            .processor
            .test_set_active_exception(Exception::HardFault);
        simulation
            .processor
            .test_set_active_exception(Exception::NMI);
        simulation.processor.execution_priority = simulation.processor.get_execution_priority();
        simulation.processor.set_r(Reg::LR, 0xffff_fff9);
        simulation.processor.set_pc(0x4c); // bx lr
        simulation.processor.set_exception_pending(Exception::NMI);
        assert!(matches!(simulation.step(), SimulationEvent::FaultTrap(_)));
        assert!(simulation.processor.is_locked_up());
        assert_eq!(simulation.processor.execution_priority, -2);
        assert_eq!(simulation.processor.get_pc(), 0xffff_fffe);
        assert!(matches!(simulation.step(), SimulationEvent::DoneStep));
        assert!(simulation.processor.is_locked_up());
        assert_eq!(simulation.processor.psr.get_isr_number(), 2);
    }

    // An unstacking fault on a return from NMI to the HardFault handler locks up at -1 and at
    // 0xFFFFFFFE (B1-557), where the `bx lr` was.
    #[test]
    fn test_an_unstacking_fault_on_an_nmi_return_locks_up_at_the_lockup_address() {
        let mut simulation = simulation_with_a_pendsv_handler();
        simulation
            .processor
            .test_set_active_exception(Exception::HardFault);
        simulation
            .processor
            .test_set_active_exception(Exception::NMI);
        simulation.processor.execution_priority = simulation.processor.get_execution_priority();
        simulation.processor.set_msp(0x1fff_ff00); // unmapped
        simulation.processor.set_r(Reg::LR, 0xffff_fff1);
        simulation.processor.set_pc(0x4c);
        assert!(matches!(simulation.step(), SimulationEvent::FaultTrap(_)));
        assert!(simulation.processor.is_locked_up());
        assert_eq!(simulation.processor.execution_priority, -1);
        assert_eq!(simulation.processor.get_pc(), 0xffff_fffe);
    }

    // A reset recovers a locked core completely: the next step runs instructions again.
    #[test]
    fn test_a_reset_recovers_a_locked_core() {
        let mut simulation = simulation_with_a_pendsv_handler();
        simulation
            .processor
            .test_set_active_exception(Exception::HardFault);
        simulation.processor.execution_priority = simulation.processor.get_execution_priority();
        simulation.processor.set_pc(0x48); // udf in the HardFault handler
        assert!(matches!(simulation.step(), SimulationEvent::FaultTrap(_)));
        assert!(simulation.processor.is_locked_up());
        simulation.reset().unwrap();
        assert!(!simulation.processor.is_locked_up());
        assert!(simulation.processor.running);
        assert!(
            matches!(simulation.step(), SimulationEvent::Break),
            "the bkpt at the reset vector"
        );
    }

    // The core can be stepped out of a lockup at -1 by an NMI, and `lockup_can_end` says so; one
    // at -2 cannot.
    #[test]
    fn test_only_a_lockup_below_minus_two_cannot_be_ended_by_an_nmi() {
        let mut processor = Processor::new();
        processor.lock_up(-1);
        assert!(!processor.lockup_can_end());
        processor.set_exception_pending(Exception::NMI);
        assert!(processor.lockup_can_end());
        let mut processor = Processor::new();
        processor.lock_up(-2);
        processor.set_exception_pending(Exception::NMI);
        assert!(!processor.lockup_can_end());
    }

    // The same through the GDB simulation's reset.
    #[test]
    fn test_a_failed_simulation_reset_leaves_the_core_locked_up() {
        let mut simulation = simulation_with_a_pendsv_handler();
        simulation
            .processor
            .test_set_active_exception(Exception::HardFault);
        simulation.processor.execution_priority = simulation.processor.get_execution_priority();
        simulation.processor.set_pc(0x48); // udf in the HardFault handler
        simulation.step();
        assert!(simulation.processor.is_locked_up());
        simulation.processor.boot_address = 0x1fff_ff80;
        assert!(simulation.reset().is_err());
        assert!(simulation.processor.is_locked_up());
        assert!(!simulation.processor.running);
        assert_eq!(simulation.processor.get_pc(), 0xffff_fffe);
    }
}
