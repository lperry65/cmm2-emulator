use super::decode_UDF_t2;
use super::vfp_operands::{double_list, single_list};
use crate::core::bits::Bits;
use crate::core::instruction::{Instruction, VPushPopParams};
use enum_set::EnumSet;

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VPUSH_t1(opcode: u32) -> Instruction {
    let imm8 = opcode.get_bits(0..8);
    let first = opcode.get_bits(22..23) << 4 | opcode.get_bits(12..16);

    let Some(double_precision_registers) = double_list(first, imm8 / 2) else {
        return decode_UDF_t2(opcode);
    };

    Instruction::VPUSH {
        params: VPushPopParams {
            single_regs: false,
            single_precision_registers: EnumSet::new(),
            double_precision_registers,
            imm32: imm8 << 2,
        },
    }
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VPUSH_t2(opcode: u32) -> Instruction {
    let imm8 = opcode.get_bits(0..8);
    let first = opcode.get_bits(12..16) << 1 | opcode.get_bits(22..23);

    let Some(single_precision_registers) = single_list(first, imm8) else {
        return decode_UDF_t2(opcode);
    };

    Instruction::VPUSH {
        params: VPushPopParams {
            single_regs: true,
            single_precision_registers,
            double_precision_registers: EnumSet::new(),
            imm32: imm8 << 2,
        },
    }
}
