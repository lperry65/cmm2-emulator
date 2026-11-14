use super::vfp_operands::decode_three_reg;
use crate::core::instruction::Instruction;

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VFMA_t1(opcode: u32) -> Instruction {
    decode_three_reg(
        opcode,
        |params| Instruction::VFMA_f32 { params },
        |params| Instruction::VFMA_f64 { params },
    )
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VFMS_t1(opcode: u32) -> Instruction {
    decode_three_reg(
        opcode,
        |params| Instruction::VFMS_f32 { params },
        |params| Instruction::VFMS_f64 { params },
    )
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VFNMS_t1(opcode: u32) -> Instruction {
    decode_three_reg(
        opcode,
        |params| Instruction::VFNMS_f32 { params },
        |params| Instruction::VFNMS_f64 { params },
    )
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VFNMA_t1(opcode: u32) -> Instruction {
    decode_three_reg(
        opcode,
        |params| Instruction::VFNMA_f32 { params },
        |params| Instruction::VFNMA_f64 { params },
    )
}
