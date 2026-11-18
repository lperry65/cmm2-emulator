use crate::core::bits::Bits;
use crate::core::instruction::{Instruction, Reg4NoSetFlagsParams};

/// USAD8 and USADA8 are one encoding (A7.7.211, A7.7.212; op1 = 111 and op2 = 00 of Table
/// A5-28): the sum of absolute differences with Ra added, or with Ra = 1111 without it. (Table
/// A5-28 has the two Ra columns the wrong way round; the instruction pages are what I follow,
/// and llvm agrees with them.)
#[allow(non_snake_case)]
pub fn decode_USAD8_t1(opcode: u32) -> Instruction {
    let ra = opcode.get_bits(12..16);
    Instruction::USAD8 {
        params: Reg4NoSetFlagsParams {
            rd: opcode.get_bits(8..12).into(),
            rn: opcode.get_bits(16..20).into(),
            rm: opcode.get_bits(0..4).into(),
            ra: ra.into(),
        },
        accumulate: ra != 0b1111,
    }
}
