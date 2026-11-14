//!
//! Functionality for representing Cortex Exceptions.
//!
//!

use crate::Processor;
use crate::ProcessorMode;
use crate::bus::Bus;
use crate::core::bits::Bits;
use crate::core::fault::{Fault, FaultStatusContext, RefusedReturn};
#[cfg(feature = "has-fp")]
use crate::core::register::ExtensionRegOperations;
#[cfg(feature = "has-fp")]
use crate::core::register::SingleReg;
use crate::core::register::{BaseReg, Ipsr, Reg};
use crate::core::reset::Reset;
#[cfg(feature = "has-fp")]
use crate::executor::FloatingPointChecks;
use crate::peripheral::nvic::NVIC;
use crate::peripheral::scb::CCR_STKALIGN;
#[cfg(feature = "has-fp")]
use crate::peripheral::scb::{
    DEMCR_MON_EN, FPCCR_BFRDY, FPCCR_HFRDY, FPCCR_LSPACT, FPCCR_LSPEN, FPCCR_MMRDY, FPCCR_MONRDY,
    FPCCR_THREAD, FPCCR_USER, SystemControlBlock,
};

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd, Copy, Clone)]
///
/// Status information for an exception
///
pub struct ExceptionState {
    priority: i16,
    pending: bool,
    active: bool,
    exception_number: usize,
}

impl ExceptionState {
    ///
    /// Create a state information for specific exception with given priority
    ///
    pub fn new(exception: Exception, priority: i16) -> Self {
        Self {
            exception_number: usize::from(exception),
            priority,
            pending: false,
            active: false,
        }
    }
}

impl Processor {
    /// Implements the ARMv7-M `ExecutionPriority()` pseudocode.
    ///
    /// Returns the effective execution priority after applying active
    /// exceptions, BASEPRI / PRIMASK / FAULTMASK, and PRIGROUP.
    ///
    /// This differs from `get_exception_priority()`, which returns a stored
    /// SHPRx/IPR priority for one exception.
    fn calculate_execution_priority_helper(&self, include_primask: bool) -> i16 {
        let mut highestpri: i16 = 256;
        let mut boostedpri: i16 = 256;
        let subgroupshift = self.aircr.get_bits(8..11);
        let mut groupvalue = 2 << subgroupshift;

        for (_, exp) in self.exceptions.iter().filter(|&(_, e)| e.active) {
            if exp.priority < highestpri {
                highestpri = exp.priority;

                if exp.exception_number == Exception::NMI.into() {
                    groupvalue = -2;
                }

                if exp.exception_number == Exception::HardFault.into() {
                    groupvalue = -1;
                }

                let subgroupvalue = highestpri % groupvalue;
                highestpri -= subgroupvalue;
            }
        }
        #[cfg(not(feature = "armv6m"))]
        if self.basepri != 0 {
            boostedpri = i16::from(self.basepri);
            let subgroupvalue = boostedpri % groupvalue;
            boostedpri -= subgroupvalue;
        }

        if include_primask && self.primask {
            boostedpri = 0;
        }

        #[cfg(not(feature = "armv6m"))]
        {
            if self.faultmask {
                boostedpri = -1;
            }
        }

        if boostedpri < highestpri {
            boostedpri
        } else {
            highestpri
        }
    }

    ///
    /// Check if WFI wakeup condition is met (ignoring PRIMASK)
    ///
    pub fn has_wakeup_condition(&self) -> bool {
        // We specifically ignore PRIMASK checks here as WFI should wake up
        // if an interrupt is pending which would be taken if PRIMASK was clear.
        let wakeup_priority = self.calculate_execution_priority_helper(false);

        self.exceptions
            .values()
            .any(|e| e.pending && e.priority < wakeup_priority)
    }

    #[cfg(test)]
    pub(crate) fn test_set_active_exception(&mut self, exception: Exception) {
        self.psr.set_isr_number(exception.into());
        self.exceptions
            .get_mut(&usize::from(exception))
            .unwrap()
            .active = true;
        self.mode = ProcessorMode::HandlerMode;
    }
}

///
/// Trait for interacting with exceptions
///
pub trait ExceptionHandling {
    ///
    /// Get the current processor execution priority (EP).
    ///
    /// This is the ARM pseudocode `ExecutionPriority()`, not a single
    /// exception's stored SHPR/IPR priority.
    ///
    fn get_execution_priority(&self) -> i16;

    ///
    /// Set exception pending if it is not already pending
    ///
    fn set_exception_pending(&mut self, exception: Exception);

    ///
    /// Get the currently highest priority pending exception
    ///
    fn get_pending_exception(&self) -> Option<Exception>;

    ///
    /// Clear the pending status of an exception
    ///
    fn clear_pending_exception(&mut self, exception: Exception);

    ///
    /// Check if given exception is currently pending.
    ///
    fn exception_pending(&self, exception: Exception) -> bool;

    ///
    /// Enter an exception.
    ///
    /// Return adress is the address to which the execution should return when this exception is returned from.
    ///
    fn exception_entry(&mut self, exception: Exception, return_address: u32) -> Result<(), Fault>;

    ///
    /// Return from an exception.
    ///
    /// `exc_return` determines the mode to which to return to.
    ///
    /// Exception return happens when processor is in `HandlerMode` and `exc_return` value is loaded to PC using
    /// LDM, POP, LDR, or BX instructions
    ///
    fn exception_return(&mut self, exc_return: u32) -> Result<(), Fault>;

    ///
    /// Check if given exception is currently active
    ///
    fn exception_active(&self, exception: Exception) -> bool;

    ///
    /// Set priority of an exception. Smaller priority number has higher urgency.
    ///
    fn set_exception_priority(&mut self, exception: Exception, priority: u8);

    ///
    /// Get the configured priority of an exception.
    ///
    /// For configurable exceptions this corresponds to the stored SHPRx/IPR
    /// priority, for example BusFault -> `SHPR1.PRI_5`.
    ///
    fn get_exception_priority(&self, exception: Exception) -> i16;

    ///
    /// Clear exceptions to reset state
    ///
    fn exceptions_reset(&mut self);

    ///
    /// Check if any exceptions have happened.
    ///
    fn check_exceptions(&mut self);
}

trait ExceptionHandlingHelpers {
    fn exception_taken(&mut self, exception: Exception) -> Result<(), Fault>;
    fn deactivate(&mut self, returning_exception_number: usize);
    fn invalid_exception_return_fault(
        &mut self,
        exc_return: u32,
        popped_xpsr: Option<u32>,
    ) -> Result<(), Fault>;
    fn invalid_exception_return(
        &mut self,
        returning_exception_number: usize,
        exc_return: u32,
    ) -> Result<(), Fault>;
    fn push_stack(
        &mut self,
        return_address: u32,
        refused: Option<RefusedReturn>,
    ) -> Result<(), Fault>;
    #[cfg(feature = "has-fp")]
    fn update_fpccr(&mut self, frameptr: u32) -> Result<(), Fault>;
    fn pop_stack(&mut self, frameptr: u32, exc_return: u32) -> Result<(), Fault>;
    fn exception_active_bit_count(&self) -> usize;
    fn is_activatable(&self, exception_number: usize) -> bool;
}

/// The fields of the xPSR that PushStack stacks: N Z C V Q (31:27), IT/ICI (26:25 and 15:10), T
/// (24), GE (19:16, only with the DSP extension) and the IPSR (8:0). Bit 9, the alignment of the
/// frame, is the frame's own.
#[cfg(feature = "has-dsp-ext")]
const XPSR_IMPLEMENTED: u32 = 0xff0f_fc00 | 0x1ff;
#[cfg(not(feature = "has-dsp-ext"))]
const XPSR_IMPLEMENTED: u32 = 0xff00_fc00 | 0x1ff;

/// xPSR bits that hold ITSTATE: [26:25] and [15:10]
const IT_XPSR_MASK: u32 = 0b11 << 25 | 0b11_1111 << 10;

#[derive(PartialEq, Debug, Copy, Clone)]
///
/// List of supported Exceptions
///
/// Interrupts are controlled by NVIC, but still are generally handled like other exceptions (being sorted by priority)
///
pub enum Exception {
    /// Special exception to reset the processor
    Reset,
    /// Highest priority exception (for except the reset) that cannot be ever masked away.
    /// Can be triggered by a peripheral or triggered by software.
    NMI,
    /// Denotes a error during exception processing, or because an exception cannot be
    /// handled by any other exception handling mechanism.
    HardFault,
    /// Memory protection related fault
    MemoryManagementFault,
    /// Memory related fault (bus access error either for instructions or data)
    BusFault,
    /// Instruction execution faults for multiple underlying reasons. Example: undefined instructions.
    UsageFault,
    /// Reserved for future
    Reserved4,
    /// Reserved for future
    Reserved5,
    /// Reserved for future
    Reserved6,
    /// Debugging related exceptions
    DebugMonitor,
    /// Supervisor call exception, used typically for OS supervisor API handling.
    /// SVC instruction triggers `SVCall` exception.
    SVCall,
    /// Reserved for future
    Reserved8,
    /// Reserved for future
    Reserved9,
    /// Request for system level service, used typically for context switching in OS.
    PendSV,
    /// System timer exception, used for generating timer ticks.
    SysTick,
    /// Exception from a peripheral or software triggered interrupt
    Interrupt {
        /// Interrupt number, 0..
        n: usize,
    },
}

impl Processor {
    /// A HardFault can not be taken when the execution priority is already below 0 (HardFault or
    /// NMI running, or FAULTMASK set): the core locks up. A fault and a failed entry share this.
    pub(crate) fn is_lockup(&self, exception: Exception) -> bool {
        exception == Exception::HardFault && self.get_execution_priority() < 0
    }

    /// Enter an exception. `refused` is set by the fault of an exception return that was refused.
    pub(crate) fn enter_exception(
        &mut self,
        exception: Exception,
        return_address: u32,
        refused: Option<RefusedReturn>,
    ) -> Result<(), Fault> {
        if exception == Exception::Reset {
            self.reset()
        } else {
            if let Exception::Interrupt { n } = exception {
                self.nvic_unpend_interrupt(n);
            }
            self.push_stack(return_address, refused)
                .map_err(Fault::on_exception_entry_stack)?;
            self.exception_taken(exception)
        }
    }

    /// The exception that is running. It is the one in the IPSR when that one is active, and
    /// otherwise the active one of the highest group priority, the lowest number among equals (the
    /// manual gives the lower exception number the precedence). This is the active set, and not
    /// the IPSR alone, which a refused exception return may have been given by the stacked xPSR.
    pub(crate) fn active_exception(&self) -> Option<Exception> {
        let ipsr = self.psr.get_isr_number();
        if self.exceptions.get(&ipsr).is_some_and(|state| state.active) {
            return Some(Exception::from(ipsr));
        }

        let group = 2 << self.aircr.get_bits(8..11);
        self.exceptions
            .values()
            .filter(|state| state.active)
            .min_by_key(|state| {
                // the system exceptions with a negative priority have no group
                let priority = if state.priority < 0 {
                    state.priority
                } else {
                    state.priority - state.priority % group
                };
                (priority, state.exception_number)
            })
            .map(|state| state.exception_number.into())
    }
}

