use super::vfp_operands::decode_two_reg;
use crate::core::{
    bits::Bits,
    instruction::{
        Instruction, VMovCr2DpParams, VMovCr2Sp2Params, VMovCrScalarParams, VMovCrSpParams,
        VMovImmParams32, VMovImmParams64,
    },
    register::{DoubleReg, Reg, SingleReg},
};

// The encodings the manual calls unpredictable decode as undefined.
fn undefined(opcode: u32) -> Instruction {
    Instruction::UDF {
        opcode: opcode.into(),
        thumb32: true,
        imm32: 0,
    }
}

// Example:
//
// e = 11
// f = 64 - e - 1 = 52
// bits 7654 3210
// ..............
//      0111 1000
//
// sign = 0
// exp = NOT(imm8[6]) : Replicate(imm8[6], e-3) :imm8[5:4]
//     = NOT(1) : Replicate(1, 8) : 11
//     = 0 : 1111 1111 : 11
//     = 0b01111 1111 11
//     = 0x3ff
//
// frac = imm8[3:0] : Zeros(F-4)
//      = imm8[3:0] : Zeros(48)
//      = 1000 : 48 zeroes
//      = 0x8 0000 0000 0000
//
// result = sign : exp : frac
//        = 0 : 0x3ff : 0x8000_000000000
//        = 0x3ff8_000000000000
fn vfpexpand_imm64(imm8: u8) -> u64 {
    let e = 11;
    let f = 64 - e - 1;
    let sign = imm8.get_bit(7);

    // exp is bitwise concatenation of following:
    // NOT(bit 6 of imm8) : replicate 'bit 6' of imm8 "e-3" times) : imm8 bits 5:4

    let bit6_vec = if imm8.get_bit(6) { 0xff } else { 0 };

    let exp: u64 = u64::from(!imm8.get_bit(6)) << (2 + (e - 3))
        | bit6_vec << 2
        | u64::from(imm8.get_bits(4..6));
    // frac is concatenation of: imm8 bits 3:0 : zeroes F-4 times
    let upper = u64::from(imm8.get_bits(0..4));
    let frac: u64 = upper << (f - 4);
    // result is contatenation of sign : exp : frac

    u64::from(sign) << 63 | exp << f | frac
}

