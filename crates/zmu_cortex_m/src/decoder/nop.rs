use crate::core::instruction::Instruction;

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_NOP_t1(_: u16) -> Instruction {
    Instruction::NOP { thumb32: false }
}
