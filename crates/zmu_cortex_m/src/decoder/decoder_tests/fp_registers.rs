use super::*;
use crate::core::instruction::VPushPopParams;
use enum_set::EnumSet;

// The FPU is FPv5-D16: D16-D31 do not exist. An encoding that names one of them (through the D,
// N or M bit) is UNDEFINED, and does not alias onto D0-D15. Each row is the D32 encoding and the
// same instruction on low registers.
const NEEDS_D16_OR_ABOVE: &[(&str, u32, u32)] = &[
    ("vadd.f64 d16, d1, d2", 0xee71_0b02, 0xee31_0b02),
    ("vadd.f64 d0, d17, d2", 0xee31_0b82, 0xee31_0b02),
    ("vadd.f64 d0, d1, d18", 0xee31_0b22, 0xee31_0b02),
    ("vsub.f64 d16, d1, d2", 0xee71_0b42, 0xee31_0b42),
    ("vmul.f64 d0, d17, d2", 0xee21_0b82, 0xee21_0b02),
    ("vnmul.f64 d0, d1, d18", 0xee21_0b62, 0xee21_0b42),
    ("vdiv.f64 d16, d1, d2", 0xeec1_0b02, 0xee81_0b02),
    ("vfma.f64 d0, d17, d2", 0xeea1_0b82, 0xeea1_0b02),
    ("vfms.f64 d0, d1, d18", 0xeea1_0b62, 0xeea1_0b42),
    ("vfnma.f64 d16, d1, d2", 0xeed1_0b42, 0xee91_0b42),
    ("vfnms.f64 d0, d17, d2", 0xee91_0b82, 0xee91_0b02),
    ("vabs.f64 d16, d1", 0xeef0_0bc1, 0xeeb0_0bc1),
    ("vabs.f64 d0, d17", 0xeeb0_0be1, 0xeeb0_0bc1),
    ("vneg.f64 d16, d1", 0xeef1_0b41, 0xeeb1_0b41),
    ("vneg.f64 d0, d17", 0xeeb1_0b61, 0xeeb1_0b41),
    ("vsqrt.f64 d16, d1", 0xeef1_0bc1, 0xeeb1_0bc1),
    ("vsqrt.f64 d0, d17", 0xeeb1_0be1, 0xeeb1_0bc1),
    ("vcmp.f64 d16, d1", 0xeef4_0b41, 0xeeb4_0b41),
    ("vcmp.f64 d0, d17", 0xeeb4_0b61, 0xeeb4_0b41),
    ("vcmpe.f64 d16, d1", 0xeef4_0bc1, 0xeeb4_0bc1),
    ("vcmp.f64 d16, #0", 0xeef5_0b40, 0xeeb5_0b40),
    ("vmov.f64 d16, #1.0", 0xeef7_0b00, 0xeeb7_0b00),
    ("vmov.f64 d16, d1", 0xeef0_0b41, 0xeeb0_0b41),
    ("vmov.f64 d0, d17", 0xeeb0_0b61, 0xeeb0_0b41),
    ("vmov r0, r1, d16", 0xec51_0b30, 0xec51_0b10),
    ("vmov d16, r0, r1", 0xec41_0b30, 0xec41_0b10),
    ("vmov.32 d16[0], r0", 0xee00_0b90, 0xee00_0b10),
    ("vmov.32 r0, d17[1]", 0xee31_0b90, 0xee31_0b10),
    ("vldr d16, [r0]", 0xedd0_0b00, 0xed90_0b00),
    ("vstr d16, [r0]", 0xedc0_0b00, 0xed80_0b00),
    ("vldmia r0, {d16, d17}", 0xecd0_0b04, 0xec90_0b04),
    ("vldmia r0!, {d16, d17}", 0xecf0_0b04, 0xecb0_0b04),
    ("vstmia r0, {d16, d17}", 0xecc0_0b04, 0xec80_0b04),
    ("vstmdb r0!, {d16, d17}", 0xed60_0b04, 0xed20_0b04),
    ("vpush {d16, d17}", 0xed6d_0b04, 0xed2d_0b04),
    ("vpop {d16, d17}", 0xecfd_0b04, 0xecbd_0b04),
    ("vldmia r0, {d14, d15, d16}", 0xec90_eb06, 0xec90_db06),
    ("vstmia r0, {d15, d16}", 0xec80_fb04, 0xec80_eb04),
    ("vpush {d14, d15, d16}", 0xed2d_eb06, 0xed2d_db06),
    ("vpop {d15, d16}", 0xecbd_fb04, 0xecbd_eb04),
    ("vcvt.f64.f32 d16, s0", 0xeef7_0ac0, 0xeeb7_0ac0),
    ("vcvt.f32.f64 s0, d16", 0xeeb7_0be0, 0xeeb7_0bc0),
    ("vcvt.s32.f64 s0, d16", 0xeebd_0be0, 0xeebd_0bc0),
    ("vcvt.u32.f64 s0, d17", 0xeebc_0be1, 0xeebc_0bc1),
    ("vcvtr.s32.f64 s0, d16", 0xeebd_0b60, 0xeebd_0b40),
    ("vcvt.f64.s32 d16, s0", 0xeef8_0bc0, 0xeeb8_0bc0),
    ("vcvt.f64.u32 d17, s0", 0xeef8_1b40, 0xeeb8_1b40),
    ("vcvt.s32.f64 d16, d16, #4", 0xeefe_0bce, 0xeebe_0bce),
    ("vcvt.f64.u16 d16, d16, #4", 0xeefb_0b46, 0xeebb_0b46),
    ("vcvta.s32.f64 s0, d16", 0xfebc_0be0, 0xfebc_0bc0),
    ("vcvtn.u32.f64 s0, d16", 0xfebd_0b60, 0xfebd_0b40),
    ("vcvtp.s32.f64 s0, d17", 0xfebe_0be1, 0xfebe_0bc1),
    ("vcvtm.s32.f64 s0, d16", 0xfebf_0be0, 0xfebf_0bc0),
    ("vrintz.f64 d16, d1", 0xeef6_0bc1, 0xeeb6_0bc1),
    ("vrintr.f64 d0, d17", 0xeeb6_0b61, 0xeeb6_0b41),
    ("vrintx.f64 d16, d1", 0xeef7_0b41, 0xeeb7_0b41),
    ("vrinta.f64 d16, d1", 0xfef8_0b41, 0xfeb8_0b41),
    ("vrintn.f64 d0, d17", 0xfeb9_0b61, 0xfeb9_0b41),
    ("vrintp.f64 d16, d1", 0xfefa_0b41, 0xfeba_0b41),
    ("vrintm.f64 d0, d17", 0xfebb_0b61, 0xfebb_0b41),
    ("vmaxnm.f64 d16, d1, d2", 0xfec1_0b02, 0xfe81_0b02),
    ("vminnm.f64 d0, d17, d2", 0xfe81_0bc2, 0xfe81_0b42),
    ("vminnm.f64 d0, d1, d18", 0xfe81_0b62, 0xfe81_0b42),
    ("vseleq.f64 d16, d1, d2", 0xfe41_0b02, 0xfe01_0b02),
    ("vselvs.f64 d0, d17, d2", 0xfe11_0b82, 0xfe11_0b02),
    ("vselge.f64 d0, d1, d18", 0xfe21_0b22, 0xfe21_0b02),
];

