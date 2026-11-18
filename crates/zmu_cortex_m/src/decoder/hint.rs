use crate::core::bits::Bits;
use crate::core::instruction::Instruction;

/// The 32-bit hints (A5.3.4, Table A5-14): op1 in hw2<10:8> and op2 in hw2<7:0>. An op1 that is
/// not 000 is UNDEFINED. NOP, YIELD, WFE, WFI, SEV, CSDB (0x14) and DBG (0xFx) are themselves, and
/// every other op2 is an unallocated hint, which executes as a NOP (A7.7.32 says the same of DBG's
/// option, which is for the debug system). The (0) bits hw2<13> and hw2<11> are not tested, as for
/// the barriers.
#[allow(non_snake_case)]
pub fn decode_HINT_t1(opcode: u32) -> Instruction {
    if opcode.get_bits(8..11) != 0 {
        return super::decode_UDF_t2(opcode);
    }
    match opcode.get_bits(0..8) {
        0x01 => Instruction::YIELD { thumb32: true },
        0x02 => Instruction::WFE { thumb32: true },
        0x03 => Instruction::WFI { thumb32: true },
        0x04 => Instruction::SEV { thumb32: true },
        0x14 => Instruction::CSDB,
        option if option >> 4 == 0xf => Instruction::DBG {
            option: (option & 0xf) as u8,
        },
        _ => Instruction::NOP { thumb32: true },
    }
}
