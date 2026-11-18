#[cfg(feature = "has-dsp-ext")]
use crate::Processor;

#[cfg(feature = "has-dsp-ext")]
use crate::executor::{ExecuteSuccess, ExecutorHelper};

#[cfg(feature = "has-dsp-ext")]
use super::ExecuteResult;
#[cfg(feature = "has-dsp-ext")]
use crate::core::instruction::{ParallelLanes, ParallelOp, ParallelResult, Reg3NoSetFlagsParams};
#[cfg(feature = "has-dsp-ext")]
use crate::core::{bits::Bits, register::BaseReg};

/// Parallel add/subtract operations (DSP extension)
#[cfg(feature = "has-dsp-ext")]
pub trait IsaParallelAddSub {
    fn exec_parallel(&mut self, params: &Reg3NoSetFlagsParams, op: ParallelOp) -> ExecuteResult;
}

/// Lane `index` of `value`, `bits` wide, as the number the instruction works on: sign extended
/// for the signed forms, zero extended for the unsigned ones.
#[cfg(feature = "has-dsp-ext")]
fn lane(value: u32, index: u32, bits: u32, signed: bool) -> i32 {
    let raw = value >> (bits * index);
    if signed {
        ((raw << (32 - bits)) as i32) >> (32 - bits)
    } else {
        (raw & ((1 << bits) - 1)) as i32
    }
}

#[cfg(feature = "has-dsp-ext")]
impl IsaParallelAddSub for Processor {
    /// All 36 are one loop over the lanes of the result (four bytes or two halfwords). A result
    /// lane is Rn's lane plus or minus a lane of Rm; the add and subtract forms use the same lane
    /// of both, and ASX and SAX cross the halves (Rn low with Rm high, Rn high with Rm low). Only
    /// the plain forms (S and U) write GE, and none of them touches Q.
    fn exec_parallel(&mut self, params: &Reg3NoSetFlagsParams, op: ParallelOp) -> ExecuteResult {
        if !self.condition_passed() {
            return Ok(ExecuteSuccess::NotTaken);
        }
        let rn = self.get_r(params.rn);
        let rm = self.get_r(params.rm);
        let bits = match op.lanes {
            ParallelLanes::Add8 | ParallelLanes::Sub8 => 8,
            _ => 16,
        };
        let mut result = 0;
        let mut ge = 0;
        for k in 0..32 / bits {
            // which lane of Rm, and whether it is subtracted
            let (m, subtract) = match op.lanes {
                ParallelLanes::Add8 | ParallelLanes::Add16 => (k, false),
                ParallelLanes::Sub8 | ParallelLanes::Sub16 => (k, true),
                ParallelLanes::Asx => (1 - k, k == 0),
                ParallelLanes::Sax => (1 - k, k == 1),
            };
            let a = lane(rn, k, bits, op.signed);
            let b = lane(rm, m, bits, op.signed);
            let value = if subtract { a - b } else { a + b };
            let kept = match op.result {
                ParallelResult::Wrap => value,
                ParallelResult::Saturate if op.signed => {
                    value.clamp(-(1 << (bits - 1)), (1 << (bits - 1)) - 1)
                }
                ParallelResult::Saturate => value.clamp(0, (1 << bits) - 1),
                // bits 8:1 or 16:1 of the sum or difference (an arithmetic shift of the value)
                ParallelResult::Halve => value >> 1,
            };
            result |= (kept as u32 & ((1 << bits) - 1)) << (bits * k);
            // an unsigned sum carries out of the lane, everything else is >= 0
            let flag = if op.signed || subtract {
                value >= 0
            } else {
                value >= 1 << bits
            };
            // a byte lane sets one GE bit, a halfword lane two
            ge |= u32::from(flag) * (((1 << (bits / 8)) - 1) << (bits / 8 * k));
        }
        self.set_r(params.rd, result);
        if op.result == ParallelResult::Wrap {
            self.psr.value.set_bits(16..20, ge);
        }
        Ok(ExecuteSuccess::Taken { cycles: 1 })
    }
}

#[cfg(all(test, not(feature = "has-dsp-ext")))]
mod tests {
    use crate::Processor;
    use crate::core::{instruction::Instruction, register::Reg};
    use crate::executor::ExecutorHelper;

    #[test]
    fn test_parallel_without_dsp_is_undef() {
        use crate::core::fault::Fault;
        use crate::core::instruction::{
            ParallelLanes, ParallelOp, ParallelResult, Reg3NoSetFlagsParams,
        };
        let mut core = Processor::new();
        let instruction = Instruction::PARALLEL {
            params: Reg3NoSetFlagsParams {
                rd: Reg::R0,
                rn: Reg::R1,
                rm: Reg::R2,
            },
            op: ParallelOp {
                signed: false,
                result: ParallelResult::Wrap,
                lanes: ParallelLanes::Add8,
            },
        };
        let result = core.execute_internal(&instruction);
        assert_eq!(result, Err(Fault::UndefInstr));
    }
}
