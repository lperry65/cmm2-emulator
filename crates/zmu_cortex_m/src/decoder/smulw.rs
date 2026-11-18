use crate::core::bits::Bits;
use crate::core::instruction::{Instruction, Reg4NoSetFlagsParams};

/// SMULWB, SMULWT, SMLAWB and SMLAWT are one encoding (A7.7.150, A7.7.141): hw2<4> picks the
/// halfword of Rm, and Ra = 1111 is the form without the accumulate.
#[allow(non_snake_case)]
pub fn decode_SMLAW_t1(opcode: u32) -> Instruction {
    let ra = opcode.get_bits(12..16);
    Instruction::SMULW {
        params: Reg4NoSetFlagsParams {
            rd: opcode.get_bits(8..12).into(),
            rn: opcode.get_bits(16..20).into(),
            rm: opcode.get_bits(0..4).into(),
            ra: ra.into(),
        },
        m_high: opcode.get_bit(4),
        accumulate: ra != 0b1111,
    }
}
