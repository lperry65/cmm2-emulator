use super::vfp_operands::decode_two_reg;
use crate::core::bits::Bits;
use crate::core::instruction::{Instruction, VCmpParamsf32, VCmpParamsf64};

// VCMP and VCMPE of two registers (T1) or of one against zero (T2). Bit 7 makes it VCMPE, which
// signals on a quiet NaN too.
fn decode_vcmp(opcode: u32, with_zero: bool) -> Instruction {
    let quiet_nan_exc = opcode.get_bit(7);

    decode_two_reg(
        opcode,
        |p| Instruction::VCMP_f32 {
            params: VCmpParamsf32 {
                sd: p.sd,
                sm: p.sm,
                quiet_nan_exc,
                with_zero,
            },
        },
        |p| Instruction::VCMP_f64 {
            params: VCmpParamsf64 {
                dd: p.dd,
                dm: p.dm,
                quiet_nan_exc,
                with_zero,
            },
        },
    )
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VCMP_t1(opcode: u32) -> Instruction {
    decode_vcmp(opcode, false)
}

#[allow(non_snake_case)]
#[inline(always)]
pub fn decode_VCMP_t2(opcode: u32) -> Instruction {
    decode_vcmp(opcode, true)
}
