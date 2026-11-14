use super::vfp_operands::decode_three_reg;
use crate::core::instruction::Instruction;

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VADD_t1(opcode: u32) -> Instruction {
    decode_three_reg(
        opcode,
        |params| Instruction::VADD_f32 { params },
        |params| Instruction::VADD_f64 { params },
    )
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VSUB_t1(opcode: u32) -> Instruction {
    decode_three_reg(
        opcode,
        |params| Instruction::VSUB_f32 { params },
        |params| Instruction::VSUB_f64 { params },
    )
}
