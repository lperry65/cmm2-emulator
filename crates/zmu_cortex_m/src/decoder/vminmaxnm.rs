use super::vfp_operands::decode_three_reg;
use crate::core::bits::Bits;
use crate::core::instruction::{Instruction, VMinMaxNumParamsf32, VMinMaxNumParamsf64};

/// VMAXNM and VMINNM share their encoding but for bit 6, which makes the minimum.
#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VMINMAXNM_t1(opcode: u32) -> Instruction {
    let max = !opcode.get_bit(6);
    decode_three_reg(
        opcode,
        |p| Instruction::VMINMAXNM_f32 {
            params: VMinMaxNumParamsf32 {
                sd: p.sd,
                sn: p.sn,
                sm: p.sm,
                max,
            },
        },
        |p| Instruction::VMINMAXNM_f64 {
            params: VMinMaxNumParamsf64 {
                dd: p.dd,
                dn: p.dn,
                dm: p.dm,
                max,
            },
        },
    )
}
