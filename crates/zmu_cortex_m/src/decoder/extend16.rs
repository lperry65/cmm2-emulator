use crate::core::bits::Bits;
use crate::core::instruction::{Instruction, Reg3UsizeParams};

/// SXTB16, SXTAB16, UXTB16 and UXTAB16 (A7.7.183, .180, .222, .219): hw1<4> is the sign (0
/// signed, 1 unsigned), and Rn = 1111 is the form without the add. The rotation is hw2<5:4>
/// times 8. The four table entries share this.
fn extend16(opcode: u32) -> Instruction {
    let rn = opcode.get_bits(16..20);
    Instruction::EXTEND16 {
        params: Reg3UsizeParams {
            rd: opcode.get_bits(8..12).into(),
            rn: rn.into(),
            rm: opcode.get_bits(0..4).into(),
            rotation: (opcode.get_bits(4..6) << 3) as usize,
        },
        signed: !opcode.get_bit(20),
        add: rn != 0b1111,
    }
}

#[allow(non_snake_case)]
pub fn decode_SXTB16_t1(opcode: u32) -> Instruction {
    extend16(opcode)
}

#[allow(non_snake_case)]
pub fn decode_SXTAB16_t1(opcode: u32) -> Instruction {
    extend16(opcode)
}

#[allow(non_snake_case)]
pub fn decode_UXTB16_t1(opcode: u32) -> Instruction {
    extend16(opcode)
}

#[allow(non_snake_case)]
pub fn decode_UXTAB16_t1(opcode: u32) -> Instruction {
    extend16(opcode)
}
