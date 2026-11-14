use super::decode_UDF_t2;
use super::vfp_operands::{double_list, single_list};
use crate::core::bits::Bits;
use crate::core::instruction::{
    AddressingMode, Instruction, VStoreMultipleParams32, VStoreMultipleParams64,
};
use crate::core::register::Reg;

// The registers VLDM and VSTM name: the first (D:Vd) and how many words imm8 gives. Doubles
// are two words each. P = U (the encodings of other instructions, or undefined) and Rn = PC are not
// a VLDM or a VSTM.
pub(super) struct MultipleOperands {
    pub rn: Reg,
    pub write_back: bool,
    pub mode: AddressingMode,
    pub imm32: u32,
    pub first: u32,
    pub words: u32,
}

pub(super) fn multiple_operands(opcode: u32, double: bool) -> Option<MultipleOperands> {
    let w = opcode.get_bit(21);
    let u = opcode.get_bit(23);
    let p = opcode.get_bit(24);
    let n = opcode.get_bits(16..20) as u8;
    let d = opcode.get_bits(22..23);
    let vd = opcode.get_bits(12..16);
    let words = opcode.get_bits(0..8);

    if p == u || n == 15 {
        return None;
    }

    Some(MultipleOperands {
        rn: Reg::from(n),
        write_back: w,
        mode: if u {
            AddressingMode::IncrementAfter
        } else {
            AddressingMode::DecrementBefore
        },
        imm32: words << 2,
        first: if double { d << 4 | vd } else { vd << 1 | d },
        words,
    })
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VLDM_t1(opcode: u32) -> Instruction {
    let Some(operands) = multiple_operands(opcode, true) else {
        return decode_UDF_t2(opcode);
    };
    let Some(list) = double_list(operands.first, operands.words / 2) else {
        return decode_UDF_t2(opcode);
    };

    Instruction::VLDM_T1 {
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
pub fn decode_VLDM_t2(opcode: u32) -> Instruction {
    let Some(operands) = multiple_operands(opcode, false) else {
        return decode_UDF_t2(opcode);
    };
    let Some(list) = single_list(operands.first, operands.words) else {
        return decode_UDF_t2(opcode);
    };

    Instruction::VLDM_T2 {
        params: VStoreMultipleParams32 {
            rn: operands.rn,
            write_back: operands.write_back,
            list,
            mode: operands.mode,
            imm32: operands.imm32,
        },
    }
}
