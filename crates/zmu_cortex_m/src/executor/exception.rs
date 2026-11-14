use crate::Processor;
#[cfg(not(feature = "armv6m"))]
use crate::core::bits::Bits;
use crate::core::fault::{Fault, FaultStatusContext};

use crate::{
    core::exception::{Exception, ExceptionHandling},
    core::register::{BaseReg, Reg},
    executor::{ExecuteSuccess, ExecutorHelper},
    semihosting::{decode_semihostcmd, semihost_return},
};

use super::ExecuteResult;

const DFSR_BKPT: u32 = 1 << 1;
#[cfg(not(feature = "armv6m"))]
const DEMCR_MON_EN: usize = 16;

/// What the guest gets in r0 for a semihosting call I cannot serve: -1.
const SEMIHOST_ERROR: u32 = u32::MAX;

/// Branching operations
pub trait IsaException {
    fn exec_svc(&mut self) -> ExecuteResult;
    fn exec_bkpt(&mut self, imm32: u32) -> ExecuteResult;
}

impl IsaException for Processor {
    fn exec_svc(&mut self) -> ExecuteResult {
        if self.condition_passed() {
            // SVCall has to be able to preempt what runs (PRIMASK, BASEPRI and FAULTMASK count);
            // if it cannot, the SVC is a forced HardFault, which is a lockup below priority 0.
            // The IT state stacked is the one after the SVC, like the return address
            // (ReturnAddress(), B1.5.6, p. B1-534). If the entry fails while stacking, the
            // derived fault stacks the SVC's own address, so the IT state goes back.
            let it_before = self.itstate;
            self.it_advance();
            if self.get_exception_priority(Exception::SVCall) >= self.get_execution_priority() {
                // an escalated fault keeps the return address of the original: for SVCall it is
                // the next instruction (ReturnAddress(), B1.5.6, p. B1-534)
                self.pending_fault_status = Some(FaultStatusContext {
                    return_address: Some(self.get_pc().wrapping_add(2)),
                    ..FaultStatusContext::default()
                });
                return Err(Fault::Forced);
            }
            let return_address = self.get_pc().wrapping_add(2);
            if let Err(fault) = self.exception_entry(Exception::SVCall, return_address) {
                self.itstate = it_before;
                return Err(fault);
            }
            return Ok(ExecuteSuccess::Branched { cycles: 12 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_bkpt(&mut self, imm32: u32) -> ExecuteResult {
        // Semihosting, when a host is there to serve it. Without a host there is nothing to read
        // from the guest, so its arguments are not looked at.
        if imm32 == 0xab && self.semihost_func.is_some() {
            let r0 = self.get_r(Reg::R0);
            let r1 = self.get_r(Reg::R1);
            match decode_semihostcmd(r0, r1, self)? {
                Some(semihost_cmd) => {
                    if let Some(sh_func) = &mut self.semihost_func {
                        let semihost_response = (sh_func)(&semihost_cmd);
                        semihost_return(self, &semihost_response);
                    }
                }
                None => self.set_r(Reg::R0, SEMIHOST_ERROR),
            }
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }

        // A debug event (C1.5). With a debugger attached it halts into it, and the PC stays on
        // the BKPT as in Debug state on silicon: the debugger moves it.
        self.dfsr |= DFSR_BKPT;
        if self.debugger_attached {
            self.debug_halt = true;
            return Ok(ExecuteSuccess::Branched { cycles: 1 });
        }
        // With no debugger: the DebugMonitor if it is enabled and its group priority is higher
        // than the execution priority (it sets DEMCR.MON_PEND), and otherwise a HardFault
        // (HFSR.DEBUGEVT). ARMv6-M has no DebugMonitor.
        #[cfg(not(feature = "armv6m"))]
        {
            let monitor_enabled = self.demcr.get_bit(DEMCR_MON_EN);
            if monitor_enabled
                && self.get_exception_priority(Exception::DebugMonitor)
                    < self.get_execution_priority()
            {
                // MON_PEND is set by the event and cleared when the exception is taken (C1-694 for the event, C1-689 for the bit),
                // which is at once here
                let return_address = self.get_pc();
                self.exception_entry(Exception::DebugMonitor, return_address)?;
                return Ok(ExecuteSuccess::Branched { cycles: 12 });
            }
        }
        Err(Fault::DebugEvt)
    }
}