impl ExceptionHandlingHelpers for Processor {
    fn exception_taken(&mut self, exception: Exception) -> Result<(), Fault> {
        let vtor = self.vtor;
        let offset: u32 = usize::from(exception) as u32 * 4;
        let start = self
            .read32(vtor.wrapping_add(offset))
            .map_err(Fault::on_vector_read)?;
        self.blx_write_pc(start);

        self.mode = ProcessorMode::HandlerMode;
        self.psr.set_isr_number(exception.into());
        #[cfg(feature = "has-fp")]
        {
            self.control.fpca = false;
        }
        self.control.sp_sel = false;
        self.exceptions.get_mut(&exception.into()).unwrap().active = true;
        self.set_shcsr_exception_active(exception, true);
        self.execution_priority = self.get_execution_priority();

        // ClearExclusiveLocal(;)
        // SetEventRegister();
        // InstructionSynchronizationBarrier();
        Ok(())
    }

    fn deactivate(&mut self, returning_exception_number: usize) {
        let exception = Exception::from(returning_exception_number);

        self.exceptions
            .get_mut(&returning_exception_number)
            .unwrap()
            .active = false;
        self.set_shcsr_exception_active(exception, false);

        #[cfg(not(feature = "armv6m"))]
        {
            if self.psr.get_isr_number() != 0b10 {
                self.faultmask = false;
            }
        }
        self.execution_priority = self.get_execution_priority();
    }

    fn invalid_exception_return_fault(
        &mut self,
        exc_return: u32,
        popped_xpsr: Option<u32>,
    ) -> Result<(), Fault> {
        // The frame of the UsageFault keeps the LR as it is (the popped one, after a pop) and the
        // popped xPSR, and the handler's LR is the EXC_RETURN that was refused. The fault carries
        // them to the entry, where push_stack uses them.
        let refused = (0b1111 << 28) + exc_return;
        self.pending_fault_status = Some(FaultStatusContext {
            refused_return: Some(RefusedReturn {
                frame_lr: self.lr,
                handler_lr: refused,
                frame_xpsr: popped_xpsr,
            }),
            ..FaultStatusContext::default()
        });
        self.set_r(Reg::LR, refused);
        Err(Fault::InvPc)
    }

    fn invalid_exception_return(
        &mut self,
        returning_exception_number: usize,
        exc_return: u32,
    ) -> Result<(), Fault> {
        self.deactivate(returning_exception_number);
        self.invalid_exception_return_fault(exc_return, None)
    }

    fn exception_active_bit_count(&self) -> usize {
        self.exceptions
            .iter()
            .filter(|&(_, exp)| exp.active)
            .fold(0, |acc, _| acc + 1)
    }

    // Exception numbers that have a state entry: Reset (not an exception that runs), the
    // faults, SVCall, PendSV, SysTick and the interrupts. The reserved ones, and the interrupts
    // past the last, have none, so a stacked IPSR can not make one of them current.
    fn is_activatable(&self, exception_number: usize) -> bool {
        exception_number != usize::from(Exception::Reset)
            && self.exceptions.contains_key(&exception_number)
    }

    // The caller says where the exception returns to: the faulting instruction for a fault, which
    // is synchronous, and the next instruction for SVC and for an exception taken between two.
    fn push_stack(
        &mut self,
        return_address: u32,
        refused: Option<RefusedReturn>,
    ) -> Result<(), Fault> {
        #[cfg(feature = "has-fp")]
        let (forcealign, frame_size): (bool, u32) = if self.control.fpca {
            (true, 0x68)
        } else {
            (self.ccr.get_bit(CCR_STKALIGN), 0x20)
        };

        #[cfg(not(feature = "has-fp"))]
        let (forcealign, frame_size): (bool, u32) = (self.ccr.get_bit(CCR_STKALIGN), 0x20);

        // forces 8 byte alignment on the stack
        let spmask = (u32::from(forcealign) << 2) ^ 0xFFFF_FFFF;

        let (frameptr, frameptralign) =
            if self.control.sp_sel && self.mode == ProcessorMode::ThreadMode {
                let align = u32::from(self.psp.get_bit(2) & forcealign);
                self.set_psp((self.psp.wrapping_sub(frame_size)) & spmask);
                (self.psp, align)
            } else {
                let align = u32::from(self.msp.get_bit(2));
                self.set_msp((self.msp.wrapping_sub(frame_size)) & spmask);
                (self.msp, align)
            };

        let r0 = self.get_r(Reg::R0);
        let r1 = self.get_r(Reg::R1);
        let r2 = self.get_r(Reg::R2);
        let r3 = self.get_r(Reg::R3);
        let r12 = self.get_r(Reg::R12);
        let lr = refused.map_or_else(|| self.get_r(Reg::LR), |refused| refused.frame_lr);

        self.write32(frameptr, r0)?;
        self.write32(frameptr.wrapping_add(0x4), r1)?;
        self.write32(frameptr.wrapping_add(0x8), r2)?;
        self.write32(frameptr.wrapping_add(0xc), r3)?;
        self.write32(frameptr.wrapping_add(0x10), r12)?;
        self.write32(frameptr.wrapping_add(0x14), lr)?;
        self.write32(frameptr.wrapping_add(0x18), return_address)?;
        // The IT state lives outside psr.value; it is stacked in the EPSR positions
        // and the handler starts without an IT block.
        let it = u32::from(self.itstate);
        let xpsr = match refused.and_then(|refused| refused.frame_xpsr) {
            // a refused return stacks the xPSR it popped, as it is
            Some(popped) => (popped & XPSR_IMPLEMENTED) | frameptralign << 9,
            None => {
                (self.psr.value & !(IT_XPSR_MASK | 1 << 9))
                    | frameptralign << 9
                    | it.get_bits(0..2) << 25
                    | it.get_bits(2..8) << 10
            }
        };
        self.itstate = 0;
        self.write32(frameptr.wrapping_add(0x1c), xpsr)?;

        #[cfg(feature = "has-fp")]
        if self.control.fpca {
            if !self.fpccr.get_bit(FPCCR_LSPEN) {
                self.check_vfp_enabled()?;
                for i in 0..16 {
                    let reg = SingleReg::from(i as u8);
                    let value = self.get_sr(reg);
                    self.write32(frameptr.wrapping_add(0x20 + i * 4), value)?;
                }
                // write FPSCR:
                self.write32(frameptr.wrapping_add(0x60), self.fpscr)?;
            } else {
                self.update_fpccr(frameptr)?;
            }
        }

        #[cfg(feature = "has-fp")]
        {
            let fpca = u32::from(!self.control.fpca) << 4;
            if self.mode == ProcessorMode::HandlerMode {
                self.lr = 0b1111_1111_1111_1111_1111_1111_1110_0001 + fpca;
            } else {
                let thread = 1 << 3;
                let spsel = u32::from(self.control.sp_sel) << 2;
                self.lr = 0b1111_1111_1111_1111_1111_1111_1110_0001 + fpca + thread + spsel;
            }
        }
        #[cfg(not(feature = "has-fp"))]
        {
            if self.mode == ProcessorMode::HandlerMode {
                self.lr = 0xFFFF_FFF1;
            } else if self.control.sp_sel {
                self.lr = 0xFFFF_FFFD;
            } else {
                self.lr = 0xFFFF_FFF9;
            }
        }
        if let Some(refused) = refused {
            self.lr = refused.handler_lr;
        }
        Ok(())
    }

    #[cfg(feature = "has-fp")]
    fn update_fpccr(&mut self, frameptr: u32) -> Result<(), Fault> {
        if !(self.control.fpca && self.fpccr.get_bit(FPCCR_LSPEN)) {
            return Ok(());
        }

        let addr = frameptr.wrapping_add(0x20);
        self.fpcar = addr & !0x7;
        self.fpccr.set_bit(FPCCR_LSPACT, true);

        if self.current_mode_is_privileged() {
            self.fpccr.set_bit(FPCCR_USER, false);
        } else {
            self.fpccr.set_bit(FPCCR_USER, true);
        }

        if self.mode == ProcessorMode::ThreadMode {
            self.fpccr.set_bit(FPCCR_THREAD, true);
        } else {
            self.fpccr.set_bit(FPCCR_THREAD, false);
        }

        if self.execution_priority > -1 {
            self.fpccr.set_bit(FPCCR_HFRDY, true);
        } else {
            self.fpccr.set_bit(FPCCR_HFRDY, false);
        }

        if self.configurable_fault_enabled(Exception::BusFault)
            && self.execution_priority > self.get_exception_priority(Exception::BusFault)
        {
            self.fpccr.set_bit(FPCCR_BFRDY, true);
        } else {
            self.fpccr.set_bit(FPCCR_BFRDY, false);
        }

        if self.configurable_fault_enabled(Exception::MemoryManagementFault)
            && self.execution_priority
                > self.get_exception_priority(Exception::MemoryManagementFault)
        {
            self.fpccr.set_bit(FPCCR_MMRDY, true);
        } else {
            self.fpccr.set_bit(FPCCR_MMRDY, false);
        }

        if self.read_demcr().get_bit(DEMCR_MON_EN)
            && self.execution_priority > self.get_exception_priority(Exception::DebugMonitor)
        {
            self.fpccr.set_bit(FPCCR_MONRDY, true);
        } else {
            self.fpccr.set_bit(FPCCR_MONRDY, false);
        }

        Ok(())
    }

    fn pop_stack(&mut self, frameptr: u32, exc_return: u32) -> Result<(), Fault> {
        #[cfg(feature = "has-fp")]
        let (frame_size, forcealign): (u32, bool) = if !exc_return.get_bit(4) {
            (0x68, true)
        } else {
            (0x20, self.ccr.get_bit(CCR_STKALIGN))
        };

        #[cfg(not(feature = "has-fp"))]
        let (frame_size, forcealign): (u32, bool) = (0x20, self.ccr.get_bit(CCR_STKALIGN));

        let r0 = self.read32(frameptr)?;
        let r1 = self.read32(frameptr.wrapping_add(0x4))?;
        let r2 = self.read32(frameptr.wrapping_add(0x8))?;
        let r3 = self.read32(frameptr.wrapping_add(0xc))?;
        let r12 = self.read32(frameptr.wrapping_add(0x10))?;
        let lr = self.read32(frameptr.wrapping_add(0x14))?;
        let pc = self.read32(frameptr.wrapping_add(0x18))?;
        let psr = self.read32(frameptr.wrapping_add(0x1c))?;

        #[cfg(feature = "has-fp")]
        {
            if !exc_return.get_bit(4) {
                if self.fpccr.get_bit(FPCCR_LSPACT) {
                    self.fpccr.set_bit(FPCCR_LSPACT, false);
                } else {
                    self.check_vfp_enabled()?;
                    for i in 0..16 {
                        let value = self.read32(frameptr.wrapping_add(0x20 + i * 4))?;
                        let reg = SingleReg::from(i as u8);
                        self.set_sr(reg, value);
                    }
                    let value = self.read32(frameptr.wrapping_add(0x60))?;
                    self.write_fpscr(value);
                }
            }
            self.control.fpca = !exc_return.get_bit(4);
        }

        self.set_r(Reg::R0, r0);
        self.set_r(Reg::R1, r1);
        self.set_r(Reg::R2, r2);
        self.set_r(Reg::R3, r3);
        self.set_r(Reg::R12, r12);
        self.set_r(Reg::LR, lr);

        self.branch_write_pc(pc);

        let spmask = u32::from(psr.get_bit(9) && forcealign) << 2;
        match exc_return.get_bits(0..4) {
            0b0001 | 0b1001 => {
                let msp = self.get_msp();
                self.set_msp((msp.wrapping_add(frame_size)) | spmask);
            }
            // 0b1101: exception_return has refused every other EXC_RETURN
            _ => {
                let psp = self.get_psp();
                self.set_psp((psp.wrapping_add(frame_size)) | spmask);
            }
        }
        self.psr.value.set_bits(27..32, psr.get_bits(27..32));
        self.psr.value.set_bit(24, psr.get_bit(24));
        self.itstate = (psr.get_bits(25..27) | psr.get_bits(10..16) << 2) as u8;
        // GE[3:0] bits (APSR bits 19:16) are DSP-extension-only architectural state.
        // Restore them from the stacked xPSR only when the DSP extension is present.
        #[cfg(feature = "has-dsp-ext")]
        self.psr.value.set_bits(16..20, psr.get_bits(16..20));

        // What the popped xPSR says is checked after the pop, as I remember the manual: the pop is
        // done when it is found wrong. The IPSR against the mode is the check of the return itself,
        // so it comes first: zero for Thread mode, an exception the core has for Handler mode. A
        // refused IPSR is not written to the xPSR, so that it never holds an exception that does
        // not exist. EPSR.T = 0 is only INVSTATE once the IPSR has passed.
        let stacked_exception_number = psr.get_bits(0..9) as usize;
        let to_handler = exc_return.get_bits(0..4) == 0b0001;
        let ipsr_agrees = if to_handler {
            self.is_activatable(stacked_exception_number)
        } else {
            stacked_exception_number == 0
        };
        if !ipsr_agrees {
            return self.invalid_exception_return_fault(exc_return, Some(psr));
        }
        self.psr.value.set_bits(0..9, psr.get_bits(0..9));
        if !psr.get_bit(24) {
            return Err(Fault::Invstate);
        }
        Ok(())
    }
}

