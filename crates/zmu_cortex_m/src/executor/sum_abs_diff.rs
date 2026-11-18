use crate::Processor;

use crate::executor::{ExecuteSuccess, ExecutorHelper};

use super::ExecuteResult;
use crate::core::instruction::Reg4NoSetFlagsParams;
use crate::core::register::BaseReg;

/// USAD8 and USADA8 (DSP extension)
pub trait IsaSumAbsDiff {
    fn exec_usad8(&mut self, params: &Reg4NoSetFlagsParams, accumulate: bool) -> ExecuteResult;
}

impl IsaSumAbsDiff for Processor {
    /// The absolute differences of the four unsigned bytes of Rn and Rm, added up, and Ra added
    /// for USADA8, wrapped to 32 bits. It does not touch GE or Q.
    fn exec_usad8(&mut self, params: &Reg4NoSetFlagsParams, accumulate: bool) -> ExecuteResult {
        if !self.condition_passed() {
            return Ok(ExecuteSuccess::NotTaken);
        }
        let (rn, rm) = (self.get_r(params.rn), self.get_r(params.rm));
        let mut result = if accumulate { self.get_r(params.ra) } else { 0 };
        for lane in 0..4 {
            let byte = |value: u32| (value >> (8 * lane)) as u8;
            result = result.wrapping_add(u32::from(byte(rn).abs_diff(byte(rm))));
        }
        self.set_r(params.rd, result);
        Ok(ExecuteSuccess::Taken { cycles: 1 })
    }
}
