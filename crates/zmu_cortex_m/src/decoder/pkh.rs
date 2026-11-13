use crate::core::bits::Bits;
use crate::core::instruction::{Instruction, Reg3ShiftParams, SetFlags};
use crate::core::operation::decode_imm_shift;

/// PKHBT and PKHTB are one encoding; the TB bit picks the form, and with it the shift: PKHBT
/// shifts left, PKHTB shifts right arithmetically.
#[allow(non_snake_case)]
pub fn decode_PKH_t1(opcode: u32) -> Instruction {
    let tb_form = opcode.get_bit(5);
    let imm5 = (opcode.get_bits(12..15) << 2 | opcode.get_bits(6..8)) as u8;
    let (shift_t, shift_n) = decode_imm_shift(u8::from(tb_form) << 1, imm5);

    Instruction::PKH {
        params: Reg3ShiftParams {
            rd: opcode.get_bits(8..12).into(),
            rn: opcode.get_bits(16..20).into(),
            rm: opcode.get_bits(0..4).into(),
            shift_t,
            shift_n,
            setflags: SetFlags::False,
        },
        tb_form,
    }
}
