use super::decode_UDF_t2;
use crate::core::{
    bits::Bits,
    fpregister::FPRounding,
    instruction::{
        Instruction, VCVTFixedParams, VCVTParams, VCVTParamsF16F32, VCVTParamsF16F64,
        VCVTParamsF32F16, VCVTParamsF32F64, VCVTParamsF64F16, VCVTParamsF64F32,
    },
    register::{DoubleReg, ExtensionReg, SingleReg},
};

// A conversion from floating point to a 32-bit integer in a single register.
fn decode_vcvt_to_integer(
    opcode: u32,
    unsigned: bool,
    rounding: Option<FPRounding>,
) -> Instruction {
    let dp_operation = opcode.get_bit(8);
    let d = (opcode.get_bits(12..16) as u8) << 1 | u8::from(opcode.get_bit(22));
    let vm = opcode.get_bits(0..4) as u8;
    let m_low = u8::from(opcode.get_bit(5));
    let m = if dp_operation {
        let Some(reg) = DoubleReg::new(m_low << 4 | vm) else {
            return decode_UDF_t2(opcode);
        };
        ExtensionReg::Double { reg }
    } else {
        ExtensionReg::Single {
            reg: SingleReg::from(vm << 1 | m_low),
        }
    };
    Instruction::VCVT {
        params: VCVTParams {
            to_integer: true,
            dp_operation,
            unsigned,
            rounding,
            d: ExtensionReg::Single {
                reg: SingleReg::from(d),
            },
            m,
        },
    }
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VCVT_t1(opcode: u32) -> Instruction {
    let vm = opcode.get_bits(0..4) as u8;
    let M = u8::from(opcode.get_bit(5));
    let op = opcode.get_bit(7);
    let sz = opcode.get_bit(8);
    let vd = opcode.get_bits(12..16) as u8;
    let opc2 = opcode.get_bits(16..19) as u8;
    let D = u8::from(opcode.get_bit(22));

    let to_integer = opc2.get_bit(2);
    let dp_operation = sz;
    if to_integer {
        // VCVT always rounds towards zero, VCVTR takes the FPSCR mode.
        decode_vcvt_to_integer(opcode, !opc2.get_bit(0), op.then_some(FPRounding::Zero))
    } else {
        let unsigned = !op;
        let m = vm << 1 | M;
        let d = if dp_operation {
            let Some(reg) = DoubleReg::new(D << 4 | vd) else {
                return decode_UDF_t2(opcode);
            };
            ExtensionReg::Double { reg }
        } else {
            ExtensionReg::Single {
                reg: SingleReg::from(vd << 1 | D),
            }
        };
        Instruction::VCVT {
            params: VCVTParams {
                to_integer,
                dp_operation,
                unsigned,
                rounding: None,
                d,
                m: ExtensionReg::Single {
                    reg: SingleReg::from(m),
                },
            },
        }
    }
}

/// VCVTA, VCVTN, VCVTP and VCVTM: bits 17 and 16 name the rounding and bit 7 the signedness.
#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VCVT_rm_t1(opcode: u32) -> Instruction {
    let rounding = FPRounding::from_rm(opcode.get_bits(16..18));
    decode_vcvt_to_integer(opcode, !opcode.get_bit(7), Some(rounding))
}

