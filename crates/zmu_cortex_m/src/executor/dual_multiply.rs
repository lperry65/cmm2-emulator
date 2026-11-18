use crate::Processor;

use crate::executor::{ExecuteSuccess, ExecutorHelper};

use super::ExecuteResult;
use crate::core::instruction::{Reg4NoSetFlagsParams, Reg643232Params};
use crate::core::register::{Apsr, BaseReg};

/// SMUAD, SMUSD, SMLAD, SMLSD, SMLALD and SMLSLD, with their X forms (DSP extension)
pub trait IsaDualMultiply {
    fn exec_smdual(
        &mut self,
        params: &Reg4NoSetFlagsParams,
        subtract: bool,
        swap: bool,
        accumulate: bool,
    ) -> ExecuteResult;
    fn exec_smdual_long(
        &mut self,
        params: &Reg643232Params,
        subtract: bool,
        swap: bool,
    ) -> ExecuteResult;
}

/// The products of the bottom halfwords and of the top halfwords of Rn and Rm (with Rm's
/// halfwords exchanged for the X forms), each a signed 32-bit number held in 64 bits
fn products(rn: u32, rm: u32, swap: bool) -> (i64, i64) {
    let rm = if swap { rm.rotate_right(16) } else { rm };
    let half = |value: u32, shift: u32| i64::from((value >> shift) as u16 as i16);
    (half(rn, 0) * half(rm, 0), half(rn, 16) * half(rm, 16))
}

impl IsaDualMultiply for Processor {
    /// The sum or difference of the two products, with Ra added, in 64 bits so that nothing is
    /// lost: Rd is its low 32 bits, and Q is set (never cleared) when it did not fit them.
    fn exec_smdual(
        &mut self,
        params: &Reg4NoSetFlagsParams,
        subtract: bool,
        swap: bool,
        accumulate: bool,
    ) -> ExecuteResult {
        if !self.condition_passed() {
            return Ok(ExecuteSuccess::NotTaken);
        }
        let (first, second) = products(self.get_r(params.rn), self.get_r(params.rm), swap);
        let mut result = if subtract {
            first - second
        } else {
            first + second
        };
        if accumulate {
            result += i64::from(self.get_r(params.ra) as i32);
        }
        self.set_r(params.rd, result as u32);
        if result != i64::from(result as i32) {
            self.psr.set_q(true);
        }
        Ok(ExecuteSuccess::Taken { cycles: 1 })
    }

    /// The same sum or difference added to the 64-bit value in RdHi:RdLo, wrapping at 2^64 without
    /// a flag.
    fn exec_smdual_long(
        &mut self,
        params: &Reg643232Params,
        subtract: bool,
        swap: bool,
    ) -> ExecuteResult {
        if !self.condition_passed() {
            return Ok(ExecuteSuccess::NotTaken);
        }
        let (first, second) = products(self.get_r(params.rn), self.get_r(params.rm), swap);
        let accumulator =
            (u64::from(self.get_r(params.rdhi)) << 32 | u64::from(self.get_r(params.rdlo))) as i64;
        let sum = if subtract {
            first - second
        } else {
            first + second
        };
        let result = accumulator.wrapping_add(sum) as u64;
        self.set_r(params.rdlo, result as u32);
        self.set_r(params.rdhi, (result >> 32) as u32);
        Ok(ExecuteSuccess::Taken { cycles: 1 })
    }
}
