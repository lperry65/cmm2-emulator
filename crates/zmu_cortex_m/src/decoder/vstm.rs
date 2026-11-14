use super::decode_UDF_t2;
use super::vfp_operands::{double_list, single_list};
use super::vldm::multiple_operands;
use crate::core::instruction::{Instruction, VStoreMultipleParams32, VStoreMultipleParams64};

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VSTM_t1(opcode: u32) -> Instruction {
    let Some(operands) = multiple_operands(opcode, true) else {
        return decode_UDF_t2(opcode);
    };
    let Some(list) = double_list(operands.first, operands.words / 2) else {
        return decode_UDF_t2(opcode);
    };

    Instruction::VSTM_T1 {
        params: VStoreMultipleParams64 {
            rn: operands.rn,
            write_back: operands.write_back,
            list,
            mode: operands.mode,
            imm32: operands.imm32,
        },
    }
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VSTM_t2(opcode: u32) -> Instruction {
    let Some(operands) = multiple_operands(opcode, false) else {
        return decode_UDF_t2(opcode);
    };
    let Some(list) = single_list(operands.first, operands.words) else {
        return decode_UDF_t2(opcode);
    };

    Instruction::VSTM_T2 {
        params: VStoreMultipleParams32 {
            rn: operands.rn,
            write_back: operands.write_back,
            list,
            mode: operands.mode,
            imm32: operands.imm32,
        },
    }
}
