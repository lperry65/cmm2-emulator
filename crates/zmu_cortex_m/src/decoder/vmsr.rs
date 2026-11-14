use crate::core::{bits::Bits, instruction::Instruction, register::Reg};

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VMSR(opcode: u32) -> Instruction {
    let rt = Reg::from(opcode.get_bits(12..16) as u8);

    // Rt = SP and Rt = PC are unpredictable.
    if rt == Reg::SP || rt == Reg::PC {
        return Instruction::UDF {
            opcode: opcode.into(),
            thumb32: true,
            imm32: 0,
        };
    }

    Instruction::VMSR { rt }
}
