use crate::Processor;

use crate::executor::{ExecuteSuccess, ExecutorHelper};

use super::ExecuteResult;
use crate::core::instruction::{Reg3NoSetFlagsParams, SaturatingOp};
use crate::core::register::{Apsr, BaseReg};

/// QADD, QSUB, QDADD and QDSUB (DSP extension)
pub trait IsaSaturatingAddSub {
    fn exec_qsat(&mut self, params: &Reg3NoSetFlagsParams, op: SaturatingOp) -> ExecuteResult;
}

/// The value as a 32-bit signed number, and whether it had to be clamped to get there.
fn signed_sat_q(value: i64) -> (i64, bool) {
    let clamped = value.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
    (clamped, clamped != value)
}

impl IsaSaturatingAddSub for Processor {
    /// Rm plus or minus Rn, where the two doubling forms double Rn first and saturate that too.
    /// Q is set when either step saturates and is never cleared (it is sticky).
    fn exec_qsat(&mut self, params: &Reg3NoSetFlagsParams, op: SaturatingOp) -> ExecuteResult {
        if !self.condition_passed() {
            return Ok(ExecuteSuccess::NotTaken);
        }
        let rm = i64::from(self.get_r(params.rm) as i32);
        let rn = i64::from(self.get_r(params.rn) as i32);
        let (operand, doubling_saturated) = match op {
            SaturatingOp::Add | SaturatingOp::Sub => (rn, false),
            SaturatingOp::DoubleAdd | SaturatingOp::DoubleSub => signed_sat_q(2 * rn),
        };
        let (result, saturated) = match op {
            SaturatingOp::Add | SaturatingOp::DoubleAdd => signed_sat_q(rm + operand),
            SaturatingOp::Sub | SaturatingOp::DoubleSub => signed_sat_q(rm - operand),
        };
        self.set_r(params.rd, result as u32);
        if doubling_saturated || saturated {
            self.psr.set_q(true);
        }
        Ok(ExecuteSuccess::Taken { cycles: 1 })
    }
}
