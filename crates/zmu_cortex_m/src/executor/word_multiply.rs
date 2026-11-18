use crate::Processor;

use crate::executor::{ExecuteSuccess, ExecutorHelper};

use super::ExecuteResult;
use crate::core::instruction::Reg4NoSetFlagsParams;
use crate::core::register::{Apsr, BaseReg};

/// SMULWB, SMULWT, SMLAWB and SMLAWT (DSP extension)
pub trait IsaWordMultiply {
    fn exec_smulw(
        &mut self,
        params: &Reg4NoSetFlagsParams,
        m_high: bool,
        accumulate: bool,
    ) -> ExecuteResult;
}

impl IsaWordMultiply for Processor {
    /// Bits 47:16 of Rn times the signed halfword of Rm (plus Ra << 16 for SMLAW). The 48-bit
    /// product cannot overflow, but the sum with Ra can, and that sets Q, which is never cleared.
    fn exec_smulw(
        &mut self,
        params: &Reg4NoSetFlagsParams,
        m_high: bool,
        accumulate: bool,
    ) -> ExecuteResult {
        if !self.condition_passed() {
            return Ok(ExecuteSuccess::NotTaken);
        }
        let rm = self.get_r(params.rm);
        let halfword = if m_high { rm >> 16 } else { rm } as u16 as i16;
        let mut result = i64::from(self.get_r(params.rn) as i32) * i64::from(halfword);
        if accumulate {
            result += i64::from(self.get_r(params.ra) as i32) << 16;
        }
        let high = result >> 16;
        self.set_r(params.rd, high as u32);
        if accumulate && high != i64::from(high as i32) {
            self.psr.set_q(true);
        }
        Ok(ExecuteSuccess::Taken { cycles: 1 })
    }
}