impl ExceptionHandling for Processor {
    fn exceptions_reset(&mut self) {
        self.set_shcsr_exception_active(Exception::MemoryManagementFault, false);
        self.set_shcsr_exception_active(Exception::BusFault, false);
        self.set_shcsr_exception_active(Exception::UsageFault, false);

        for exception in self.exceptions.values_mut() {
            exception.pending = false;
            exception.active = false;

            if exception.exception_number > Exception::HardFault.into() {
                exception.priority = 0;
            }
        }
        self.pending_exception_count = 0;
    }
    // An exception without state is one the core does not have: the reserved ones, and the
    // interrupts past the last. The registers that name them read as zero and ignore writes,
    // so there is no state to read, and nothing to write to.
    fn exception_active(&self, exception: Exception) -> bool {
        self.exceptions
            .get(&usize::from(exception))
            .is_some_and(|state| state.active)
    }

    fn exception_pending(&self, exception: Exception) -> bool {
        self.exceptions
            .get(&usize::from(exception))
            .is_some_and(|state| state.pending)
    }

    fn set_exception_priority(&mut self, exception: Exception, priority: u8) {
        if let Some(state) = self.exceptions.get_mut(&exception.into()) {
            state.priority = i16::from(priority);
        }
    }

    fn get_exception_priority(&self, exception: Exception) -> i16 {
        self.exceptions
            .get(&exception.into())
            .map_or(0, |state| state.priority)
    }

    fn get_execution_priority(&self) -> i16 {
        self.calculate_execution_priority_helper(true)
    }

    fn set_exception_pending(&mut self, exception: Exception) {
        if let Some(exp) = self.exceptions.get_mut(&exception.into())
            && !exp.pending
        {
            exp.pending = true;
            self.pending_exception_count += 1;
        }
    }

    fn get_pending_exception(&self) -> Option<Exception> {
        if self.pending_exception_count > 0 {
            let mut selected: Option<ExceptionState> = None;

            for exception in self.exceptions.values() {
                if !exception.pending || exception.priority >= self.execution_priority {
                    continue;
                }

                let replace = match selected {
                    None => true,
                    Some(current) => {
                        exception.priority < current.priority
                            || (exception.priority == current.priority
                                && exception.exception_number < current.exception_number)
                    }
                };

                if replace {
                    selected = Some(*exception);
                }
            }

            return selected.map(|exception| exception.exception_number.into());
        }
        None
    }

    fn clear_pending_exception(&mut self, exception: Exception) {
        if let Some(exp) = self.exceptions.get_mut(&exception.into())
            && exp.pending
        {
            exp.pending = false;
            self.pending_exception_count -= 1;
        }
    }

    fn exception_entry(&mut self, exception: Exception, return_address: u32) -> Result<(), Fault> {
        self.enter_exception(exception, return_address, None)
    }

    fn exception_return(&mut self, exc_return: u32) -> Result<(), Fault> {
        assert!(self.mode == ProcessorMode::HandlerMode);

        let returning_exception_number = self.psr.get_isr_number();
        let nested_activation = self.exception_active_bit_count();

        if self
            .exceptions
            .get(&returning_exception_number)
            .is_some_and(|state| state.active)
        {
            let frameptr;
            match exc_return.get_bits(0..4) {
                0b0001 => {
                    // return to handler
                    if nested_activation == 1 {
                        return self
                            .invalid_exception_return(returning_exception_number, exc_return);
                    }
                    frameptr = self.get_msp();
                    self.mode = ProcessorMode::HandlerMode;
                    self.control.sp_sel = false;
                }
                0b1001 => {
                    // returning to thread using main stack
                    if nested_activation != 1
                    /*&& !self.ccr.nonbasethreadena*/
                    {
                        return self
                            .invalid_exception_return(returning_exception_number, exc_return);
                    } else {
                        frameptr = self.get_msp();
                        self.mode = ProcessorMode::ThreadMode;
                        self.control.sp_sel = false;
                    }
                }
                0b1101 => {
                    // returning to thread using process stack
                    if nested_activation != 1
                    /*&& !self.ccr.nonbasethreadena*/
                    {
                        return self
                            .invalid_exception_return(returning_exception_number, exc_return);
                    } else {
                        frameptr = self.get_psp();
                        self.mode = ProcessorMode::ThreadMode;
                        self.control.sp_sel = true;
                    }
                }
                _ => {
                    return self.invalid_exception_return(returning_exception_number, exc_return);
                }
            }

            self.deactivate(returning_exception_number);
            self.pop_stack(frameptr, exc_return)?;

            if self.mode == ProcessorMode::ThreadMode
                && nested_activation == 1 // deactivate() reduced one
                && self.scr.get_bit(1)
            {
                self.sleeping = true;
            }

            Ok(())
        } else {
            self.invalid_exception_return(returning_exception_number, exc_return)
        }
    }

    #[inline(always)]
    fn check_exceptions(&mut self) {
        if let Some(exception) = self.get_pending_exception() {
            self.sleeping = false;
            self.clear_pending_exception(exception);
            let pc = self.get_pc();
            if let Err(fault) = self.exception_entry(exception, pc) {
                let active_exception = self.active_exception();
                self.record_fault_status(fault, crate::core::fault::FaultStatusContext::default());
                self.set_hfsr_forced();
                let trap_reason =
                    if exception == Exception::HardFault || self.is_lockup(Exception::HardFault) {
                        crate::core::fault::FaultTrapReason::Lockup
                    } else {
                        crate::core::fault::FaultTrapReason::Fault
                    };
                self.pending_fault_trap = Some(crate::core::fault::FaultContext {
                    trap_reason,
                    fault,
                    exception: Exception::HardFault,
                    pc,
                    active_exception,
                });
            }
        } else if self.sleeping && self.has_wakeup_condition() {
            self.sleeping = false;
        }
    }
}

impl From<Exception> for usize {
    fn from(value: Exception) -> Self {
        match value {
            Exception::Reset => 1,
            Exception::NMI => 2,
            Exception::HardFault => 3,
            Exception::MemoryManagementFault => 4,
            Exception::BusFault => 5,
            Exception::UsageFault => 6,
            Exception::Reserved4 => 7,
            Exception::Reserved5 => 8,
            Exception::Reserved6 => 9,
            Exception::DebugMonitor => 12,
            Exception::SVCall => 11,
            Exception::Reserved8 => 10,
            Exception::Reserved9 => 13,
            Exception::PendSV => 14,
            Exception::SysTick => 15,
            Exception::Interrupt { n } => 16 + n,
        }
    }
}

