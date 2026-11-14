use super::vfp_operands::decode_three_reg;
use crate::core::instruction::Instruction;

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VMUL_t1(opcode: u32) -> Instruction {
    decode_three_reg(
        opcode,
        |params| Instruction::VMUL_f32 { params },
        |params| Instruction::VMUL_f64 { params },
    )
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VNMUL_t1(opcode: u32) -> Instruction {
    decode_three_reg(
        opcode,
        |params| Instruction::VNMUL_f32 { params },
        |params| Instruction::VNMUL_f64 { params },
    )
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VDIV_t1(opcode: u32) -> Instruction {
    decode_three_reg(
        opcode,
        |params| Instruction::VDIV_f32 { params },
        |params| Instruction::VDIV_f64 { params },
    )
}
