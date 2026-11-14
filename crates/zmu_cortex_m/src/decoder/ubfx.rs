use super::decode_UDF_t2;
use crate::core::bits::Bits;
use crate::core::instruction::{BfxParams, Instruction};

#[allow(non_snake_case)]
pub fn decode_UBFX_t1(opcode: u32) -> Instruction {
    let lsb = (opcode.get_bits(6..8) + (opcode.get_bits(12..15) << 2)) as usize;
    let widthminus1 = opcode.get_bits(0..5) as usize;

    // A field that runs past bit 31 is UNPREDICTABLE, so it decodes as UDF (like BFI).
    if lsb + widthminus1 > 31 {
        return decode_UDF_t2(opcode);
    }

    Instruction::UBFX {
        params: BfxParams {
            rd: From::from(opcode.get_bits(8..12) as u8),
            rn: From::from(opcode.get_bits(16..20) as u8),
            lsb,
            widthminus1,
        },
    }
}
