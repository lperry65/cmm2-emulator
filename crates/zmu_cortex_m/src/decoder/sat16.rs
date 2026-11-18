use crate::core::bits::Bits;
use crate::core::instruction::{Instruction, Sat16Params};

/// SSAT16 stores the bit position less 1 in `sat_imm` (A7.7.153), USAT16 the position itself
/// (A7.7.214). The table has `sh = 1` and a shift amount of 0 here, the encoding that SSAT and
/// USAT hand over to these.
fn sat16(opcode: u32, signed: bool) -> Instruction {
    let field = opcode.get_bits(0..4) as u8;
    Instruction::SAT16 {
        params: Sat16Params {
            rd: opcode.get_bits(8..12).into(),
            rn: opcode.get_bits(16..20).into(),
            saturate_to: if signed { field + 1 } else { field },
        },
        signed,
    }
}

#[allow(non_snake_case)]
pub fn decode_SSAT16_t1(opcode: u32) -> Instruction {
    sat16(opcode, true)
}

#[allow(non_snake_case)]
pub fn decode_USAT16_t1(opcode: u32) -> Instruction {
    sat16(opcode, false)
}
