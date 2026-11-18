use crate::core::bits::Bits;
use crate::core::instruction::{Instruction, Reg3NoSetFlagsParams, SaturatingOp};

/// QADD, QDADD, QSUB and QDSUB are one encoding with op2 in hw2<5:4> (Table A5-27, op1 = 00):
/// 00 QADD, 01 QDADD, 10 QSUB, 11 QDSUB. Rn is in hw1 and Rm in hw2, and the manual's operand
/// order is Rm, Rn.
#[allow(non_snake_case)]
pub fn decode_QSAT_t1(opcode: u32) -> Instruction {
    let op = match opcode.get_bits(4..6) {
        0b00 => SaturatingOp::Add,
        0b01 => SaturatingOp::DoubleAdd,
        0b10 => SaturatingOp::Sub,
        _ => SaturatingOp::DoubleSub,
    };
    Instruction::QSAT {
        params: Reg3NoSetFlagsParams {
            rd: opcode.get_bits(8..12).into(),
            rn: opcode.get_bits(16..20).into(),
            rm: opcode.get_bits(0..4).into(),
        },
        op,
    }
}
