use crate::core::bits::Bits;
use crate::core::instruction::{Instruction, SatParams};
use crate::core::operation::decode_imm_shift;

#[allow(non_snake_case)]
pub fn decode_SSAT_t1(opcode: u32) -> Instruction {
    let imm5 = ((opcode.get_bits(12..15) << 2) + opcode.get_bits(6..8)) as u8;
    let (shift_t, shift_n) = decode_imm_shift(u8::from(opcode.get_bit(21)) << 1, imm5);
    Instruction::SSAT {
        params: SatParams {
            rd: opcode.get_bits(8..12).into(),
            rn: opcode.get_bits(16..20).into(),
            // SSAT stores saturate_to - 1 in the field, USAT stores it as is
            saturate_to: opcode.get_bits(0..5) as u8 + 1,
            shift_t,
            shift_n,
        },
    }
}
