use crate::Processor;

use crate::executor::{ExecuteSuccess, ExecutorHelper};

use super::ExecuteResult;
use crate::core::instruction::Reg3UsizeParams;
use crate::core::{operation::ror, register::BaseReg};

/// SXTB16, SXTAB16, UXTB16 and UXTAB16 (DSP extension)
pub trait IsaExtend16 {
    fn exec_extend16(&mut self, params: &Reg3UsizeParams, signed: bool, add: bool)
    -> ExecuteResult;
}

impl IsaExtend16 for Processor {
    /// Rm rotated right, then its bytes 0 and 2 extended to halfwords, each added to the same
    /// halfword of Rn for the add forms. The halfwords add on their own: a carry out of the low
    /// one does not reach the high one.
    fn exec_extend16(
        &mut self,
        params: &Reg3UsizeParams,
        signed: bool,
        add: bool,
    ) -> ExecuteResult {
        if !self.condition_passed() {
            return Ok(ExecuteSuccess::NotTaken);
        }
        let rotated = ror(self.get_r(params.rm), params.rotation);
        let rn = if add { self.get_r(params.rn) } else { 0 };
        let extend = |byte: u32| -> u16 {
            if signed {
                i16::from(byte as u8 as i8) as u16
            } else {
                u16::from(byte as u8)
            }
        };
        let low = (rn as u16).wrapping_add(extend(rotated));
        let high = ((rn >> 16) as u16).wrapping_add(extend(rotated >> 16));
        self.set_r(params.rd, u32::from(high) << 16 | u32::from(low));
        Ok(ExecuteSuccess::Taken { cycles: 1 })
    }
}
