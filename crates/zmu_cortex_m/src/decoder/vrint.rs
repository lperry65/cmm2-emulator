use super::vfp_operands::decode_two_reg;
use crate::core::{
    bits::Bits,
    fpregister::FPRounding,
    instruction::{Instruction, VRintParamsf32, VRintParamsf64},
};

// The registers and size of a VRINT, with the rounding and exactness its encoding gives it.
fn decode_vrint(opcode: u32, rounding: Option<FPRounding>, exact: bool) -> Instruction {
    decode_two_reg(
        opcode,
        |p| Instruction::VRINT_f32 {
            params: VRintParamsf32 {
                sd: p.sd,
                sm: p.sm,
                rounding,
                exact,
            },
        },
        |p| Instruction::VRINT_f64 {
            params: VRintParamsf64 {
                dd: p.dd,
                dm: p.dm,
                rounding,
                exact,
            },
        },
    )
}

/// VRINTR and VRINTZ differ in bit 7: VRINTZ rounds towards zero, VRINTR takes the FPSCR mode.
#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VRINTR_VRINTZ_t1(opcode: u32) -> Instruction {
    decode_vrint(opcode, opcode.get_bit(7).then_some(FPRounding::Zero), false)
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VRINTX_t1(opcode: u32) -> Instruction {
    decode_vrint(opcode, None, true)
}

/// VRINTA, VRINTN, VRINTP and VRINTM: bits 17 and 16 name the rounding.
#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VRINT_rm_t1(opcode: u32) -> Instruction {
    decode_vrint(
        opcode,
        Some(FPRounding::from_rm(opcode.get_bits(16..18))),
        false,
    )
}
