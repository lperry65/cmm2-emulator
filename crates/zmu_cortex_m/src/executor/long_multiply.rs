use crate::Processor;

use crate::executor::{ExecuteSuccess, ExecutorHelper};

use super::ExecuteResult;
use crate::core::instruction::Reg643232Params;
use crate::core::register::BaseReg;

/// SMLALBB, SMLALBT, SMLALTB, SMLALTT and UMAAL (DSP extension)
pub trait IsaLongMultiply {
    fn exec_smlal_half(
        &mut self,
        params: &Reg643232Params,
        n_high: bool,
        m_high: bool,
    ) -> ExecuteResult;
    fn exec_umaal(&mut self, params: &Reg643232Params) -> ExecuteResult;
}

impl Processor {
    fn long_accumulator(&self, params: &Reg643232Params) -> u64 {
        u64::from(self.get_r(params.rdhi)) << 32 | u64::from(self.get_r(params.rdlo))
    }

    fn set_long_result(&mut self, params: &Reg643232Params, result: u64) {
        self.set_r(params.rdlo, result as u32);
        self.set_r(params.rdhi, (result >> 32) as u32);
    }
}

impl IsaLongMultiply for Processor {
    /// The signed product of the two chosen halfwords added to the 64-bit value in RdHi:RdLo,
    /// wrapping at 2^64 without a flag.
    fn exec_smlal_half(
        &mut self,
        params: &Reg643232Params,
        n_high: bool,
        m_high: bool,
    ) -> ExecuteResult {
        if !self.condition_passed() {
            return Ok(ExecuteSuccess::NotTaken);
        }
        let half = |value: u32, high: bool| {
            i64::from((if high { value >> 16 } else { value }) as u16 as i16)
        };
        let product = half(self.get_r(params.rn), n_high) * half(self.get_r(params.rm), m_high);
        let result = (self.long_accumulator(params) as i64).wrapping_add(product);
        self.set_long_result(params, result as u64);
        Ok(ExecuteSuccess::Taken { cycles: 1 })
    }

    /// Rn * Rm + `RdHi` + `RdLo`, unsigned, which is at most 2^64 - 1 and so never wraps.
    fn exec_umaal(&mut self, params: &Reg643232Params) -> ExecuteResult {
        if !self.condition_passed() {
            return Ok(ExecuteSuccess::NotTaken);
        }
        let result = u64::from(self.get_r(params.rn)) * u64::from(self.get_r(params.rm))
            + u64::from(self.get_r(params.rdhi))
            + u64::from(self.get_r(params.rdlo));
        self.set_long_result(params, result);
        Ok(ExecuteSuccess::Taken { cycles: 1 })
    }
}
