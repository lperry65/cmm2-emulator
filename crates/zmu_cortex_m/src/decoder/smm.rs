use crate::core::bits::Bits;
use crate::core::instruction::{Instruction, MswAccumulate, Reg4NoSetFlagsParams};

/// SMMUL, SMMLA and SMMLS with their R forms. SMMUL and SMMLA are `decode_SMMLA_t1`. The op1
/// field (hw1<6:4>) is 101 for SMMLA and SMMUL, which is SMMLA with Ra = 1111, and 110 for SMMLS;
/// R is hw2<4>.
#[allow(non_snake_case)]
pub fn decode_SMMLA_t1(opcode: u32) -> Instruction {
    decode_smm(opcode)
}

/// SMMLS, op1 = 110
#[allow(non_snake_case)]
pub fn decode_SMMLS_t1(opcode: u32) -> Instruction {
    decode_smm(opcode)
}

fn decode_smm(opcode: u32) -> Instruction {
    let ra = opcode.get_bits(12..16);
    let accumulate = match (opcode.get_bit(21), ra) {
        (false, 0b1111) => MswAccumulate::None,
        (false, _) => MswAccumulate::Add,
        (true, _) => MswAccumulate::Sub,
    };
    Instruction::SMM {
        params: Reg4NoSetFlagsParams {
            rd: opcode.get_bits(8..12).into(),
            rn: opcode.get_bits(16..20).into(),
            rm: opcode.get_bits(0..4).into(),
            ra: ra.into(),
        },
        accumulate,
        round: opcode.get_bit(4),
    }
}
