use crate::Processor;

use crate::executor::{ExecuteSuccess, ExecutorHelper};

use super::ExecuteResult;
use crate::core::instruction::{MswAccumulate, Reg4NoSetFlagsParams};
use crate::core::register::BaseReg;

/// SMMUL, SMMLA and SMMLS, and their rounding forms (DSP extension)
pub trait IsaMswMultiply {
    fn exec_smm(
        &mut self,
        params: &Reg4NoSetFlagsParams,
        accumulate: MswAccumulate,
        round: bool,
    ) -> ExecuteResult;
}

impl IsaMswMultiply for Processor {
    /// The high word of `(Ra << 32) + Rn * Rm`, of `(Ra << 32) - Rn * Rm`, or of `Rn * Rm`, all
    /// signed, with `0x8000_0000` added first for the R forms. The sum can pass 2^63, but only
    /// bits 63:32 are kept, so it is done in a wrapping 64 bits. Q is not touched.
    fn exec_smm(
        &mut self,
        params: &Reg4NoSetFlagsParams,
        accumulate: MswAccumulate,
        round: bool,
    ) -> ExecuteResult {
        if !self.condition_passed() {
            return Ok(ExecuteSuccess::NotTaken);
        }
        let product =
            i64::from(self.get_r(params.rn) as i32) * i64::from(self.get_r(params.rm) as i32);
        let ra = i64::from(self.get_r(params.ra) as i32) << 32;
        let mut result = match accumulate {
            MswAccumulate::None => product,
            MswAccumulate::Add => ra.wrapping_add(product),
            MswAccumulate::Sub => ra.wrapping_sub(product),
        };
        if round {
            result = result.wrapping_add(0x8000_0000);
        }
        self.set_r(params.rd, (result >> 32) as u32);
        Ok(ExecuteSuccess::Taken { cycles: 1 })
    }
}
