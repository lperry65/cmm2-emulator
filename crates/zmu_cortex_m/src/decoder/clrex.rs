use crate::core::instruction::Instruction;

/// CLREX (A7.7.23). The (1) and (0) bits of the diagram are not tested by the table, as for the
/// barriers beside it.
#[allow(non_snake_case)]
pub fn decode_CLREX_t1(_opcode: u32) -> Instruction {
    Instruction::CLREX
}
