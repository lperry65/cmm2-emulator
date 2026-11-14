use super::vfp_operands::decode_two_reg;
use crate::core::instruction::Instruction;

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VABS_t1(opcode: u32) -> Instruction {
    decode_two_reg(
        opcode,
        |params| Instruction::VABS_f32 { params },
        |params| Instruction::VABS_f64 { params },
    )
}
