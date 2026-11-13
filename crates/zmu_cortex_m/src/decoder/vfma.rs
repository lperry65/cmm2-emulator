use crate::core::{
    bits::Bits,
    instruction::{Instruction, VAddSubParamsf32, VAddSubParamsf64},
    register::{DoubleReg, SingleReg},
};

/// Reads the three register operands of a VFP data-processing instruction (Vd, Vn and Vm with
/// their D, N and M bits) and hands them to `single` or `double` as the size bit says.
pub(super) fn decode_three_reg(
    opcode: u32,
    single: impl FnOnce(VAddSubParamsf32) -> Instruction,
    double: impl FnOnce(VAddSubParamsf64) -> Instruction,
) -> Instruction {
    let d = u8::from(opcode.get_bit(22));
    let n = opcode.get_bits(16..20) as u8;
    let vd = opcode.get_bits(12..16) as u8;
    let sz = opcode.get_bit(8);
    let n_low = u8::from(opcode.get_bit(7));
    let m_low = u8::from(opcode.get_bit(5));
    let vm = opcode.get_bits(0..4) as u8;

    if sz {
        double(VAddSubParamsf64 {
            dd: DoubleReg::from(d << 4 | vd),
            dn: DoubleReg::from(n_low << 4 | n),
            dm: DoubleReg::from(m_low << 4 | vm),
        })
    } else {
        single(VAddSubParamsf32 {
            sd: SingleReg::from(vd << 1 | d),
            sn: SingleReg::from(n << 1 | n_low),
            sm: SingleReg::from(vm << 1 | m_low),
        })
    }
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VFMA_t1(opcode: u32) -> Instruction {
    decode_three_reg(
        opcode,
        |params| Instruction::VFMA_f32 { params },
        |params| Instruction::VFMA_f64 { params },
    )
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VFMS_t1(opcode: u32) -> Instruction {
    decode_three_reg(
        opcode,
        |params| Instruction::VFMS_f32 { params },
        |params| Instruction::VFMS_f64 { params },
    )
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VFNMS_t1(opcode: u32) -> Instruction {
    decode_three_reg(
        opcode,
        |params| Instruction::VFNMS_f32 { params },
        |params| Instruction::VFNMS_f64 { params },
    )
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VFNMA_t1(opcode: u32) -> Instruction {
    decode_three_reg(
        opcode,
        |params| Instruction::VFNMA_f32 { params },
        |params| Instruction::VFNMA_f64 { params },
    )
}
