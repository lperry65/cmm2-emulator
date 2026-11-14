use super::vfp_operands::decode_three_reg;
use crate::core::{
    bits::Bits,
    condition::Condition,
    instruction::{Instruction, VSelParamsf32, VSelParamsf64},
};

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VSEL_t1(opcode: u32) -> Instruction {
    // Bits 21:20 pick EQ, VS, GE or GT.
    let cond = match opcode.get_bits(20..22) {
        0b00 => Condition::EQ,
        0b01 => Condition::VS,
        0b10 => Condition::GE,
        _ => Condition::GT,
    };

    decode_three_reg(
        opcode,
        |p| Instruction::VSEL_f32 {
            params: VSelParamsf32 {
                sd: p.sd,
                sn: p.sn,
                sm: p.sm,
                cond,
            },
        },
        |p| Instruction::VSEL_f64 {
            params: VSelParamsf64 {
                dd: p.dd,
                dn: p.dn,
                dm: p.dm,
                cond,
            },
        },
    )
}
