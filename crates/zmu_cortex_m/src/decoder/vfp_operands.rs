//!
//! Register operands the VFP decoders share
//!
//! The FPU is FPv5-D16, so an encoding that names D16-D31 is UNDEFINED and decodes as UDF.
//!

use super::decode_UDF_t2;
use crate::core::bits::Bits;
use crate::core::instruction::{Instruction, VAddSubParamsf32, VAddSubParamsf64};
use crate::core::instruction::{VMovRegParamsf32, VMovRegParamsf64};
use crate::core::register::{DoubleReg, SingleReg};
use enum_set::EnumSet;

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
        let (Some(dd), Some(dn), Some(dm)) = (
            DoubleReg::new(d << 4 | vd),
            DoubleReg::new(n_low << 4 | n),
            DoubleReg::new(m_low << 4 | vm),
        ) else {
            return decode_UDF_t2(opcode);
        };
        double(VAddSubParamsf64 { dd, dn, dm })
    } else {
        single(VAddSubParamsf32 {
            sd: SingleReg::from(vd << 1 | d),
            sn: SingleReg::from(n << 1 | n_low),
            sm: SingleReg::from(vm << 1 | m_low),
        })
    }
}

/// The same for the two operands (Vd and Vm) of VABS, VNEG, VSQRT, VCMP, VRINT and VMOV.
pub(super) fn decode_two_reg(
    opcode: u32,
    single: impl FnOnce(VMovRegParamsf32) -> Instruction,
    double: impl FnOnce(VMovRegParamsf64) -> Instruction,
) -> Instruction {
    let d = u8::from(opcode.get_bit(22));
    let vd = opcode.get_bits(12..16) as u8;
    let sz = opcode.get_bit(8);
    let m = u8::from(opcode.get_bit(5));
    let vm = opcode.get_bits(0..4) as u8;

    if sz {
        let (Some(dd), Some(dm)) = (DoubleReg::new(d << 4 | vd), DoubleReg::new(m << 4 | vm))
        else {
            return decode_UDF_t2(opcode);
        };
        double(VMovRegParamsf64 { dd, dm })
    } else {
        single(VMovRegParamsf32 {
            sd: SingleReg::from(vd << 1 | d),
            sm: SingleReg::from(vm << 1 | m),
        })
    }
}

/// The `count` double registers from D`first`. `None` when the list is empty or runs past D15.
pub(super) fn double_list(first: u32, count: u32) -> Option<EnumSet<DoubleReg>> {
    if count == 0 {
        return None;
    }
    let mut list = EnumSet::new();
    for number in first..first + count {
        list.insert(DoubleReg::new(u8::try_from(number).ok()?)?);
    }
    Some(list)
}

/// The `count` single registers from S`first`. `None` when the list is empty or runs past S31.
pub(super) fn single_list(first: u32, count: u32) -> Option<EnumSet<SingleReg>> {
    if count == 0 || first.checked_add(count)? > 32 {
        return None;
    }
    let mut list = EnumSet::new();
    for number in first..first + count {
        list.insert(SingleReg::from(number as u8));
    }
    Some(list)
}
