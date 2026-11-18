//!
//! The registers the manual makes UNPREDICTABLE for the DSP instructions of issue 77, batch 3
//!
//! Like the rest of `unpredictable.rs`, an encoding that names one decodes as UDF, so the
//! executors never see them. SP and PC are never allowed as a destination or an operand of these,
//! except where the manual lets an accumulate register or the Rn of the extend-and-adds be PC (a
//! different instruction, which the decoder has already picked) and says SP is the only one.
//!

use super::unpredictable::is_bad;
use crate::core::instruction::Instruction;
use crate::core::register::Reg;

/// A7.7.211 and the pages for the others: d, n and m in {13, 15}; a == 13 where there is an
/// accumulate register (Ra = 15 is the form without one); dHi == dLo for the long multiplies; and
/// for the extends d, m in {13, 15} and n == 13 (n == 15 is the plain extend).
pub(super) fn uses_a_register_the_manual_forbids(instruction: &Instruction) -> bool {
    match instruction {
        Instruction::USAD8 { params, accumulate } => {
            is_bad(params.rd)
                || is_bad(params.rn)
                || is_bad(params.rm)
                || (*accumulate && params.ra == Reg::SP)
        }
        Instruction::SMULW {
            params, accumulate, ..
        }
        | Instruction::SMDUAL {
            params, accumulate, ..
        } => {
            is_bad(params.rd)
                || is_bad(params.rn)
                || is_bad(params.rm)
                || (*accumulate && params.ra == Reg::SP)
        }
        Instruction::SMDUAL_LONG { params, .. }
        | Instruction::SMLAL_half { params, .. }
        | Instruction::UMAAL { params } => {
            is_bad(params.rdlo)
                || is_bad(params.rdhi)
                || is_bad(params.rn)
                || is_bad(params.rm)
                || params.rdlo == params.rdhi
        }
        Instruction::EXTEND16 { params, add, .. } => {
            is_bad(params.rd) || is_bad(params.rm) || (*add && params.rn == Reg::SP)
        }
        // A7.7.179 to A7.7.181, A7.7.218 and A7.7.220: the same rule as SXTAB16
        Instruction::SXTAB { params }
        | Instruction::SXTAH { params }
        | Instruction::UXTAB { params }
        | Instruction::UXTAH { params } => {
            is_bad(params.rd) || is_bad(params.rm) || params.rn == Reg::SP
        }
        // A7.7.153 and A7.7.214: d and n in {13, 15}
        Instruction::SAT16 { params, .. } => is_bad(params.rd) || is_bad(params.rn),
        // A7.7.152 and A7.7.213: d and n in {13, 15}
        Instruction::SSAT { params } | Instruction::USAT { params } => {
            is_bad(params.rd) || is_bad(params.rn)
        }
        _ => false,
    }
}
