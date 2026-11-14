use crate::core::{
    bits::Bits,
    instruction::{Instruction, VMRSTarget},
    register::Reg,
};

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VMRS(opcode: u32) -> Instruction {
    let target = Reg::from(opcode.get_bits(12..16) as u8);

    // Rt = SP is unpredictable. Rt = PC is the form that moves the flags into the APSR.
    if target == Reg::SP {
        return Instruction::UDF {
            opcode: opcode.into(),
            thumb32: true,
            imm32: 0,
        };
    }

    let rt = if target == Reg::PC {
        VMRSTarget::APSRNZCV
    } else {
        VMRSTarget::Register(target)
    };

    Instruction::VMRS { rt }
}
