use crate::core::instruction::{Instruction, MsrParams};
use crate::core::{bits::Bits, register::Reg};

/// MSR (A7.7.83, B5.2.3). The mask says which fields of the APSR are written: bit 1 the flags
/// NZCVQ, bit 0 the GE bits. A mask of 00 is UNPREDICTABLE, and so is any mask but 10 for a
/// `SYSm` that is not the APSR, IAPSR, EAPSR or XPSR (0 to 3); those decode as UDF.
#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_MSR_reg_t1(opcode: u32) -> Instruction {
    let reg_rn: u8 = opcode.get_bits(16..20) as u8;
    let sysm = opcode.get_bits(0..8) as u8;
    let mask = opcode.get_bits(10..12) as u8;
    if mask == 0 || (mask != 0b10 && sysm > 3) {
        return Instruction::UDF {
            imm32: 0,
            opcode: opcode.into(),
            thumb32: true,
        };
    }
    Instruction::MSR_reg {
        params: MsrParams {
            rn: Reg::from(reg_rn),
            sysm,
            mask,
        },
    }
}