/// VCVT between floating point and fixed point, in place. Bit 18 says which way, bit 16 whether
/// the fixed-point number is unsigned, bit 7 whether it has 32 bits and not 16. The fraction
/// bits are the size less the immediate, which cannot be more than the size.
#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VCVT_fixed_t1(opcode: u32) -> Instruction {
    let size: u8 = if opcode.get_bit(7) { 32 } else { 16 };
    let imm5 = (opcode.get_bits(0..4) << 1 | opcode.get_bits(5..6)) as u8;
    let d = u8::from(opcode.get_bit(22));
    let vd = opcode.get_bits(12..16) as u8;

    if imm5 > size {
        return decode_UDF_t2(opcode);
    }
    let reg = if opcode.get_bit(8) {
        let Some(reg) = DoubleReg::new(d << 4 | vd) else {
            return decode_UDF_t2(opcode);
        };
        ExtensionReg::Double { reg }
    } else {
        ExtensionReg::Single {
            reg: SingleReg::from(vd << 1 | d),
        }
    };
    Instruction::VCVT_fixed {
        params: VCVTFixedParams {
            reg,
            to_fixed: opcode.get_bit(18),
            unsigned: opcode.get_bit(16),
            size,
            fraction_bits: size - imm5,
        },
    }
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VCVT_ds_t1(opcode: u32) -> Instruction {
    let vm = opcode.get_bits(0..4) as u8;
    let m = u8::from(opcode.get_bit(5));
    let sz = opcode.get_bit(8);
    let vd = opcode.get_bits(12..16) as u8;
    let d = u8::from(opcode.get_bit(22));

    if sz {
        let Some(dm) = DoubleReg::new((m << 4) | vm) else {
            return decode_UDF_t2(opcode);
        };
        Instruction::VCVT_f32_f64 {
            params: VCVTParamsF32F64 {
                sd: SingleReg::from((vd << 1) | d),
                dm,
            },
        }
    } else {
        let Some(dd) = DoubleReg::new((d << 4) | vd) else {
            return decode_UDF_t2(opcode);
        };
        Instruction::VCVT_f64_f32 {
            params: VCVTParamsF64F32 {
                dd,
                sm: SingleReg::from((vm << 1) | m),
            },
        }
    }
}

// VCVTB and VCVTT (A7.7.231): one half of a single register holds the half-precision number.
// The destination of the .f16 forms and the source of the forms that read one are single
// registers numbered like the S banks; the wider operand is a whole register.

/// The bottom half of the source goes to the bottom half of the destination; bit 8 says
/// whether the source is single or double.
#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VCVTB_to_f16_t1(opcode: u32) -> Instruction {
    decode_vcvt_half(opcode, false, false)
}

/// The top half of the source goes to the bottom half of the destination.
#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VCVTT_to_f16_t1(opcode: u32) -> Instruction {
    decode_vcvt_half(opcode, false, true)
}

/// The bottom half of the source goes to the whole destination.
#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VCVTB_from_f16_t1(opcode: u32) -> Instruction {
    decode_vcvt_half(opcode, true, false)
}

/// The top half of the source goes to the whole destination.
#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VCVTT_from_f16_t1(opcode: u32) -> Instruction {
    decode_vcvt_half(opcode, true, true)
}

fn decode_vcvt_half(opcode: u32, from_f16: bool, top: bool) -> Instruction {
    let vm = opcode.get_bits(0..4) as u8;
    let m = u8::from(opcode.get_bit(5));
    let sz = opcode.get_bit(8);
    let vd = opcode.get_bits(12..16) as u8;
    let d = u8::from(opcode.get_bit(22));
    let sd = SingleReg::from((vd << 1) | d);
    let sm = SingleReg::from((vm << 1) | m);

    if from_f16 {
        if sz {
            let Some(dd) = DoubleReg::new((d << 4) | vd) else {
                return decode_UDF_t2(opcode);
            };
            if top {
                Instruction::VCVTT_f64_f16 {
                    params: VCVTParamsF64F16 { dd, sm },
                }
            } else {
                Instruction::VCVTB_f64_f16 {
                    params: VCVTParamsF64F16 { dd, sm },
                }
            }
        } else if top {
            Instruction::VCVTT_f32_f16 {
                params: VCVTParamsF32F16 { sd, sm },
            }
        } else {
            Instruction::VCVTB_f32_f16 {
                params: VCVTParamsF32F16 { sd, sm },
            }
        }
    } else if sz {
        let Some(dm) = DoubleReg::new((m << 4) | vm) else {
            return decode_UDF_t2(opcode);
        };
        if top {
            Instruction::VCVTT_f16_f64 {
                params: VCVTParamsF16F64 { sd, dm },
            }
        } else {
            Instruction::VCVTB_f16_f64 {
                params: VCVTParamsF16F64 { sd, dm },
            }
        }
    } else if top {
        Instruction::VCVTT_f16_f32 {
            params: VCVTParamsF16F32 { sd, sm },
        }
    } else {
        Instruction::VCVTB_f16_f32 {
            params: VCVTParamsF16F32 { sd, sm },
        }
    }
}