fn vfpexpand_imm32(imm8: u8) -> u32 {
    let e = 8;
    let f = 32 - e - 1;
    let sign = imm8.get_bit(7);

    // exp is bitwise concatenation of following:
    // NOT(bit 6 of imm8) : replicate 'bit 6' of imm8 "e-3" times) : imm8 bits 5:4

    let bit6_vec = if imm8.get_bit(6) { 0x1f } else { 0 };

    let exp: u32 = (u32::from(!imm8.get_bit(6))) << (2 + (e - 3))
        | bit6_vec << 2
        | u32::from(imm8.get_bits(4..6));
    // frac is concatenation of: imm8 bits 3:0 : zeroes F-4 times
    let upper = u32::from(imm8.get_bits(0..4));
    let frac: u32 = upper << (f - 4);

    // result is contatenation of sign : exp : frac
    u32::from(sign) << 31 | exp << f | frac
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VMOV_imm(opcode: u32) -> Instruction {
    let sz = opcode.get_bit(8);
    let D = u8::from(opcode.get_bit(22));
    let vd = opcode.get_bits(12..16) as u8;
    let imm4l = opcode.get_bits(0..4) as u8;
    let imm4h = opcode.get_bits(16..20) as u8;
    let imm8 = imm4h << 4 | imm4l;

    if sz {
        let Some(dd) = DoubleReg::new(D << 4 | vd) else {
            return undefined(opcode);
        };
        Instruction::VMOV_imm_64 {
            params: VMovImmParams64 {
                dd,
                imm64: vfpexpand_imm64(imm8),
            },
        }
    } else {
        Instruction::VMOV_imm_32 {
            params: VMovImmParams32 {
                sd: SingleReg::from(vd << 1 | D),
                imm32: vfpexpand_imm32(imm8),
            },
        }
    }
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VMOV_reg(opcode: u32) -> Instruction {
    decode_two_reg(
        opcode,
        |params| Instruction::VMOV_reg_f32 { params },
        |params| Instruction::VMOV_reg_f64 { params },
    )
}

// The 32-bit scalar form both directions share: bit 21 picks the word of the double register,
// which is N:Vn in bits 7 and 19:16, and N set would name D16 or above. Rt can not be SP or PC.
fn scalar_params(opcode: u32) -> Option<VMovCrScalarParams> {
    let rt = Reg::from(opcode.get_bits(12..16) as u8);
    if rt == Reg::PC || rt == Reg::SP || opcode.get_bit(7) {
        return None;
    }
    Some(VMovCrScalarParams {
        rt,
        dd: DoubleReg::new(opcode.get_bits(16..20) as u8)?,
        x: opcode.get_bit(21),
    })
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VMOV_cr_scalar(opcode: u32) -> Instruction {
    match scalar_params(opcode) {
        Some(params) => Instruction::VMOV_cr_scalar { params },
        None => undefined(opcode),
    }
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VMOV_scalar_cr(opcode: u32) -> Instruction {
    match scalar_params(opcode) {
        Some(params) => Instruction::VMOV_scalar_cr { params },
        None => undefined(opcode),
    }
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VMOV_cr_sp(opcode: u32) -> Instruction {
    let rt = Reg::from(opcode.get_bits(12..16) as u8);

    // rt 15 and 13 are unpredictable:
    if rt == Reg::PC || rt == Reg::SP {
        return undefined(opcode);
    }

    let N = opcode.get_bit(7);
    let vn = (opcode.get_bits(16..20) as u8) << 1;
    let sn = SingleReg::from(vn | u8::from(N));
    Instruction::VMOV_cr_sp {
        params: VMovCrSpParams {
            to_arm_register: opcode.get_bit(20),
            rt,
            sn,
        },
    }
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VMOV_cr2_sp2(opcode: u32) -> Instruction {
    let rt = Reg::from(opcode.get_bits(12..16) as u8);
    let rt2 = Reg::from(opcode.get_bits(16..20) as u8);
    let to_arm_registers = opcode.get_bit(20);
    let m = (opcode.get_bits(0..4) as u8) << 1 | u8::from(opcode.get_bit(5));

    // SP and PC in either register, the same register twice into the core, and the pair that
    // would run on from s31 are unpredictable.
    if [rt, rt2].iter().any(|r| *r == Reg::PC || *r == Reg::SP)
        || (to_arm_registers && rt == rt2)
        || m == 31
    {
        return undefined(opcode);
    }

    Instruction::VMOV_cr2_sp2 {
        params: VMovCr2Sp2Params {
            to_arm_registers,
            rt,
            rt2,
            sm: SingleReg::from(m),
            sm1: SingleReg::from(m + 1),
        },
    }
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VMOV_cr2_dp(opcode: u32) -> Instruction {
    let rt = Reg::from(opcode.get_bits(12..16) as u8);
    let rt2 = Reg::from(opcode.get_bits(16..20) as u8);

    // rt&rt2 15 and 13 are unpredictable:
    if rt == Reg::PC || rt == Reg::SP {
        return undefined(opcode);
    }

    if rt2 == Reg::PC || rt2 == Reg::SP {
        return undefined(opcode);
    }
    let op = opcode.get_bit(20);
    if op && rt == rt2 {
        return undefined(opcode);
    }

    let M = opcode.get_bit(5);
    let vm = opcode.get_bits(0..4) as u8;
    let m = ((u8::from(M)) << 4) + vm;

    let Some(dm) = DoubleReg::new(m) else {
        return undefined(opcode);
    };

    Instruction::VMOV_cr2_dp {
        params: VMovCr2DpParams {
            to_arm_registers: op,
            rt,
            rt2,
            dm,
        },
    }
}
