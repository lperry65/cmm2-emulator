use crate::core::{
    bits::Bits,
    fpregister::FPRounding,
    instruction::{Instruction, VRintParamsf32, VRintParamsf64},
    register::{DoubleReg, SingleReg},
};

// The registers and size of a VRINT, with the rounding and exactness its encoding gives it.
fn decode_vrint(opcode: u32, rounding: Option<FPRounding>, exact: bool) -> Instruction {
    let sz = opcode.get_bit(8);

    let d = u8::from(opcode.get_bit(22));
    let vd = opcode.get_bits(12..16) as u8;
    let vm = opcode.get_bits(0..4) as u8;
    let m = u8::from(opcode.get_bit(5));

    if sz {
        Instruction::VRINT_f64 {
            params: VRintParamsf64 {
                dd: DoubleReg::from((d << 4) | vd),
                dm: DoubleReg::from((m << 4) | vm),
                rounding,
                exact,
            },
        }
    } else {
        Instruction::VRINT_f32 {
            params: VRintParamsf32 {
                sd: SingleReg::from((vd << 1) | d),
                sm: SingleReg::from((vm << 1) | m),
                rounding,
                exact,
            },
        }
    }
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
