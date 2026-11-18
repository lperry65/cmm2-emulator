use crate::Processor;

use crate::executor::{ExecuteSuccess, ExecutorHelper};

use super::ExecuteResult;
use crate::core::instruction::Sat16Params;
use crate::core::{
    operation::saturate,
    register::{Apsr, BaseReg},
};

/// SSAT16 and USAT16 (DSP extension)
pub trait IsaSaturate16 {
    fn exec_sat16(&mut self, params: &Sat16Params, signed: bool) -> ExecuteResult;
}

impl IsaSaturate16 for Processor {
    /// Each halfword of Rn is a signed number, saturated to the range on its own. Q is set if
    /// either was, and never cleared.
    fn exec_sat16(&mut self, params: &Sat16Params, signed: bool) -> ExecuteResult {
        if !self.condition_passed() {
            return Ok(ExecuteSuccess::NotTaken);
        }
        let rn = self.get_r(params.rn);
        let bits = u32::from(params.saturate_to);
        let (low, low_saturated) = saturate(i64::from(rn as u16 as i16), bits, signed);
        let (high, high_saturated) = saturate(i64::from((rn >> 16) as u16 as i16), bits, signed);
        self.set_r(
            params.rd,
            u32::from(high as u16) << 16 | u32::from(low as u16),
        );
        if low_saturated || high_saturated {
            self.psr.set_q(true);
        }
        Ok(ExecuteSuccess::Taken { cycles: 1 })
    }
}
