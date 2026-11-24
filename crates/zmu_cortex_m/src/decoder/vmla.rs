//! VMLA, VMLS, VNMLA and VNMLS (A7.7.238 and A7.7.250), the multiply-accumulate family that
//! keeps the product in a register. Like VFMA and friends they are encodings of the three
//! operand class, and the operands are read the same way; the family differs in what it does
//! with them, not in what it names.

use super::vfp_operands::decode_three_reg;
use crate::core::instruction::Instruction;

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VMLA_t1(opcode: u32) -> Instruction {
    decode_three_reg(
        opcode,
        |params| Instruction::VMLA_f32 { params },
        |params| Instruction::VMLA_f64 { params },
    )
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VMLS_t1(opcode: u32) -> Instruction {
    decode_three_reg(
        opcode,
        |params| Instruction::VMLS_f32 { params },
        |params| Instruction::VMLS_f64 { params },
    )
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VNMLA_t1(opcode: u32) -> Instruction {
    decode_three_reg(
        opcode,
        |params| Instruction::VNMLA_f32 { params },
        |params| Instruction::VNMLA_f64 { params },
    )
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VNMLS_t1(opcode: u32) -> Instruction {
    decode_three_reg(
        opcode,
        |params| Instruction::VNMLS_f32 { params },
        |params| Instruction::VNMLS_f64 { params },
    )
}
