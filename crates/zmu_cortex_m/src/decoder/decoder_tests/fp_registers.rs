use super::*;
use crate::core::instruction::VPushPopParams;
use enum_set::EnumSet;

// The FPU is FPv5-D16: D16-D31 do not exist. An encoding that names one of them (through the D,
// N or M bit) is UNDEFINED, and does not alias onto D0-D15. Each row is the D32 encoding and the
// same instruction on low registers.
const NEEDS_D16_OR_ABOVE: &[(&str, u32, u32)] = &[
    ("vadd.f64 d16, d1, d2", 0xee710b02, 0xee310b02),
    ("vadd.f64 d0, d17, d2", 0xee310b82, 0xee310b02),
    ("vadd.f64 d0, d1, d18", 0xee310b22, 0xee310b02),
    ("vsub.f64 d16, d1, d2", 0xee710b42, 0xee310b42),
    ("vmul.f64 d0, d17, d2", 0xee210b82, 0xee210b02),
    ("vnmul.f64 d0, d1, d18", 0xee210b62, 0xee210b42),
    ("vdiv.f64 d16, d1, d2", 0xeec10b02, 0xee810b02),
    ("vfma.f64 d0, d17, d2", 0xeea10b82, 0xeea10b02),
    ("vfms.f64 d0, d1, d18", 0xeea10b62, 0xeea10b42),
    ("vfnma.f64 d16, d1, d2", 0xeed10b42, 0xee910b42),
    ("vfnms.f64 d0, d17, d2", 0xee910b82, 0xee910b02),
    ("vabs.f64 d16, d1", 0xeef00bc1, 0xeeb00bc1),
    ("vabs.f64 d0, d17", 0xeeb00be1, 0xeeb00bc1),
    ("vneg.f64 d16, d1", 0xeef10b41, 0xeeb10b41),
    ("vneg.f64 d0, d17", 0xeeb10b61, 0xeeb10b41),
    ("vsqrt.f64 d16, d1", 0xeef10bc1, 0xeeb10bc1),
    ("vsqrt.f64 d0, d17", 0xeeb10be1, 0xeeb10bc1),
    ("vcmp.f64 d16, d1", 0xeef40b41, 0xeeb40b41),
    ("vcmp.f64 d0, d17", 0xeeb40b61, 0xeeb40b41),
    ("vcmpe.f64 d16, d1", 0xeef40bc1, 0xeeb40bc1),
    ("vcmp.f64 d16, #0", 0xeef50b40, 0xeeb50b40),
    ("vmov.f64 d16, #1.0", 0xeef70b00, 0xeeb70b00),
    ("vmov.f64 d16, d1", 0xeef00b41, 0xeeb00b41),
    ("vmov.f64 d0, d17", 0xeeb00b61, 0xeeb00b41),
    ("vmov r0, r1, d16", 0xec510b30, 0xec510b10),
    ("vmov d16, r0, r1", 0xec410b30, 0xec410b10),
    ("vmov.32 d16[0], r0", 0xee000b90, 0xee000b10),
    ("vmov.32 r0, d17[1]", 0xee310b90, 0xee310b10),
    ("vldr d16, [r0]", 0xedd00b00, 0xed900b00),
    ("vstr d16, [r0]", 0xedc00b00, 0xed800b00),
    ("vldmia r0, {d16, d17}", 0xecd00b04, 0xec900b04),
    ("vldmia r0!, {d16, d17}", 0xecf00b04, 0xecb00b04),
    ("vstmia r0, {d16, d17}", 0xecc00b04, 0xec800b04),
    ("vstmdb r0!, {d16, d17}", 0xed600b04, 0xed200b04),
    ("vpush {d16, d17}", 0xed6d0b04, 0xed2d0b04),
    ("vpop {d16, d17}", 0xecfd0b04, 0xecbd0b04),
    ("vldmia r0, {d14, d15, d16}", 0xec90eb06, 0xec90db06),
    ("vstmia r0, {d15, d16}", 0xec80fb04, 0xec80eb04),
    ("vpush {d14, d15, d16}", 0xed2deb06, 0xed2ddb06),
    ("vpop {d15, d16}", 0xecbdfb04, 0xecbdeb04),
    ("vcvt.f64.f32 d16, s0", 0xeef70ac0, 0xeeb70ac0),
    ("vcvt.f32.f64 s0, d16", 0xeeb70be0, 0xeeb70bc0),
    ("vcvt.s32.f64 s0, d16", 0xeebd0be0, 0xeebd0bc0),
    ("vcvt.u32.f64 s0, d17", 0xeebc0be1, 0xeebc0bc1),
    ("vcvtr.s32.f64 s0, d16", 0xeebd0b60, 0xeebd0b40),
    ("vcvt.f64.s32 d16, s0", 0xeef80bc0, 0xeeb80bc0),
    ("vcvt.f64.u32 d17, s0", 0xeef81b40, 0xeeb81b40),
    ("vcvt.s32.f64 d16, d16, #4", 0xeefe0bce, 0xeebe0bce),
    ("vcvt.f64.u16 d16, d16, #4", 0xeefb0b46, 0xeebb0b46),
    ("vcvta.s32.f64 s0, d16", 0xfebc0be0, 0xfebc0bc0),
    ("vcvtn.u32.f64 s0, d16", 0xfebd0b60, 0xfebd0b40),
    ("vcvtp.s32.f64 s0, d17", 0xfebe0be1, 0xfebe0bc1),
    ("vcvtm.s32.f64 s0, d16", 0xfebf0be0, 0xfebf0bc0),
    ("vrintz.f64 d16, d1", 0xeef60bc1, 0xeeb60bc1),
    ("vrintr.f64 d0, d17", 0xeeb60b61, 0xeeb60b41),
    ("vrintx.f64 d16, d1", 0xeef70b41, 0xeeb70b41),
    ("vrinta.f64 d16, d1", 0xfef80b41, 0xfeb80b41),
    ("vrintn.f64 d0, d17", 0xfeb90b61, 0xfeb90b41),
    ("vrintp.f64 d16, d1", 0xfefa0b41, 0xfeba0b41),
    ("vrintm.f64 d0, d17", 0xfebb0b61, 0xfebb0b41),
    ("vmaxnm.f64 d16, d1, d2", 0xfec10b02, 0xfe810b02),
    ("vminnm.f64 d0, d17, d2", 0xfe810bc2, 0xfe810b42),
    ("vminnm.f64 d0, d1, d18", 0xfe810b62, 0xfe810b42),
    ("vseleq.f64 d16, d1, d2", 0xfe410b02, 0xfe010b02),
    ("vselvs.f64 d0, d17, d2", 0xfe110b82, 0xfe110b02),
    ("vselge.f64 d0, d1, d18", 0xfe210b22, 0xfe210b02),
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
