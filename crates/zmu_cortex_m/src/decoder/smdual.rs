use crate::core::bits::Bits;
use crate::core::instruction::{Instruction, Reg4NoSetFlagsParams};

/// SMUAD, SMLAD (op1 = 010) and SMUSD, SMLSD (op1 = 100), with their X forms in hw2<4>
/// (A7.7.147, .137, .151, .142; Table A5-28). Ra = 1111 is the form without the accumulate.
fn smdual(opcode: u32) -> Instruction {
    let ra = opcode.get_bits(12..16);
    Instruction::SMDUAL {
        params: Reg4NoSetFlagsParams {
            rd: opcode.get_bits(8..12).into(),
            rn: opcode.get_bits(16..20).into(),
            rm: opcode.get_bits(0..4).into(),
            ra: ra.into(),
        },
        subtract: opcode.get_bit(22),
        swap: opcode.get_bit(4),
        accumulate: ra != 0b1111,
    }
}

#[allow(non_snake_case)]
pub fn decode_SMLAD_t1(opcode: u32) -> Instruction {
    smdual(opcode)
}

#[allow(non_snake_case)]
pub fn decode_SMLSD_t1(opcode: u32) -> Instruction {
    smdual(opcode)
}