impl From<usize> for Exception {
    fn from(value: usize) -> Self {
        match value {
            1 => Self::Reset,
            2 => Self::NMI,
            3 => Self::HardFault,
            4 => Self::MemoryManagementFault,
            5 => Self::BusFault,
            6 => Self::UsageFault,
            7 => Self::Reserved4,
            8 => Self::Reserved5,
            9 => Self::Reserved6,
            12 => Self::DebugMonitor,
            11 => Self::SVCall,
            10 => Self::Reserved8,
            13 => Self::Reserved9,
            14 => Self::PendSV,
            15 => Self::SysTick,
            _ => Self::Interrupt { n: value - 16 },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::Bus;
    use crate::core::exception::Exception;
    use crate::core::exception::ExceptionHandling;
    #[cfg(not(feature = "armv6m"))]
    use crate::core::fault::Fault;
    #[cfg(not(feature = "armv6m"))]
    use crate::core::fault::FaultTrapReason;
    #[cfg(not(feature = "armv6m"))]
    use crate::core::instruction::Instruction;
    #[cfg(feature = "has-dsp-ext")]
    use crate::core::register::Apsr;
    #[cfg(feature = "has-fp")]
    use crate::core::register::{ExtensionRegOperations, SingleReg};
    #[cfg(not(feature = "armv6m"))]
    use crate::decoder::decode_16;
    #[cfg(not(feature = "armv6m"))]
    use crate::executor::Executor;
    use crate::peripheral::nvic::NVIC;
    #[cfg(feature = "has-fp")]
    use crate::peripheral::scb::{
        DEMCR_MON_EN, FPCCR_ASPEN, FPCCR_BFRDY, FPCCR_HFRDY, FPCCR_LSPACT, FPCCR_LSPEN,
        FPCCR_MMRDY, FPCCR_MONRDY, FPCCR_THREAD, FPCCR_USER, SHCSR_BUSFAULTENA, SHCSR_MEMFAULTENA,
    };

    #[cfg(not(feature = "armv6m"))]
    const CFSR_MSTKERR: u32 = 1 << 4;
    #[cfg(not(feature = "armv6m"))]
    const HFSR_FORCED: u32 = 1 << 30;
    #[cfg(all(not(feature = "armv6m"), feature = "has-fp"))]
    const EXC_RETURN_HANDLER: u32 = 0x11;
    #[cfg(all(not(feature = "armv6m"), not(feature = "has-fp")))]
    const EXC_RETURN_HANDLER: u32 = 0x1;
    #[cfg(all(not(feature = "armv6m"), feature = "has-fp"))]
    const EXC_RETURN_THREAD_MSP: u32 = 0x19;
    #[cfg(all(not(feature = "armv6m"), not(feature = "has-fp")))]
    const EXC_RETURN_THREAD_MSP: u32 = 0x9;

    #[cfg(not(feature = "armv6m"))]
    fn set_active_exception(processor: &mut Processor, exception: Exception) {
        processor.test_set_active_exception(exception);
    }

    #[cfg(feature = "has-fp")]
    fn fp_test_processor() -> Processor {
        let mut core = Processor::new();
        core.cpacr = 0x00f0_0000;
        core
    }

    #[cfg(feature = "has-fp")]
    fn seed_low_fp_registers(core: &mut Processor, base: u32) {
        for i in 0..16 {
            core.set_sr(SingleReg::from(i as u8), base + i);
        }
    }

    #[cfg(feature = "has-fp")]
    fn configure_update_fpccr_ready_context(core: &mut Processor) {
        core.shcsr = SHCSR_BUSFAULTENA | SHCSR_MEMFAULTENA;
        core.demcr = 1 << DEMCR_MON_EN;
        core.set_exception_priority(Exception::BusFault, 5);
        core.set_exception_priority(Exception::MemoryManagementFault, 4);
        core.set_exception_priority(Exception::DebugMonitor, 3);
    }

    #[cfg(feature = "has-fp")]
    fn write_basic_exception_frame(core: &mut Processor, frameptr: u32) {
        core.write32(frameptr, 1).unwrap();
        core.write32(frameptr.wrapping_add(0x4), 2).unwrap();
        core.write32(frameptr.wrapping_add(0x8), 3).unwrap();
        core.write32(frameptr.wrapping_add(0xc), 4).unwrap();
        core.write32(frameptr.wrapping_add(0x10), 12).unwrap();
        core.write32(frameptr.wrapping_add(0x14), 0x0800_00f1)
            .unwrap();
        core.write32(frameptr.wrapping_add(0x18), 0x0800_0001)
            .unwrap();
        core.write32(frameptr.wrapping_add(0x1c), 1 << 24).unwrap();
    }

    #[test]
    fn test_push_stack() {
        const STACK_START: u32 = 0x2000_0100;
        let mut core = Processor::new();

        // arrange
        let lr = {
            //    if self.control.sp_sel && self.mode == ProcessorMode::ThreadMode {
            core.control.sp_sel = false;
            //core.mode = ProcessorMode::ThreadMode;
            core.set_r(Reg::R0, 42);
            core.set_r(Reg::R1, 43);
            core.set_r(Reg::R2, 44);
            core.set_r(Reg::R3, 45);
            core.set_r(Reg::R12, 46);
            core.set_r(Reg::LR, 47);
            core.set_psp(0);
            core.set_msp(STACK_START);
            core.psr.value = 0xffff_ffff;

            // act
            core.push_stack(99, None).unwrap();

            assert_eq!(core.msp, STACK_START - 32);
            core.get_r(Reg::LR)
        };

        // values pushed on to stack
        assert_eq!(core.read32(STACK_START - 0x20).unwrap(), 42);
        assert_eq!(core.read32(STACK_START - 0x20 + 4).unwrap(), 43);
        assert_eq!(core.read32(STACK_START - 0x20 + 8).unwrap(), 44);
        assert_eq!(core.read32(STACK_START - 0x20 + 12).unwrap(), 45);
        assert_eq!(core.read32(STACK_START - 0x20 + 16).unwrap(), 46);
        assert_eq!(core.read32(STACK_START - 0x20 + 20).unwrap(), 47);
        assert_eq!(core.read32(STACK_START - 0x20 + 24).unwrap(), 99);
        assert_eq!(
            core.read32(STACK_START - 0x20 + 28).unwrap(),
            0b1111_1001_1111_1111_0000_0001_1111_1111
        );
        assert_eq!(lr, 0xffff_fff9);
    }

    #[test]
    #[cfg(feature = "has-fp")]
    fn test_push_stack_with_active_fp_context_uses_extended_frame_and_fp_exc_return() {
        const STACK_START: u32 = 0x2000_0100;
        let mut core = fp_test_processor();

        core.control.fpca = true;
        core.fpccr = 0;
        core.control.sp_sel = true;
        core.mode = ProcessorMode::ThreadMode;
        core.set_r(Reg::R0, 42);
        core.set_r(Reg::R1, 43);
        core.set_r(Reg::R2, 44);
        core.set_r(Reg::R3, 45);
        core.set_r(Reg::R12, 46);
        core.set_r(Reg::LR, 47);
        core.set_psp(STACK_START);
        core.set_msp(0);
        core.fpscr = 0xa3c0_0014;
        core.psr.value = 0xffff_ffff;

        seed_low_fp_registers(&mut core, 0x1111_0000);

        core.push_stack(99, None).unwrap();

        assert_eq!(core.psp, STACK_START - 0x68);
        assert_eq!(core.read32(STACK_START - 0x68).unwrap(), 42);
        assert_eq!(core.read32(STACK_START - 0x68 + 4).unwrap(), 43);
        assert_eq!(core.read32(STACK_START - 0x68 + 8).unwrap(), 44);
        assert_eq!(core.read32(STACK_START - 0x68 + 12).unwrap(), 45);
        assert_eq!(core.read32(STACK_START - 0x68 + 16).unwrap(), 46);
        assert_eq!(core.read32(STACK_START - 0x68 + 20).unwrap(), 47);
        assert_eq!(core.read32(STACK_START - 0x68 + 24).unwrap(), 99);
        assert_eq!(
            core.read32(STACK_START - 0x68 + 28).unwrap(),
            0b1111_1001_1111_1111_0000_0001_1111_1111
        );

        for i in 0..16 {
            assert_eq!(
                core.read32(STACK_START - 0x68 + 0x20 + (i * 4)).unwrap(),
                0x1111_0000 + i
            );
        }

        assert_eq!(core.read32(STACK_START - 0x68 + 0x60).unwrap(), 0xa3c0_0014);
        assert_eq!(core.get_r(Reg::LR), 0xffff_ffed);
    }

    #[test]
    #[cfg(feature = "has-fp")]
    fn test_push_stack_with_lazy_fp_state_sets_lspact_and_fpcar() {
        const STACK_START: u32 = 0x2000_0100;
        let mut core = fp_test_processor();

        core.control.fpca = true;
        core.mode = ProcessorMode::ThreadMode;
        core.set_msp(STACK_START);
        core.fpccr = 1 << FPCCR_LSPEN;

        core.push_stack(99, None).unwrap();

        assert_eq!(core.msp, STACK_START - 0x68);
        assert!(core.fpccr.get_bit(FPCCR_LSPACT));
        assert_eq!(core.fpcar, STACK_START - 0x68 + 0x20);
        assert_eq!(core.get_r(Reg::LR), 0xffff_ffe9);
    }

    #[test]
    #[cfg(feature = "has-fp")]
    fn test_update_fpccr_sets_ready_bits_from_fault_enables_and_demcr() {
        let mut core = fp_test_processor();

        core.control.fpca = true;
        core.mode = ProcessorMode::ThreadMode;
        core.execution_priority = 10;
        core.control.n_priv = true;
        core.fpccr = (1 << FPCCR_ASPEN) | (1 << FPCCR_LSPEN);
        configure_update_fpccr_ready_context(&mut core);

        core.update_fpccr(0x2000_0080).unwrap();

        assert_eq!(core.fpcar, 0x2000_00a0);
        assert!(core.fpccr.get_bit(FPCCR_LSPACT));
        assert!(core.fpccr.get_bit(FPCCR_USER));
        assert!(core.fpccr.get_bit(FPCCR_THREAD));
        assert!(core.fpccr.get_bit(FPCCR_HFRDY));
        assert!(core.fpccr.get_bit(FPCCR_BFRDY));
        assert!(core.fpccr.get_bit(FPCCR_MMRDY));
        assert!(core.fpccr.get_bit(FPCCR_MONRDY));
    }

    #[test]
    #[cfg(feature = "has-fp")]
    fn test_update_fpccr_clears_ready_bits_when_fault_handlers_not_ready() {
        let mut core = fp_test_processor();

        core.control.fpca = true;
        core.mode = ProcessorMode::HandlerMode;
        core.execution_priority = -1;
        core.control.n_priv = false;
        core.fpccr = (1 << FPCCR_ASPEN) | (1 << FPCCR_LSPEN);
        configure_update_fpccr_ready_context(&mut core);

        core.update_fpccr(0x2000_0080).unwrap();

        assert!(!core.fpccr.get_bit(FPCCR_USER));
        assert!(!core.fpccr.get_bit(FPCCR_THREAD));
        assert!(!core.fpccr.get_bit(FPCCR_HFRDY));
        assert!(!core.fpccr.get_bit(FPCCR_BFRDY));
        assert!(!core.fpccr.get_bit(FPCCR_MMRDY));
        assert!(!core.fpccr.get_bit(FPCCR_MONRDY));
    }

    #[test]
    #[cfg(feature = "has-fp")]
    fn test_update_fpccr_leaves_state_unmodified_when_fpca_clear() {
        let mut core = fp_test_processor();

        core.control.fpca = false;
        core.mode = ProcessorMode::ThreadMode;
        core.execution_priority = 10;
        core.control.n_priv = true;
        core.fpccr = 1 << FPCCR_LSPEN;
        core.fpcar = 0x2000_0120;
        configure_update_fpccr_ready_context(&mut core);

        core.update_fpccr(0x2000_0080).unwrap();

        assert_eq!(core.fpccr, 1 << FPCCR_LSPEN);
        assert_eq!(core.fpcar, 0x2000_0120);
    }

    #[test]
    #[cfg(feature = "has-fp")]
    fn test_update_fpccr_leaves_state_unmodified_when_lspen_clear() {
        let mut core = fp_test_processor();

        core.control.fpca = true;
        core.mode = ProcessorMode::ThreadMode;
        core.execution_priority = 10;
        core.control.n_priv = true;
        core.fpccr = 0;
        core.fpcar = 0x2000_0140;
        configure_update_fpccr_ready_context(&mut core);

        core.update_fpccr(0x2000_0080).unwrap();

        assert_eq!(core.fpccr, 0);
        assert_eq!(core.fpcar, 0x2000_0140);
    }

    #[test]
    #[cfg(feature = "has-fp")]
    fn test_pop_stack_with_extended_fp_frame_restores_fp_state() {
        const FRAMEPTR: u32 = 0x2000_0180;
        let mut core = fp_test_processor();

        core.set_msp(FRAMEPTR);
        write_basic_exception_frame(&mut core, FRAMEPTR);

        for i in 0..16 {
            core.write32(FRAMEPTR.wrapping_add(0x20 + i * 4), 0x2222_0000 + i)
                .unwrap();
        }
        core.write32(FRAMEPTR.wrapping_add(0x60), 0xabcd_1234)
            .unwrap();

        core.pop_stack(FRAMEPTR, 0xffff_ffe9).unwrap();

        for i in 0..16 {
            assert_eq!(core.get_sr(SingleReg::from(i as u8)), 0x2222_0000 + i);
        }
        // The trap enables (bits 8-12 and 15) and the other reserved bits do not come back.
        assert_eq!(core.fpscr, 0xa3c0_0014);
        assert!(core.control.fpca);
        assert_eq!(core.get_msp(), FRAMEPTR + 0x68);
    }

    #[test]
    #[cfg(feature = "has-fp")]
    fn test_pop_stack_with_lazy_fp_state_only_clears_lspact() {
        const FRAMEPTR: u32 = 0x2000_0200;
        let mut core = fp_test_processor();

        core.fpccr = 1 << FPCCR_LSPACT;
        core.fpscr = 0x1111_2222;
        core.set_msp(FRAMEPTR);
        core.write32(FRAMEPTR.wrapping_add(0x18), 0x0800_0001)
            .unwrap();
        core.write32(FRAMEPTR.wrapping_add(0x1c), 1 << 24).unwrap();

        core.pop_stack(FRAMEPTR, 0xffff_ffe9).unwrap();

        assert!(!core.fpccr.get_bit(FPCCR_LSPACT));
        assert_eq!(core.fpscr, 0x1111_2222);
        assert!(core.control.fpca);
        assert_eq!(core.get_msp(), FRAMEPTR + 0x68);
    }

    #[test]
    #[cfg(feature = "has-fp")]
    fn test_pop_stack_with_extended_fp_frame_updates_psp_for_thread_return() {
        const FRAMEPTR: u32 = 0x2000_0280;
        let mut core = fp_test_processor();

        core.fpccr = 1 << FPCCR_LSPACT;
        core.set_psp(FRAMEPTR);
        core.write32(FRAMEPTR.wrapping_add(0x18), 0x0800_0001)
            .unwrap();
        core.write32(FRAMEPTR.wrapping_add(0x1c), 1 << 24).unwrap();

        core.pop_stack(FRAMEPTR, 0xffff_ffed).unwrap();

        assert_eq!(core.get_psp(), FRAMEPTR + 0x68);
        assert!(core.control.fpca);
    }

    #[test]
    fn test_exception_taken() {
        // Arrange
        let mut core = Processor::new();

        core.control.sp_sel = true;
        core.mode = ProcessorMode::ThreadMode;
        core.psr.value = 0xffff_ffff;

        // Act
        core.exception_taken(Exception::BusFault).unwrap();

        // Assert
        assert!(!core.control.sp_sel);
        assert_eq!(core.mode, ProcessorMode::HandlerMode);
        assert_eq!(core.psr.get_isr_number(), Exception::BusFault.into());
        assert!(core.exception_active(Exception::BusFault));
    }

    #[test]
    fn test_get_execution_priority() {
        let mut p = Processor::new();

        p.reset().unwrap();
        p.msp = 0x2000_0400;
        p.set_exception_pending(Exception::HardFault);
        p.check_exceptions();

        assert_eq!(p.get_execution_priority(), -1);

        p.reset().unwrap();
        p.msp = 0x2000_0400;
        p.set_exception_pending(Exception::NMI);
        p.check_exceptions();
        assert_eq!(p.get_execution_priority(), -2);
    }

    #[test]
    fn test_exception_priority() {
        // Arrange
        let mut processor = Processor::new();

        processor.reset().unwrap();

        // Act
        processor.set_exception_pending(Exception::Reset);
        processor.set_exception_pending(Exception::NMI);
        processor.set_exception_pending(Exception::HardFault);

        // Assert
        assert_eq!(processor.get_pending_exception(), Some(Exception::Reset));
    }

    #[test]
    fn test_exception_priority_same_priority_setting() {
        // Arrange
        let mut processor = Processor::new();

        processor.reset().unwrap();

        // Act
        processor.set_exception_pending(Exception::MemoryManagementFault);
        processor.set_exception_pending(Exception::UsageFault);
        processor.set_exception_pending(Exception::BusFault);

        // Assert (exception number should define the priority for same priority setting)
        assert_eq!(
            processor.get_pending_exception(),
            Some(Exception::MemoryManagementFault)
        );
    }

    #[cfg(not(feature = "armv6m"))]
    #[test]
    fn test_faultmask_priority() {
        // Arrange
        let mut processor = Processor::new();

        processor.reset().unwrap();

        // Act
        processor.set_exception_pending(Exception::HardFault);

        processor.execute(
            &Instruction::CPS {
                im: true,
                affect_pri: false,
                affect_fault: true,
            },
            2,
        );

        // Assert
        assert_eq!(processor.get_pending_exception(), None);
    }

    #[test]
    fn test_exception_entry_clears_nvic() {
        // Arrange
        let mut processor = Processor::new();

        // Arrange
        processor.reset().unwrap();
        processor.nvic_write_iser(0, 1);
        processor.nvic_write_ispr(0, 1);
        assert_eq!(processor.nvic_read_ispr(0), 1);

        // Act
        let _ = processor.exception_entry(Exception::Interrupt { n: 0 }, 0);

        // Assert
        assert_eq!(processor.nvic_read_ispr(0), 0);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_check_exceptions_escalates_entry_stack_fault_to_hardfault() {
        let mut processor = Processor::new();

        processor.reset().unwrap();
        processor.set_msp(0);
        processor.set_exception_pending(Exception::SysTick);

        processor.check_exceptions();

        let trap = processor
            .take_pending_fault_trap()
            .expect("fault trap expected");
        assert_eq!(trap.fault, Fault::Mstkerr);
        assert_eq!(trap.exception, Exception::HardFault);
        assert_eq!(processor.cfsr, CFSR_MSTKERR);
        assert_eq!(processor.hfsr, HFSR_FORCED);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_exception_return_rejects_invalid_exc_return_encoding() {
        let mut processor = Processor::new();

        set_active_exception(&mut processor, Exception::SysTick);

        let result = processor.exception_return(0x0);

        assert_eq!(result, Err(Fault::InvPc));
        assert_eq!(processor.get_r(Reg::LR), 0xF000_0000);
        assert!(!processor.exception_active(Exception::SysTick));
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_exception_return_rejects_handler_return_without_nested_activation() {
        let mut processor = Processor::new();

        set_active_exception(&mut processor, Exception::SysTick);

        let result = processor.exception_return(EXC_RETURN_HANDLER);

        assert_eq!(result, Err(Fault::InvPc));
        assert_eq!(processor.get_r(Reg::LR), 0xF000_0000 + EXC_RETURN_HANDLER);
        assert!(!processor.exception_active(Exception::SysTick));
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_exception_return_rejects_thread_return_with_other_active_exception() {
        const FRAMEPTR: u32 = 0x2000_00e0;

        let mut processor = Processor::new();
        processor.set_msp(FRAMEPTR);
        processor
            .write32(FRAMEPTR.wrapping_add(0x18), 0x0800_0001)
            .unwrap();
        processor
            .write32(FRAMEPTR.wrapping_add(0x1c), 1 << 24)
            .unwrap();

        set_active_exception(&mut processor, Exception::HardFault);
        processor
            .exceptions
            .get_mut(&usize::from(Exception::SysTick))
            .unwrap()
            .active = true;

        let result = processor.exception_return(EXC_RETURN_THREAD_MSP);

        assert_eq!(result, Err(Fault::InvPc));
        assert_eq!(
            processor.get_r(Reg::LR),
            0xF000_0000 + EXC_RETURN_THREAD_MSP
        );
        assert!(!processor.exception_active(Exception::HardFault));
        assert!(processor.exception_active(Exception::SysTick));
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_exception_return_rejects_handler_return_with_zero_stacked_ipsr() {
        const FRAMEPTR: u32 = 0x2000_00e0;

        let mut processor = Processor::new();
        processor.set_msp(FRAMEPTR);
        processor
            .write32(FRAMEPTR.wrapping_add(0x18), 0x0800_0001)
            .unwrap();
        processor
            .write32(FRAMEPTR.wrapping_add(0x1c), 1 << 24)
            .unwrap();

        set_active_exception(&mut processor, Exception::HardFault);
        processor
            .exceptions
            .get_mut(&usize::from(Exception::SysTick))
            .unwrap()
            .active = true;

        let result = processor.exception_return(EXC_RETURN_HANDLER);

        assert_eq!(result, Err(Fault::InvPc));
        assert_eq!(processor.get_r(Reg::LR), 0xF000_0000 + EXC_RETURN_HANDLER);
        assert!(!processor.exception_active(Exception::HardFault));
        assert!(processor.exception_active(Exception::SysTick));
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_exception_return_rejects_handler_return_to_an_exception_that_does_not_exist() {
        const FRAMEPTR: u32 = 0x2000_00e0;

        // Reset cannot be active, 7-10 and 13 are reserved, and the last interrupt is 255.
        for stacked_ipsr in [1_u32, 7, 8, 9, 10, 13, 256, 300, 511] {
            let mut processor = Processor::new();
            processor.set_msp(FRAMEPTR);
            processor
                .write32(FRAMEPTR.wrapping_add(0x18), 0x0800_0001)
                .unwrap();
            processor
                .write32(FRAMEPTR.wrapping_add(0x1c), (1 << 24) | stacked_ipsr)
                .unwrap();

            set_active_exception(&mut processor, Exception::HardFault);
            processor
                .exceptions
                .get_mut(&usize::from(Exception::SysTick))
                .unwrap()
                .active = true;

            let result = processor.exception_return(EXC_RETURN_HANDLER);

            assert_eq!(result, Err(Fault::InvPc), "stacked IPSR {stacked_ipsr}");
            // the frame was popped before the check, so the processor has what it held
            assert_eq!(
                processor.get_pc(),
                0x0800_0000,
                "stacked IPSR {stacked_ipsr}"
            );
            assert_eq!(processor.get_msp(), FRAMEPTR + 0x20);
            assert_eq!(processor.get_r(Reg::LR), 0xF000_0000 + EXC_RETURN_HANDLER);
        }
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_exception_return_accepts_handler_return_to_an_interrupt() {
        const FRAMEPTR: u32 = 0x2000_00e0;
        let interrupt = Exception::Interrupt { n: 239 };

        let mut processor = Processor::new();
        processor.set_msp(FRAMEPTR);
        processor
            .write32(FRAMEPTR.wrapping_add(0x18), 0x0800_0001)
            .unwrap();
        processor
            .write32(
                FRAMEPTR.wrapping_add(0x1c),
                (1 << 24) | usize::from(interrupt) as u32,
            )
            .unwrap();

        set_active_exception(&mut processor, Exception::HardFault);
        processor
            .exceptions
            .get_mut(&usize::from(interrupt))
            .unwrap()
            .active = true;

        assert_eq!(processor.exception_return(EXC_RETURN_HANDLER), Ok(()));
        assert_eq!(processor.psr.get_isr_number(), usize::from(interrupt));
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_exception_entry_with_the_vector_table_at_the_top_of_memory() {
        let mut processor = Processor::new();
        processor.set_msp(0x2000_0100);
        // VTOR takes any value, and the vector of SVCall is then past the end of the address
        // space, so it is read from the bottom of it (the flash).
        processor.write32(0xE000_ED08, 0xffff_fff8).unwrap();

        assert_eq!(processor.exception_entry(Exception::SVCall, 0), Ok(()));
    }

    // The stacked PC of a fault found by an instruction is that instruction (ExceptionTaken
    // stacks the address of the faulting instruction for a synchronous fault).
    #[cfg(not(feature = "armv6m"))]
    fn stacked_pc_after(processor: &mut Processor) -> u32 {
        let frame = processor.get_msp();
        processor.read32(frame.wrapping_add(0x18)).unwrap()
    }

    // SVC from Thread mode, then the handler corrupts the stacked xPSR (Thumb bit set, IPSR as
    // given) and returns with BX LR. UsageFault is disabled, so the refused return is a HardFault.
    #[cfg(not(feature = "armv6m"))]
    fn svc_then_corrupt_the_frame(stacked_ipsr: u32) -> Processor {
        let mut processor = Processor::new();
        processor.set_msp(0x2000_0200);
        processor.set_pc(0x100);
        processor.set_r(Reg::LR, 0x1234);
        processor.execute(&decode_16(0xdf00), 2); // svc #0
        assert_eq!(processor.psr.get_isr_number(), 11);

        let frame = processor.get_msp();
        processor
            .write32(frame + 0x1c, (1 << 24) | stacked_ipsr)
            .unwrap();
        processor.set_r(Reg::LR, 0xffff_fff9);
        processor.execute(&decode_16(0x4770), 2); // bx lr
        processor
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_a_refused_return_is_a_hardfault_and_not_a_lockup_whatever_ipsr_it_stacked() {
        // 3 and 2 are HardFault and NMI, which are not active, and 12 is DebugMonitor
        for stacked_ipsr in [3, 2, 12] {
            let mut processor = svc_then_corrupt_the_frame(stacked_ipsr);

            assert!(processor.running, "IPSR {stacked_ipsr}");
            let trap = processor.take_pending_fault_trap().expect("a trap");
            assert_eq!(
                trap.trap_reason,
                FaultTrapReason::Fault,
                "IPSR {stacked_ipsr}"
            );
            assert_eq!(trap.fault, Fault::InvPc);
            assert_eq!(trap.active_exception, None, "IPSR {stacked_ipsr}");
            // HardFault is the one running
            assert_eq!(processor.psr.get_isr_number(), 3, "IPSR {stacked_ipsr}");
        }
        // with a zero IPSR the return is valid
        let mut processor = svc_then_corrupt_the_frame(0);
        assert!(processor.running);
        assert!(processor.take_pending_fault_trap().is_none());
        assert_eq!(processor.psr.get_isr_number(), 0);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_a_refused_return_does_not_leave_the_ipsr_of_an_exception_that_does_not_exist() {
        const FRAMEPTR: u32 = 0x2000_00e0;

        // return to Handler mode with IPSR 300, and then a second return from there
        let mut processor = Processor::new();
        processor.set_msp(FRAMEPTR);
        processor.write32(FRAMEPTR + 0x18, 0x0800_0001).unwrap();
        processor.write32(FRAMEPTR + 0x1c, (1 << 24) | 300).unwrap();
        set_active_exception(&mut processor, Exception::HardFault);
        processor
            .exceptions
            .get_mut(&usize::from(Exception::SysTick))
            .unwrap()
            .active = true;

        assert_eq!(
            processor.exception_return(EXC_RETURN_HANDLER),
            Err(Fault::InvPc)
        );
        assert!(processor.is_activatable(processor.psr.get_isr_number()));
        // a return from what is left does not panic either
        let _ = processor.exception_return(EXC_RETURN_HANDLER);
    }

    // The IPSR against the mode is the check of the return itself; EPSR.T = 0 only faults when
    // the next instruction would execute. With both wrong, it is INVPC.
    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_ipsr_that_does_not_fit_the_mode_wins_over_the_thumb_bit() {
        const FRAMEPTR: u32 = 0x2000_00e0;

        let mut processor = Processor::new();
        processor.set_msp(FRAMEPTR);
        processor.write32(FRAMEPTR + 0x18, 0x0800_0001).unwrap();
        processor.write32(FRAMEPTR + 0x1c, 5).unwrap(); // T clear, IPSR 5, back to Thread
        set_active_exception(&mut processor, Exception::SysTick);

        assert_eq!(
            processor.exception_return(EXC_RETURN_THREAD_MSP),
            Err(Fault::InvPc)
        );
    }

    // A refused return pushes the popped state, and the handler's LR is the EXC_RETURN that was
    // refused. The frame keeps the LR that was popped.
    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_the_frame_of_a_refused_return_keeps_the_popped_lr() {
        let mut processor = Processor::new();
        processor.set_msp(0x2000_0200);
        processor.set_pc(0x100);
        processor.set_r(Reg::LR, 0x7777);
        processor.execute(&decode_16(0xdf00), 2); // svc #0

        let frame = processor.get_msp();
        processor.write32(frame + 0x14, 0x1234).unwrap(); // the LR to be popped
        processor.write32(frame + 0x1c, (1 << 24) | 3).unwrap(); // IPSR 3 for Thread mode
        processor.set_r(Reg::LR, 0xffff_fff9);
        processor.execute(&decode_16(0x4770), 2);

        let new_frame = processor.get_msp();
        assert_eq!(processor.read32(new_frame + 0x14), Ok(0x1234));
        assert_eq!(processor.get_r(Reg::LR), 0xffff_fff9);
    }

    // A refused return that ends in a lockup, then a reset, must leave nothing for the next
    // exception entry: it stacks the LR that is there and enters with its own EXC_RETURN.
    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_a_lockup_and_a_reset_leave_nothing_of_a_refused_return() {
        let mut processor = Processor::new();
        processor.set_msp(0x2000_0200);
        let frame = processor.get_msp();
        processor.write32(frame + 0x14, 0xcdcd_cdcd).unwrap();
        processor.write32(frame + 0x1c, (1 << 24) | 300).unwrap();
        // HardFault and NMI are active, the NMI is running, and UsageFault is disabled
        set_active_exception(&mut processor, Exception::HardFault);
        set_active_exception(&mut processor, Exception::NMI);
        processor.set_r(Reg::LR, 0xffff_fff1);

        processor.execute(&decode_16(0x4770), 2); // bx lr: refused, and HardFault is a lockup

        assert!(!processor.running);
        assert_eq!(processor.reset(), Ok(()));
        processor.set_msp(0x2000_0200);
        processor.set_pc(0x100);
        processor.set_r(Reg::LR, 0x5555);
        processor.execute(&decode_16(0xdf00), 2); // svc #0

        let new_frame = processor.get_msp();
        assert_eq!(processor.read32(new_frame + 0x14), Ok(0x5555));
        assert_eq!(processor.get_r(Reg::LR), 0xffff_fff9);
    }

    // The frame of a refused return holds the popped xPSR as it is, bad IPSR and all, and that
    // IPSR is never the current one: handing the same EXC_RETURN back is refused again.
    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_the_frame_of_a_refused_return_holds_the_popped_xpsr_and_a_second_return_is_refused() {
        let mut processor = Processor::new();
        processor.set_msp(0x2000_0200);
        processor.set_pc(0x100);
        processor.execute(&decode_16(0xdf00), 2); // svc #0

        let frame = processor.get_msp();
        let popped_xpsr = 0x8100_0000 | 3; // N, Thumb, and the IPSR of HardFault, for Thread mode
        processor.write32(frame + 0x1c, popped_xpsr).unwrap();
        processor.set_r(Reg::LR, 0xffff_fff9);
        processor.execute(&decode_16(0x4770), 2); // bx lr: refused
        assert_eq!(processor.get_r(Reg::LR), 0xffff_fff9);

        let new_frame = processor.get_msp();
        assert_eq!(
            processor.read32(new_frame + 0x1c).unwrap() & 0xf1ff_01ff,
            popped_xpsr
        );
        let entered = processor.psr.get_isr_number();
        assert!(entered == 3 || entered == 6, "a fault handler is running");

        // the handler returns with what it was given: the same frame is popped, and refused again
        processor.execute(&decode_16(0x4770), 2);
        assert_eq!(processor.get_r(Reg::LR), 0xffff_fff9);
        assert_eq!(processor.cfsr & (1 << 18), 1 << 18, "INVPC");
        assert!(processor.running);
        let trap = processor.take_pending_fault_trap().expect("a trap");
        assert_eq!(trap.fault, Fault::InvPc);
    }

    // A synchronous fault is escalated to HardFault when it is disabled or when its priority is
    // not higher than the execution priority, which includes PRIMASK, BASEPRI and FAULTMASK as
    // the manual's ExecutionPriority() has them (as I remember it). A HardFault at an execution
    // priority below 0 cannot be taken: it is a lockup.
    #[cfg(not(feature = "armv6m"))]
    fn run_udf_with_usage_fault_enabled(processor: &mut Processor) {
        processor.set_msp(0x2000_0200);
        processor.set_pc(0x100);
        processor.write32(0xE000_ED24, 1 << 18).unwrap();
        processor.execute(&decode_16(0xde00), 2);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_a_fault_while_hardfault_is_active_is_a_lockup_and_not_a_nested_usage_fault() {
        let mut processor = Processor::new();
        set_active_exception(&mut processor, Exception::HardFault);

        run_udf_with_usage_fault_enabled(&mut processor);

        assert!(!processor.running);
        let trap = processor.take_pending_fault_trap().expect("a trap");
        assert_eq!(trap.trap_reason, FaultTrapReason::Lockup);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_a_fault_of_the_same_priority_as_the_running_exception_goes_to_hardfault() {
        let mut processor = Processor::new();
        // SysTick runs at priority 0, and so does UsageFault: it cannot preempt
        set_active_exception(&mut processor, Exception::SysTick);

        run_udf_with_usage_fault_enabled(&mut processor);

        assert!(processor.running);
        assert_eq!(processor.psr.get_isr_number(), 3);
        assert_eq!(processor.hfsr & (1 << 30), 1 << 30, "FORCED");

        // at a higher priority number, which is less urgent, UsageFault does preempt
        let mut processor = Processor::new();
        processor.set_exception_priority(Exception::UsageFault, 0);
        processor.set_exception_priority(Exception::SysTick, 0x40);
        set_active_exception(&mut processor, Exception::SysTick);
        run_udf_with_usage_fault_enabled(&mut processor);
        assert_eq!(processor.psr.get_isr_number(), 6);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_a_fault_with_faultmask_set_is_a_lockup_and_with_primask_goes_to_hardfault() {
        let mut processor = Processor::new();
        processor.faultmask = true;
        run_udf_with_usage_fault_enabled(&mut processor);
        assert!(!processor.running);
        assert_eq!(
            processor
                .take_pending_fault_trap()
                .map(|trap| trap.trap_reason),
            Some(FaultTrapReason::Lockup)
        );

        let mut processor = Processor::new();
        processor.primask = true;
        run_udf_with_usage_fault_enabled(&mut processor);
        assert!(processor.running);
        assert_eq!(processor.psr.get_isr_number(), 3);
    }

    // The exception that is reported as running is the one in the IPSR when that one is active,
    // and otherwise the active one of the highest group priority, the lower number among equals.
    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_the_running_exception_is_the_one_in_the_ipsr_then_the_highest_group_priority() {
        let mut processor = Processor::new();
        // PRIGROUP 7: every priority is in one group, so only the numbers tell them apart
        processor.write32(0xE000_ED0C, 0x05FA_0700).unwrap();
        processor.set_exception_priority(Exception::PendSV, 0x20);
        processor.set_exception_priority(Exception::SysTick, 0x10);
        set_active_exception(&mut processor, Exception::PendSV);
        set_active_exception(&mut processor, Exception::SysTick); // IPSR is SysTick now

        assert_eq!(processor.active_exception(), Some(Exception::SysTick));

        // with nothing in the IPSR, the group priority is the same and PendSV has the lower number
        processor.psr.set_isr_number(0);
        assert_eq!(processor.active_exception(), Some(Exception::PendSV));
    }

    // SVC is taken as SVCall only when SVCall has a higher priority than the execution priority.
    // Otherwise it is escalated to HardFault (FORCED), or locks up below priority 0.
    #[cfg(not(feature = "armv6m"))]
    fn run_svc(processor: &mut Processor) {
        processor.set_msp(0x2000_0200);
        processor.set_pc(0x100);
        processor.execute(&decode_16(0xdf00), 2);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_svc_that_cannot_preempt_is_escalated_to_hardfault() {
        // PRIMASK: the execution priority is 0, and so is SVCall's
        let mut processor = Processor::new();
        processor.primask = true;
        run_svc(&mut processor);
        assert!(processor.running);
        assert_eq!(processor.psr.get_isr_number(), 3);
        assert_eq!(processor.hfsr & (1 << 30), 1 << 30, "FORCED");

        // BASEPRI 0x10 against an SVCall at 0x20, and then at 0x08, which preempts
        let mut processor = Processor::new();
        processor.basepri = 0x10;
        processor.set_exception_priority(Exception::SVCall, 0x20);
        run_svc(&mut processor);
        assert_eq!(processor.psr.get_isr_number(), 3);
        let mut processor = Processor::new();
        processor.basepri = 0x10;
        processor.set_exception_priority(Exception::SVCall, 0x08);
        run_svc(&mut processor);
        assert_eq!(processor.psr.get_isr_number(), 11);

        // inside a handler of the same priority
        let mut processor = Processor::new();
        set_active_exception(&mut processor, Exception::SysTick);
        run_svc(&mut processor);
        assert_eq!(processor.psr.get_isr_number(), 3);

        // and with nothing in the way it is SVCall
        let mut processor = Processor::new();
        run_svc(&mut processor);
        assert_eq!(processor.psr.get_isr_number(), 11);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_svc_inside_hardfault_or_with_faultmask_is_a_lockup() {
        let mut processor = Processor::new();
        set_active_exception(&mut processor, Exception::HardFault);
        run_svc(&mut processor);
        assert!(!processor.running);
        assert_eq!(
            processor
                .take_pending_fault_trap()
                .map(|trap| trap.trap_reason),
            Some(FaultTrapReason::Lockup)
        );

        let mut processor = Processor::new();
        processor.faultmask = true;
        run_svc(&mut processor);
        assert!(!processor.running);
    }

    // BKPT with nothing to serve it is a debug event: DFSR.BKPT is set, and with the DebugMonitor
    // enabled (DEMCR.MON_EN) and able to preempt it is taken, and otherwise it is escalated to
    // HardFault with HFSR.DEBUGEVT. This is from my memory of the manual. BKPT #0xab with no
    // semihosting handler is the same, there being no host to take it.
    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_bkpt_without_a_debugger_is_a_hardfault_with_debugevt() {
        for opcode in [0xbe00, 0xbeab] {
            let mut processor = Processor::new();
            processor.set_msp(0x2000_0200);
            processor.set_pc(0x100);

            processor.execute(&decode_16(opcode), 2);

            assert_eq!(processor.psr.get_isr_number(), 3, "{opcode:#x}");
            assert_eq!(processor.hfsr & (1 << 31), 1 << 31, "DEBUGEVT");
            assert_eq!(processor.dfsr & (1 << 1), 1 << 1, "DFSR.BKPT");
            // the stacked PC is the BKPT itself
            let frame = processor.get_msp();
            assert_eq!(processor.read32(frame + 0x18), Ok(0x100));
        }
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_bkpt_with_the_debug_monitor_enabled_takes_it() {
        let mut processor = Processor::new();
        processor.set_msp(0x2000_0200);
        processor.set_pc(0x100);
        processor.write32(0xE000_EDFC, 1 << 16).unwrap(); // DEMCR.MON_EN

        processor.execute(&decode_16(0xbe00), 2);

        assert_eq!(processor.psr.get_isr_number(), 12);
        assert_eq!(processor.dfsr & (1 << 1), 1 << 1);
        assert_eq!(processor.hfsr & (1 << 31), 0);

        // inside a handler of the same priority it cannot preempt: HardFault
        let mut processor = Processor::new();
        processor.set_msp(0x2000_0200);
        processor.write32(0xE000_EDFC, 1 << 16).unwrap();
        set_active_exception(&mut processor, Exception::SysTick);
        processor.execute(&decode_16(0xbe00), 2);
        assert_eq!(processor.psr.get_isr_number(), 3);
    }

    // A stacking failure on the entry of an exception is a lockup when the execution priority is
    // already below 0, as it is for a fault, and an ordinary fault above it.
    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_a_stacking_failure_on_entry_is_a_lockup_only_below_priority_zero() {
        // an interrupt from Thread mode, with a stack pointer that is not in memory
        let mut processor = Processor::new();
        processor.set_msp(0x0f00_0000);
        processor.execution_priority = 256;
        processor.set_exception_pending(Exception::Interrupt { n: 3 });
        processor.check_exceptions();
        let trap = processor.take_pending_fault_trap().expect("a trap");
        assert_eq!(trap.trap_reason, FaultTrapReason::Fault);

        // the NMI preempting HardFault, which is the only thing that can
        let mut processor = Processor::new();
        set_active_exception(&mut processor, Exception::HardFault);
        processor.set_msp(0x0f00_0000);
        processor.set_exception_pending(Exception::NMI);
        processor.check_exceptions();
        let trap = processor.take_pending_fault_trap().expect("a trap");
        assert_eq!(trap.trap_reason, FaultTrapReason::Lockup);

        // and with FAULTMASK, where no IPSR says HardFault or NMI
        let mut processor = Processor::new();
        processor.faultmask = true;
        processor.set_msp(0x0f00_0000);
        processor.set_exception_pending(Exception::NMI);
        processor.check_exceptions();
        let trap = processor.take_pending_fault_trap().expect("a trap");
        assert_eq!(trap.trap_reason, FaultTrapReason::Lockup);
    }

    // The xPSR a refused return stacks is the popped one in the fields the xPSR has: N Z C V Q,
    // IT/ICI, T, the IPSR, GE on a DSP build, and the stack alignment bit. The reserved ones stack
    // as zero.
    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_the_frame_of_a_refused_return_stacks_the_reserved_bits_of_the_xpsr_as_zero() {
        let mut processor = Processor::new();
        processor.set_msp(0x2000_0200);
        processor.set_pc(0x100);
        processor.execute(&decode_16(0xdf00), 2); // svc #0

        let frame = processor.get_msp();
        // every bit set but IPSR (a refused 3 for Thread mode, so the return is refused)
        processor.write32(frame + 0x1c, 0xffff_fe00 | 3).unwrap();
        processor.set_r(Reg::LR, 0xffff_fff9);
        processor.execute(&decode_16(0x4770), 2);

        let stacked = processor.read32(processor.get_msp() + 0x1c).unwrap();
        let ge = if cfg!(feature = "has-dsp-ext") {
            0x000f_0000
        } else {
            0
        };
        // 31:27, IT 26:25, T, GE, IT 15:10, bit 9 is the alignment of the new frame, IPSR
        let implemented = 0xffff_0000 & !0x00f0_0000 & !0x000f_0000 | ge | 0xfc00 | 0x1ff;
        assert_eq!(
            stacked & !(1 << 9),
            (0xffff_fe00 | 3) & implemented & !(1 << 9)
        );
        assert_eq!(stacked & 0x00f0_0000, 0x0100_0000 & 0x00f0_0000);
    }

    // ARMv7-M numbers: 7-10 reserved, 11 SVCall, 12 DebugMonitor, 13 reserved, 14 PendSV.
    #[test]
    fn test_exception_numbers_are_the_armv7m_ones() {
        assert_eq!(usize::from(Exception::SVCall), 11);
        assert_eq!(usize::from(Exception::DebugMonitor), 12);
        assert_eq!(usize::from(Exception::PendSV), 14);
        for number in 1..16 {
            assert_eq!(usize::from(Exception::from(number)), number);
        }
        assert_eq!(Exception::from(12), Exception::DebugMonitor);
        for reserved in [7, 8, 9, 10, 13] {
            let processor = Processor::new();
            assert!(!processor.is_activatable(reserved), "{reserved}");
        }
        assert!(Processor::new().is_activatable(12));
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_debug_monitor_takes_its_vector_at_offset_0x30() {
        let mut processor = Processor::new();
        processor.set_msp(0x2000_0200);
        // the flash is the vector table; the words at 0x28 (exception 10) and 0x30 (exception 12)
        processor.write32(0x28, 0x0000_0201).unwrap();
        processor.write32(0x30, 0x0000_0101).unwrap();

        assert_eq!(
            processor.exception_entry(Exception::DebugMonitor, 0),
            Ok(())
        );

        assert_eq!(processor.get_pc(), 0x100);
        assert_eq!(processor.psr.get_isr_number(), 12);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_undefined_instruction_stacks_the_address_of_that_instruction() {
        for usage_fault_enabled in [true, false] {
            let mut processor = Processor::new();
            processor.set_msp(0x2000_0200);
            processor.set_pc(0x100);
            if usage_fault_enabled {
                processor.write32(0xE000_ED24, 1 << 18).unwrap();
            }

            // udf #0
            processor.execute(&decode_16(0xde00), 2);

            assert_eq!(stacked_pc_after(&mut processor), 0x100);
        }
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_invstate_stacks_the_address_the_return_went_to() {
        const FRAMEPTR: u32 = 0x2000_00e0;

        let mut processor = Processor::new();
        processor.write32(0xE000_ED24, 1 << 18).unwrap();
        // a stacked frame with the Thumb bit clear
        processor.set_msp(FRAMEPTR);
        processor.write32(FRAMEPTR + 0x18, 0x0800_0001).unwrap();
        // T clear, and an IPSR that is fine for a return to Handler mode
        processor
            .write32(FRAMEPTR + 0x1c, usize::from(Exception::SysTick) as u32)
            .unwrap();
        set_active_exception(&mut processor, Exception::HardFault);
        processor
            .exceptions
            .get_mut(&usize::from(Exception::SysTick))
            .unwrap()
            .active = true;
        processor.set_pc(0x100);
        processor.set_r(Reg::LR, 0xffff_ff00 | EXC_RETURN_HANDLER);

        // bx lr, which returns from the exception and finds the frame invalid
        processor.execute(&decode_16(0x4770), 2);

        // The frame was popped (its PC is where the return went), and the fault is taken on that
        // state, so the faulting instruction is the one at the address returned to.
        assert_eq!(processor.cfsr & (1 << 17), 1 << 17, "INVSTATE");
        assert_eq!(stacked_pc_after(&mut processor), 0x0800_0000);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_supervisor_call_and_interrupts_stack_the_next_instruction() {
        let mut processor = Processor::new();
        processor.set_msp(0x2000_0200);
        processor.set_pc(0x100);

        // svc #0 returns to the instruction after it
        processor.execute(&decode_16(0xdf00), 2);
        assert_eq!(stacked_pc_after(&mut processor), 0x102);

        // an interrupt taken between two instructions returns to the one that has not run
        let mut processor = Processor::new();
        processor.set_msp(0x2000_0200);
        processor.set_pc(0x200);
        processor.nvic_write_iser(0, 1);
        processor.nvic_write_ispr(0, 1);
        processor.execution_priority = 100;
        processor.check_exceptions();
        assert_eq!(stacked_pc_after(&mut processor), 0x200);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_exception_return_rejects_thread_return_with_nonzero_stacked_ipsr() {
        const FRAMEPTR: u32 = 0x2000_00e0;

        let mut processor = Processor::new();
        processor.set_msp(FRAMEPTR);
        processor
            .write32(FRAMEPTR.wrapping_add(0x18), 0x0800_0001)
            .unwrap();
        processor
            .write32(
                FRAMEPTR.wrapping_add(0x1c),
                (1 << 24) | usize::from(Exception::HardFault) as u32,
            )
            .unwrap();

        set_active_exception(&mut processor, Exception::SysTick);

        let result = processor.exception_return(EXC_RETURN_THREAD_MSP);

        assert_eq!(result, Err(Fault::InvPc));
        assert_eq!(
            processor.get_r(Reg::LR),
            0xF000_0000 + EXC_RETURN_THREAD_MSP
        );
        assert!(!processor.exception_active(Exception::SysTick));
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_exception_return_rejects_stacked_frame_with_thumb_bit_clear() {
        const FRAMEPTR: u32 = 0x2000_00e0;

        let mut processor = Processor::new();
        processor.set_msp(FRAMEPTR);
        processor.set_pc(0x0800_00aa);
        processor.set_r(Reg::R0, 0x1111_1111);
        processor.set_r(Reg::R1, 0x2222_2222);
        processor.set_r(Reg::R2, 0x3333_3333);
        processor.set_r(Reg::R3, 0x4444_4444);
        processor.set_r(Reg::R12, 0x5555_5555);
        processor.set_r(Reg::LR, 0x6666_6666);
        processor.write32(FRAMEPTR, 0xaaaa_0000).unwrap();
        processor
            .write32(FRAMEPTR.wrapping_add(0x4), 0xbbbb_0001)
            .unwrap();
        processor
            .write32(FRAMEPTR.wrapping_add(0x8), 0xcccc_0002)
            .unwrap();
        processor
            .write32(FRAMEPTR.wrapping_add(0xc), 0xdddd_0003)
            .unwrap();
        processor
            .write32(FRAMEPTR.wrapping_add(0x10), 0xeeee_0004)
            .unwrap();
        processor
            .write32(FRAMEPTR.wrapping_add(0x14), 0xffff_0005)
            .unwrap();
        processor
            .write32(FRAMEPTR.wrapping_add(0x18), 0x0800_0001)
            .unwrap();
        processor.write32(FRAMEPTR.wrapping_add(0x1c), 0).unwrap();

        set_active_exception(&mut processor, Exception::SysTick);

        let result = processor.exception_return(EXC_RETURN_THREAD_MSP);

        // The frame is popped before the xPSR is checked, and the pop is not undone: the
        // registers, PC and stack pointer are the frame's when the fault is taken.
        assert_eq!(result, Err(Fault::Invstate));
        assert_eq!(processor.get_pc(), 0x0800_0000);
        assert_eq!(processor.get_r(Reg::R0), 0xaaaa_0000);
        assert_eq!(processor.get_r(Reg::R1), 0xbbbb_0001);
        assert_eq!(processor.get_r(Reg::R2), 0xcccc_0002);
        assert_eq!(processor.get_r(Reg::R3), 0xdddd_0003);
        assert_eq!(processor.get_r(Reg::R12), 0xeeee_0004);
        assert_eq!(processor.get_r(Reg::LR), 0xffff_0005);
        assert_eq!(processor.get_msp(), FRAMEPTR + 0x20);
    }

    #[test]
    fn test_wfi_wakeup_primask_masked_interrupt() {
        // Arrange
        let mut processor = Processor::new();
        processor.reset().unwrap();

        // Enable PRIMASK
        processor.primask = true;

        // Set pending interrupt
        processor.set_exception_pending(Exception::Interrupt { n: 1 });
        processor.set_exception_priority(Exception::Interrupt { n: 1 }, 10);

        // Act & Assert
        // Should return true because WFI ignores PRIMASK
        assert!(processor.has_wakeup_condition());
    }

    #[test]
    fn test_wfi_wakeup_no_interrupt() {
        // Arrange
        let mut processor = Processor::new();
        processor.reset().unwrap();

        // Act & Assert
        assert!(!processor.has_wakeup_condition());
    }

    #[test]
    fn test_wfi_wakeup_low_priority_interrupt() {
        // Arrange
        let mut processor = Processor::new();
        processor.reset().unwrap();

        // Simulate being in a high priority handler
        processor.set_exception_priority(Exception::SysTick, 0); // High urgency
        processor.exception_taken(Exception::SysTick).unwrap();

        // Set pending interrupt with lower urgency
        processor.set_exception_priority(Exception::Interrupt { n: 1 }, 10); // Low urgency
        processor.set_exception_pending(Exception::Interrupt { n: 1 });

        // Act & Assert
        // Should return false because pending interrupt priority (10) is not < current execution priority (0)
        assert!(!processor.has_wakeup_condition());
    }

    #[cfg(not(feature = "armv6m"))]
    #[test]
    fn test_wfi_wakeup_faultmask_masked_interrupt() {
        // Arrange
        let mut processor = Processor::new();
        processor.reset().unwrap();

        // Enable FAULTMASK
        processor.faultmask = true;

        // Set pending interrupt
        processor.set_exception_pending(Exception::Interrupt { n: 1 });
        processor.set_exception_priority(Exception::Interrupt { n: 1 }, 10);

        // Act & Assert
        // Should return false because FAULTMASK blocks normal interrupts
        assert!(!processor.has_wakeup_condition());
    }

    // pop_stack GE gating: with DSP extension GE bits must be restored from the stacked xPSR.
    #[test]
    #[cfg(feature = "has-dsp-ext")]
    fn test_pop_stack_restores_ge_bits_with_dsp_ext() {
        const FRAMEPTR: u32 = 0x2000_0100;
        let mut core = Processor::new();
        core.set_msp(FRAMEPTR);

        // Build a minimal exception frame (basic, no FP).
        // offset 0x00..0x18: r0..r3, r12, lr, pc (Thumb address, bit 0 = 1)
        core.write32(FRAMEPTR, 0).unwrap();
        core.write32(FRAMEPTR.wrapping_add(0x4), 0).unwrap();
        core.write32(FRAMEPTR.wrapping_add(0x8), 0).unwrap();
        core.write32(FRAMEPTR.wrapping_add(0xc), 0).unwrap();
        core.write32(FRAMEPTR.wrapping_add(0x10), 0).unwrap();
        core.write32(FRAMEPTR.wrapping_add(0x14), 0).unwrap();
        core.write32(FRAMEPTR.wrapping_add(0x18), 0x0800_0001)
            .unwrap();
        // Stacked xPSR: Thumb bit (bit 24) set; ISR = 0 (thread return); GE1 + GE3 set.
        // GE[3:0] = 0b1010 = bits [19:16] → 0x000A_0000
        let stacked_xpsr: u32 = (1 << 24) | 0x000A_0000;
        core.write32(FRAMEPTR.wrapping_add(0x1c), stacked_xpsr)
            .unwrap();

        // Clear GE bits in live PSR so we can verify they come from the stacked value.
        core.psr.set_ge0(false);
        core.psr.set_ge1(false);
        core.psr.set_ge2(false);
        core.psr.set_ge3(false);

        // EXC_RETURN = 0xFFFF_FFF9: return-to-thread, MSP, no FP frame.
        core.pop_stack(FRAMEPTR, 0xFFFF_FFF9).unwrap();

        assert!(!core.psr.get_ge0(), "GE0 must remain clear");
        assert!(core.psr.get_ge1(), "GE1 must be restored from stacked xPSR");
        assert!(!core.psr.get_ge2(), "GE2 must remain clear");
        assert!(core.psr.get_ge3(), "GE3 must be restored from stacked xPSR");
    }

    // pop_stack GE gating: without DSP extension GE bits in the stacked xPSR must be ignored.
    #[test]
    #[cfg(not(feature = "has-dsp-ext"))]
    fn test_pop_stack_ignores_ge_bits_without_dsp_ext() {
        const FRAMEPTR: u32 = 0x2000_0200;
        let mut core = Processor::new();
        core.set_msp(FRAMEPTR);

        core.write32(FRAMEPTR, 0).unwrap();
        core.write32(FRAMEPTR.wrapping_add(0x4), 0).unwrap();
        core.write32(FRAMEPTR.wrapping_add(0x8), 0).unwrap();
        core.write32(FRAMEPTR.wrapping_add(0xc), 0).unwrap();
        core.write32(FRAMEPTR.wrapping_add(0x10), 0).unwrap();
        core.write32(FRAMEPTR.wrapping_add(0x14), 0).unwrap();
        core.write32(FRAMEPTR.wrapping_add(0x18), 0x0800_0001)
            .unwrap();
        // Stacked xPSR: Thumb bit set + all GE bits set.
        let stacked_xpsr: u32 = (1 << 24) | 0x000F_0000;
        core.write32(FRAMEPTR.wrapping_add(0x1c), stacked_xpsr)
            .unwrap();

        // GE bits in live PSR are already zero (internal storage may still hold them).
        // Capture state before pop.
        let pre_psr_ge_bits = core.psr.value & 0x000F_0000;

        core.pop_stack(FRAMEPTR, 0xFFFF_FFF9).unwrap();

        // Without DSP extension the GE bits in the live PSR must not change.
        let post_psr_ge_bits = core.psr.value & 0x000F_0000;
        assert_eq!(
            pre_psr_ge_bits, post_psr_ge_bits,
            "GE bits must not be modified by pop_stack without DSP extension"
        );
    }

    const IT_STACK_START: u32 = 0x2000_0100;

    fn stacked_xpsr_after_entry(itstate: u8) -> (Processor, u32) {
        let mut core = Processor::new();
        core.control.sp_sel = false;
        core.set_msp(IT_STACK_START);
        core.psr.value = 0xffff_ffff;
        core.itstate = itstate;
        core.push_stack(99, None).unwrap();
        let xpsr = core.read32(IT_STACK_START - 0x20 + 0x1c).unwrap();
        (core, xpsr)
    }

    #[test]
    fn test_push_stack_saves_it_state_in_xpsr() {
        // ITSTATE[7:2] goes to xPSR[15:10], ITSTATE[1:0] to xPSR[26:25]
        let (core, xpsr) = stacked_xpsr_after_entry(0x18);
        assert_eq!(xpsr.get_bits(10..16), 0b000110);
        assert_eq!(xpsr.get_bits(25..27), 0b00);
        assert_eq!(core.itstate, 0);

        let (core, xpsr) = stacked_xpsr_after_entry(0xa4);
        assert_eq!(xpsr.get_bits(10..16), 0b101001);
        assert_eq!(xpsr.get_bits(25..27), 0b00);
        assert_eq!(core.itstate, 0);

        let (_, xpsr) = stacked_xpsr_after_entry(0xa7);
        assert_eq!(xpsr.get_bits(10..16), 0b101001);
        assert_eq!(xpsr.get_bits(25..27), 0b11);
    }

    #[test]
    fn test_push_stack_without_it_block_stacks_clear_it_bits() {
        let (_, xpsr) = stacked_xpsr_after_entry(0);
        assert_eq!(xpsr.get_bits(10..16), 0);
        assert_eq!(xpsr.get_bits(25..27), 0);
    }

    fn pop_frame_with_xpsr(xpsr: u32) -> Processor {
        const FRAMEPTR: u32 = 0x2000_0100;
        let mut core = Processor::new();
        core.set_msp(FRAMEPTR);
        for offset in (0..0x18).step_by(4) {
            core.write32(FRAMEPTR + offset, 0).unwrap();
        }
        core.write32(FRAMEPTR + 0x18, 0x0800_0001).unwrap();
        core.write32(FRAMEPTR + 0x1c, xpsr).unwrap();
        core.pop_stack(FRAMEPTR, 0xffff_fff9).unwrap();
        core
    }

    #[test]
    fn test_pop_stack_restores_it_state_from_xpsr() {
        let core = pop_frame_with_xpsr((1 << 24) | (0b101001 << 10));
        assert_eq!(core.itstate, 0xa4);

        let core = pop_frame_with_xpsr((1 << 24) | (0b000110 << 10) | (0b11 << 25));
        assert_eq!(core.itstate, 0x1b);

        let core = pop_frame_with_xpsr(1 << 24);
        assert_eq!(core.itstate, 0);
    }

    #[test]
    fn test_it_state_survives_exception_entry_and_return() {
        for itstate in [0x00, 0x18, 0xa4, 0x03, 0xa7, 0xff] {
            let (core, xpsr) = stacked_xpsr_after_entry(itstate);
            assert_eq!(core.itstate, 0);
            let core = pop_frame_with_xpsr(xpsr & !0x1ff);
            assert_eq!(core.itstate, itstate);
        }
    }
}
