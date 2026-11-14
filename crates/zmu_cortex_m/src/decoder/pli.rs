use crate::core::instruction::Instruction;

#[allow(non_snake_case)]
pub fn decode_PLI_lit_imm_t1(opcode: u32) -> Instruction {
    // a preload hint: it does nothing
    super::decode_hint_t2(opcode)
}

#[allow(non_snake_case)]
pub fn decode_PLI_lit_imm_t2(opcode: u32) -> Instruction {
    // a preload hint: it does nothing
    super::decode_hint_t2(opcode)
}

#[allow(non_snake_case)]
pub fn decode_PLI_lit_imm_t3(opcode: u32) -> Instruction {
    // a preload hint: it does nothing
    super::decode_hint_t2(opcode)
}

#[allow(non_snake_case)]
pub fn decode_PLI_reg_t1(opcode: u32) -> Instruction {
    // a preload hint: it does nothing
    super::decode_hint_t2(opcode)
}
