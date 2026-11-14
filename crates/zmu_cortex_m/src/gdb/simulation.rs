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

/// Cycles of an exception entry, as `handle_fault` counts them.
const EXCEPTION_ENTRY_CYCLES: u32 = 12;

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
        if std::mem::take(&mut self.after_halt) {
            // an exception that was selected counts, whether its entry worked or failed: only one
            // exception entry is taken in a step
            entered = self.processor.check_exceptions();
        }
        if entered {
            // what an exception entry takes (`handle_fault` counts the same)
            self.processor.account_cycles(EXCEPTION_ENTRY_CYCLES);
        } else if self.processor.running {
            self.processor.step();
        } else if self.processor.sleeping {
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
        if !self.processor.running && !self.processor.sleeping {
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
    use crate::core::register::Ipsr;

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

    // vector table with PendSV, then `bkpt`, `cpsid i`, `nop` at 0x40 and the handler at 0x4c
    fn simulation_with_a_pendsv_handler() -> Simulation {
        let mut code = vec![0_u8; 0x50];
        code[0..4].copy_from_slice(&0x2000_0100_u32.to_le_bytes());
        code[4..8].copy_from_slice(&0x0000_0041_u32.to_le_bytes());
        code[0x38..0x3c].copy_from_slice(&0x0000_004d_u32.to_le_bytes());
        code[0x0c..0x10].copy_from_slice(&0x0000_004d_u32.to_le_bytes());
        code[0x40..0x46].copy_from_slice(&[0x00, 0xbe, 0x72, 0xb6, 0x00, 0xbf]);
        code[0x4c..0x4e].copy_from_slice(&[0x00, 0xbf]);
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
}
