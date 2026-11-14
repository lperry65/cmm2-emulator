use crate::core::instruction::{Instruction, RegImmParams};
use crate::core::operation::zero_extend;
use crate::core::{bits::Bits, register::Reg};

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_ADR_t1(command: u16) -> Instruction {
    Instruction::ADR {
        params: RegImmParams {
            r: From::from(command.get_bits(8..11)),
            imm32: u32::from(command.get_bits(0..8)) << 2,
        },
        thumb32: false,
    }
}

// The 12-bit offset of the two 32-bit forms, i:imm3:imm8.
fn adr_w(opcode: u32, imm32: impl FnOnce(u32) -> u32) -> Instruction {
    let i: u8 = u8::from(opcode.get_bit(26));
    let imm3: u8 = opcode.get_bits(12..15) as u8;
    let rd: u8 = opcode.get_bits(8..12) as u8;
    let imm8: u8 = opcode.get_bits(0..8) as u8;

    let params = [i, imm3, imm8];
    let lengths = [1, 3, 8];
    Instruction::ADR {
        params: RegImmParams {
            r: Reg::from(rd),
            imm32: imm32(zero_extend(&params, &lengths)),
        },
        thumb32: true,
    }
}

/// ADR.W towards lower addresses: Align(PC, 4) - imm32. The offset is kept negated, so that the
/// executor adds it in both forms.
#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_ADR_t2(opcode: u32) -> Instruction {
    adr_w(opcode, u32::wrapping_neg)
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_ADR_t3(opcode: u32) -> Instruction {
    adr_w(opcode, |imm32| imm32)
}
