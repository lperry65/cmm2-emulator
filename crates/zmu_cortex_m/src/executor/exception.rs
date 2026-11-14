use crate::Processor;
use crate::core::bits::Bits;
use crate::core::fault::Fault;

use crate::{
    core::exception::{Exception, ExceptionHandling},
    core::register::{BaseReg, Reg},
    executor::{ExecuteSuccess, ExecutorHelper},
    semihosting::{decode_semihostcmd, semihost_return},
};

use super::ExecuteResult;

const DFSR_BKPT: u32 = 1 << 1;
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
            if self.get_exception_priority(Exception::SVCall) >= self.get_execution_priority() {
                return Err(Fault::Forced);
            }
            let return_address = self.get_pc().wrapping_add(2);
            self.exception_entry(Exception::SVCall, return_address)?;
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

        // A debug event with no debugger: DFSR.BKPT, then the DebugMonitor if it is enabled and can
        // preempt, and otherwise a HardFault (HFSR.DEBUGEVT).
        self.dfsr |= DFSR_BKPT;
        let monitor_enabled = self.demcr.get_bit(DEMCR_MON_EN);
        if monitor_enabled
            && self.get_exception_priority(Exception::DebugMonitor) < self.get_execution_priority()
        {
            let return_address = self.get_pc();
            self.exception_entry(Exception::DebugMonitor, return_address)?;
            return Ok(ExecuteSuccess::Branched { cycles: 12 });
        }
        Err(Fault::DebugEvt)
    }
}
