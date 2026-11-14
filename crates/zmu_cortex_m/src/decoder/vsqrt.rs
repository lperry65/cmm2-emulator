use super::vfp_operands::decode_two_reg;
use crate::core::instruction::Instruction;

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VSQRT_t1(opcode: u32) -> Instruction {
    decode_two_reg(
        opcode,
        |params| Instruction::VSQRT_f32 { params },
        |params| Instruction::VSQRT_f64 { params },
    )
}