fn is_udf(instruction: &Instruction) -> bool {
    matches!(instruction, Instruction::UDF { .. })
}

#[test]
fn test_registers_above_d15_decode_as_udf() {
    for &(asm, high, low) in NEEDS_D16_OR_ABOVE {
        assert!(
            !is_udf(&decode_32(low)),
            "the low form of {asm} ({low:#010x}) should decode"
        );
        assert!(
            is_udf(&decode_32(high)),
            "{asm} ({high:#010x}) should be UDF on a 16-register FPU"
        );
    }
}

#[test]
fn test_register_lists_past_the_last_register_decode_as_udf() {
    // VLDM, VSTM, VPUSH and VPOP: imm8 words from the first register. A list that goes past
    // the 32nd single (or the 16th double) is UNPREDICTABLE, whatever the imm8 is.
    const FORMS: [u32; 4] = [0xec90_0a00, 0xec80_0a00, 0xed2d_0a00, 0xecbd_0a00];
    for form in FORMS {
        for double in [false, true] {
            for d in [0_u32, 1] {
                for vd in [0_u32, 7, 15] {
                    for imm8 in [0_u32, 1, 2, 3, 31, 32, 33, 64, 127, 128, 129, 200, 254, 255] {
                        // an odd count of words in the double form is not VLDM, it is LDC
                        if double && imm8 % 2 == 1 {
                            continue;
                        }
                        let opcode = form | (u32::from(double) << 8) | d << 22 | vd << 12 | imm8;
                        let instruction = decode_32(opcode);
                        let (first, count, limit) = if double {
                            ((d << 4) | vd, imm8 / 2, 16)
                        } else {
                            ((vd << 1) | d, imm8, 32)
                        };
                        let fits = count != 0 && first + count <= limit;
                        assert_eq!(
                            !is_udf(&instruction),
                            fits,
                            "{opcode:#010x}: first {first}, count {count}, limit {limit}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn test_vpush_and_vpop_of_singles_name_the_first_register_as_vd_then_d() {
    // vpush {s16, s17} and vpop {s30, s31}: the first single is Vd:D, not D:Vd
    assert_eq!(
        decode_32(0xed2d_8a02),
        Instruction::VPUSH {
            params: VPushPopParams {
                single_regs: true,
                single_precision_registers: [SingleReg::S16, SingleReg::S17].into_iter().collect(),
                double_precision_registers: EnumSet::new(),
                imm32: 8,
            }
        }
    );
    assert_eq!(
        decode_32(0xecbd_fa02),
        Instruction::VPOP {
            params: VPushPopParams {
                single_regs: true,
                single_precision_registers: [SingleReg::S30, SingleReg::S31].into_iter().collect(),
                double_precision_registers: EnumSet::new(),
                imm32: 8,
            }
        }
    );
}
