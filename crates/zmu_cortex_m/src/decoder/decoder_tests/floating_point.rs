use super::*;

use crate::core::fpregister::FPRounding;
use crate::core::instruction::AddressingMode;
use crate::core::instruction::{
    VCVTFixedParams, VMinMaxNumParamsf32, VMinMaxNumParamsf64, VRintParamsf32, VRintParamsf64,
    VSelParamsf64,
};

#[test]
fn test_decode_vabs_32() {
    //eef0 7ae7       vabs.f32        s15, s15

    assert_eq!(
        decode_32(0xeef0_7ae7),
        Instruction::VABS_f32 {
            params: VMovRegParamsf32 {
                sd: SingleReg::S15,
                sm: SingleReg::S15,
            }
        }
    );
}

#[test]
fn test_decode_vneg_f32() {
    // eef1 7a48       vneg.f32        s15, s16

    assert_eq!(
        decode_32(0xeef1_7a48),
        Instruction::VNEG_f32 {
            params: VMovRegParamsf32 {
                sd: SingleReg::S15,
                sm: SingleReg::S16,
            }
        }
    );
}

#[test]
fn test_decode_vneg_f64() {
    // eeb1 7b48       vneg.f64        d7, d8

    assert_eq!(
        decode_32(0xeeb1_7b48),
        Instruction::VNEG_f64 {
            params: VMovRegParamsf64 {
                dd: DoubleReg::D7,
                dm: DoubleReg::D8,
            }
        }
    );
}

#[test]
fn test_decode_vadd_f32() {
    // ee77 5a26       vadd.f32        s11, s14, s13

    assert_eq!(
        decode_32(0xee77_5a26),
        Instruction::VADD_f32 {
            params: VAddSubParamsf32 {
                sd: SingleReg::S11,
                sn: SingleReg::S14,
                sm: SingleReg::S13,
            }
        }
    );
}

#[test]
fn test_decode_vmul_f32() {
    // ee28 caa8       vmul.f32        s24, s17, s17

    assert_eq!(
        decode_32(0xee28_caa8),
        Instruction::VMUL_f32 {
            params: VAddSubParamsf32 {
                sd: SingleReg::S24,
                sn: SingleReg::S17,
                sm: SingleReg::S17,
            }
        }
    );
}

#[test]
fn test_decode_vnmul_f32() {
    // ee20 0ac1       vnmul.f32       s0, s1, s2

    assert_eq!(
        decode_32(0xee20_0ac1),
        Instruction::VNMUL_f32 {
            params: VAddSubParamsf32 {
                sd: SingleReg::S0,
                sn: SingleReg::S1,
                sm: SingleReg::S2,
            }
        }
    );
}

#[test]
fn test_decode_vnmul_f64() {
    // ee28 7b49       vnmul.f64       d7, d8, d9

    assert_eq!(
        decode_32(0xee28_7b49),
        Instruction::VNMUL_f64 {
            params: VAddSubParamsf64 {
                dd: DoubleReg::D7,
                dn: DoubleReg::D8,
                dm: DoubleReg::D9,
            }
        }
    );
}

#[test]
fn test_decode_vdiv_f32() {
    // ee8c da08       vdiv.f32        s26, s24, s16

    assert_eq!(
        decode_32(0xee8c_da08),
        Instruction::VDIV_f32 {
            params: VAddSubParamsf32 {
                sd: SingleReg::S26,
                sn: SingleReg::S24,
                sm: SingleReg::S16,
            }
        }
    );
}

#[test]
fn test_decode_vfma_f32() {
    // eee6 7a87       vfma.f32        s15, s13, s14

    assert_eq!(
        decode_32(0xeee6_7a87),
        Instruction::VFMA_f32 {
            params: VAddSubParamsf32 {
                sd: SingleReg::S15,
                sn: SingleReg::S13,
                sm: SingleReg::S14,
            }
        }
    );
}

#[test]
fn test_decode_vfms_f32() {
    // eee2 7ae6       vfms.f32        s15, s5, s13

    assert_eq!(
        decode_32(0xeee2_7ae6),
        Instruction::VFMS_f32 {
            params: VAddSubParamsf32 {
                sd: SingleReg::S15,
                sn: SingleReg::S5,
                sm: SingleReg::S13,
            }
        }
    );
}

#[test]
fn test_decode_vfnms_f32() {
    // ee92 0a87       vfnms.f32       s0, s5, s14

    assert_eq!(
        decode_32(0xee92_0a87),
        Instruction::VFNMS_f32 {
            params: VAddSubParamsf32 {
                sd: SingleReg::S0,
                sn: SingleReg::S5,
                sm: SingleReg::S14,
            }
        }
    );
}

#[test]
fn test_decode_vcmp_f32() {
    //eef4 7a47       vcmp.f32        s15, s14

    assert_eq!(
        decode_32(0xeef4_7a47),
        Instruction::VCMP_f32 {
            params: VCmpParamsf32 {
                sd: SingleReg::S15,
                sm: SingleReg::S14,
                with_zero: false,
                quiet_nan_exc: false,
            }
        }
    );
}

#[test]
fn test_decode_vct() {
    //eefd 7ac0       vcvt.s32.f32    s15, s0

    assert_eq!(
        decode_32(0xeefd_7ac0),
        Instruction::VCVT {
            params: VCVTParams {
                d: ExtensionReg::Single {
                    reg: SingleReg::S15
                },
                m: ExtensionReg::Single { reg: SingleReg::S0 },
                dp_operation: false,
                to_integer: true,
                unsigned: false,
                rounding: Some(FPRounding::Zero),
            }
        }
    );
}

#[test]
fn test_decode_vcvt_f32_f64() {
    // eeb7 7bc7       vcvt.f32.f64    s14, d7
    assert_eq!(
        decode_32(0xeeb7_7bc7),
        Instruction::VCVT_f32_f64 {
            params: VCVTParamsF32F64 {
                sd: SingleReg::S14,
                dm: DoubleReg::D7,
            }
        }
    );
}

#[test]
fn test_decode_vcvt_f64_f32() {
    // eeb7 3ae3       vcvt.f64.f32    d3, s7
    assert_eq!(
        decode_32(0xeeb7_3ae3),
        Instruction::VCVT_f64_f32 {
            params: VCVTParamsF64F32 {
                dd: DoubleReg::D3,
                sm: SingleReg::S7,
            }
        }
    );
}

#[test]
fn test_decode_vldr() {
    //  ed9f 7b86       vldr    d7, [pc, #536]  ; 448 <_vfprintf_r+0x290>
    assert_eq!(
        decode_32(0xed9f_7b86),
        Instruction::VLDR {
            params: VLoadAndStoreParams {
                dd: ExtensionReg::Double { reg: DoubleReg::D7 },
                rn: Reg::PC,
                add: true,
                imm32: 0x86 << 2,
            }
        }
    );
}

#[test]
fn test_decode_vldr_2() {
    //  eddf 7a23             vldr    s15, [pc, #140] @ 180 <floating_point+0xb0>
    assert_eq!(
        decode_32(0xeddf_7a23),
        Instruction::VLDR {
            params: VLoadAndStoreParams {
                dd: ExtensionReg::Single {
                    reg: SingleReg::S15
                },
                rn: Reg::PC,
                add: true,
                imm32: 140,
            }
        }
    );
}

#[test]
fn test_decode_vstr_single() {
    // edc7 7a19       vstr    s15, [r7, #100]
    assert_eq!(
        decode_32(0xedc7_7a19),
        Instruction::VSTR {
            params: VLoadAndStoreParams {
                dd: ExtensionReg::Single {
                    reg: SingleReg::S15,
                },
                rn: Reg::R7,
                add: true,
                imm32: 100,
            }
        }
    );
}

#[test]
fn test_decode_vldmia_32_ia() {
    // ecb3 7a01       vldmia  r3!, {s14}

    match decode_32(0xecb3_7a01) {
        Instruction::VLDM_T2 { params } => {
            assert_eq!(params.mode, AddressingMode::IncrementAfter);
            let single_regs: Vec<_> = params.list.iter().collect();
            assert_eq!(vec![SingleReg::S14], single_regs);
            assert_eq!(params.imm32, 4);
            assert_eq!(params.rn, Reg::R3);
            assert!(params.write_back);
        }
        _ => unreachable!(),
    }
}

#[test]
fn test_decode_vmov_cr2_dp() {
    //  ec51 0b18       vmov    r0, r1, d8

    assert_eq!(
        decode_32(0xec51_0b18),
        Instruction::VMOV_cr2_dp {
            params: VMovCr2DpParams {
                to_arm_registers: true,
                rt: Reg::R0,
                rt2: Reg::R1,
                dm: DoubleReg::D8,
            }
        }
    );
}

#[test]
fn test_decode_vmov_cr_sp() {
    //  ee09 0a10       vmov    s18, r0

    assert_eq!(
        decode_32(0xee09_0a10),
        Instruction::VMOV_cr_sp {
            params: VMovCrSpParams {
                to_arm_register: false,
                rt: Reg::R0,
                sn: SingleReg::S18,
            }
        }
    );
}

#[test]
fn test_decode_vmov_cr_sp_2() {
    //  ee08 3a90       vmov    s17, r3

    assert_eq!(
        decode_32(0xee08_3a90),
        Instruction::VMOV_cr_sp {
            params: VMovCrSpParams {
                to_arm_register: false,
                rt: Reg::R3,
                sn: SingleReg::S17,
            }
        }
    );
}

#[test]
fn test_decode_vmov_cr_sp_3() {
    //  ee19 0a10       vmov    r0, s18

    assert_eq!(
        decode_32(0xee19_0a10),
        Instruction::VMOV_cr_sp {
            params: VMovCrSpParams {
                to_arm_register: true,
                rt: Reg::R0,
                sn: SingleReg::S18,
            }
        }
    );
}

#[test]
fn test_decode_vmov_imm() {
    // eeb7 6b08       vmov.f64        d6, #120

    assert_eq!(
        decode_32(0xeeb7_6b08),
        Instruction::VMOV_imm_64 {
            params: VMovImmParams64 {
                dd: DoubleReg::D6,
                imm64: 1.5f64.to_bits()
            }
        }
    );
}

#[test]
fn test_decode_vmov_imm_2() {
    //eeff 7a00       vmov.f32        s15, #240       @ 0xbf800000 -1.0

    assert_eq!(
        decode_32(0xeeff_7a00),
        Instruction::VMOV_imm_32 {
            params: VMovImmParams32 {
                sd: SingleReg::S15,
                imm32: 0xbf80_0000 // -1.0
            }
        }
    );
}

#[test]
fn test_decode_vmov_imm_f32() {
    //  eef0 6a00       vmov.f32        s13, #0 @ 0x4000_0000  2.0

    assert_eq!(
        decode_32(0xeef0_6a00),
        Instruction::VMOV_imm_32 {
            params: VMovImmParams32 {
                sd: SingleReg::S13,
                imm32: 2.0f32.to_bits()
            }
        }
    );
}

#[test]
fn test_decode_vmov_reg_f32() {
    //eeb0 0a4a       vmov.f32        s0, s20

    assert_eq!(
        decode_32(0xeeb0_0a4a),
        Instruction::VMOV_reg_f32 {
            params: VMovRegParamsf32 {
                sd: SingleReg::S0,
                sm: SingleReg::S20,
            }
        }
    );
}

#[test]
fn test_decode_vmrs() {
    //0xeef1 fa10       vmrs    APSR_nzcv, fpscr

    assert_eq!(
        decode_32(0xeef1_fa10),
        Instruction::VMRS {
            rt: VMRSTarget::APSRNZCV
        }
    );
}

#[test]
fn test_decode_vpop() {
    //  ecbd 8b06       vpop    {d8-d10}

    match decode_32(0xecbd_8b06) {
        Instruction::VPOP { params } => {
            assert!(!params.single_regs);
            let double_regs: Vec<_> = params.double_precision_registers.iter().collect();
            assert_eq!(
                vec![DoubleReg::D8, DoubleReg::D9, DoubleReg::D10],
                double_regs
            );
            assert_eq!(params.imm32, 3 * 8);
        }
        _ => {
            unreachable!();
        }
    }
}

#[test]
fn test_decode_vpush() {
    //  ed2d 8b02       vpush   {d8}

    match decode_32(0xed2d_8b02) {
        Instruction::VPUSH { params } => {
            assert!(!params.single_regs);
            let double_regs: Vec<_> = params.double_precision_registers.iter().collect();
            assert_eq!(vec![DoubleReg::D8], double_regs);
            assert_eq!(params.imm32, 8);
        }
        _ => {
            unreachable!();
        }
    }
}

#[test]
fn test_decode_vseleq_f32() {
    // fe00 0a81       vseleq.f32      s0, s1, s2
    assert_eq!(
        decode_32(0xfe00_0a81),
        Instruction::VSEL_f32 {
            params: VSelParamsf32 {
                sd: SingleReg::S0,
                sn: SingleReg::S1,
                sm: SingleReg::S2,
                cond: Condition::EQ,
            }
        }
    );
}

#[test]
fn test_decode_vselvs_f32_high_regs() {
    // fe58 fa0f       vselvs.f32      s31, s16, s30
    assert_eq!(
        decode_32(0xfe58_fa0f),
        Instruction::VSEL_f32 {
            params: VSelParamsf32 {
                sd: SingleReg::S31,
                sn: SingleReg::S16,
                sm: SingleReg::S30,
                cond: Condition::VS,
            }
        }
    );
}

#[test]
fn test_decode_vselgt_f32() {
    // fe77 7a27       vselgt.f32      s15, s14, s15
    assert_eq!(
        decode_32(0xfe77_7a27),
        Instruction::VSEL_f32 {
            params: VSelParamsf32 {
                sd: SingleReg::S15,
                sn: SingleReg::S14,
                sm: SingleReg::S15,
                cond: Condition::GT,
            }
        }
    );
}

#[test]
fn test_decode_vselge_f64() {
    // fe28 7b09       vselge.f64      d7, d8, d9
    assert_eq!(
        decode_32(0xfe28_7b09),
        Instruction::VSEL_f64 {
            params: VSelParamsf64 {
                dd: DoubleReg::D7,
                dn: DoubleReg::D8,
                dm: DoubleReg::D9,
                cond: Condition::GE,
            }
        }
    );
}

#[test]
fn test_decode_vselgt_f64() {
    // fe3e fb0d       vselgt.f64      d15, d14, d13
    assert_eq!(
        decode_32(0xfe3e_fb0d),
        Instruction::VSEL_f64 {
            params: VSelParamsf64 {
                dd: DoubleReg::D15,
                dn: DoubleReg::D14,
                dm: DoubleReg::D13,
                cond: Condition::GT,
            }
        }
    );
}

#[test]
fn test_decode_vstm_32_ia() {
    //ecee 7a01       vstmia  lr!, {s15}

    match decode_32(0xecee_7a01) {
        Instruction::VSTM_T2 { params } => {
            assert_eq!(params.mode, AddressingMode::IncrementAfter);
            let single_regs: Vec<_> = params.list.iter().collect();
            assert_eq!(vec![SingleReg::S15], single_regs);
            assert_eq!(params.imm32, 4);
            assert_eq!(params.rn, Reg::LR);
            assert!(params.write_back);
        }
        _ => {
            unreachable!();
        }
    }
}

#[test]
fn test_decode_vstr() {
    //250:       ed8d 7b12       vstr    d7, [sp, #72]   ; 0x48
    assert_eq!(
        decode_32(0xed8d_7b12),
        Instruction::VSTR {
            params: VLoadAndStoreParams {
                dd: ExtensionReg::Double { reg: DoubleReg::D7 },
                rn: Reg::SP,
                add: true,
                imm32: 0x48,
            }
        }
    );
}

#[test]
fn test_decode_vsub_f32() {
    // ee37 5a66       vsub.f32        s10, s14, s13

    assert_eq!(
        decode_32(0xee37_5a66),
        Instruction::VSUB_f32 {
            params: VAddSubParamsf32 {
                sd: SingleReg::S10,
                sn: SingleReg::S14,
                sm: SingleReg::S13,
            }
        }
    );
}

// The sixteen VRINT encodings, each with the rounding it names and whether it signals Inexact.
#[test]
fn test_decode_vrint_f32() {
    let cases = [
        // vrinta.f32 s14, s15
        (0xfeb8_7a67, Some(FPRounding::TieAway), false),
        // vrintn.f32 s14, s15
        (0xfeb9_7a67, Some(FPRounding::TieEven), false),
        // vrintp.f32 s14, s15
        (0xfeba_7a67, Some(FPRounding::PosInf), false),
        // vrintm.f32 s14, s15
        (0xfebb_7a67, Some(FPRounding::NegInf), false),
        // vrintr.f32 s14, s15
        (0xeeb6_7a67, None, false),
        // vrintx.f32 s14, s15
        (0xeeb7_7a67, None, true),
        // vrintz.f32 s14, s15
        (0xeeb6_7ae7, Some(FPRounding::Zero), false),
    ];
    for (opcode, rounding, exact) in cases {
        assert_eq!(
            decode_32(opcode),
            Instruction::VRINT_f32 {
                params: VRintParamsf32 {
                    sd: SingleReg::S14,
                    sm: SingleReg::S15,
                    rounding,
                    exact,
                }
            },
            "{opcode:#010x}"
        );
    }
}

#[test]
fn test_decode_vrint_f64() {
    let cases = [
        // vrinta.f64 d7, d8
        (0xfeb8_7b48, Some(FPRounding::TieAway), false),
        // vrintn.f64 d7, d8
        (0xfeb9_7b48, Some(FPRounding::TieEven), false),
        // vrintp.f64 d7, d8
        (0xfeba_7b48, Some(FPRounding::PosInf), false),
        // vrintm.f64 d7, d8
        (0xfebb_7b48, Some(FPRounding::NegInf), false),
        // vrintr.f64 d7, d8
        (0xeeb6_7b48, None, false),
        // vrintx.f64 d7, d8
        (0xeeb7_7b48, None, true),
        // vrintz.f64 d7, d8
        (0xeeb6_7bc8, Some(FPRounding::Zero), false),
    ];
    for (opcode, rounding, exact) in cases {
        assert_eq!(
            decode_32(opcode),
            Instruction::VRINT_f64 {
                params: VRintParamsf64 {
                    dd: DoubleReg::D7,
                    dm: DoubleReg::D8,
                    rounding,
                    exact,
                }
            },
            "{opcode:#010x}"
        );
    }
}

#[test]
fn test_decode_vrint_with_the_upper_registers() {
    // vrintm.f64 d15, d14 and vrinta.f32 s31, s30
    assert_eq!(
        decode_32(0xfebb_fb4e),
        Instruction::VRINT_f64 {
            params: VRintParamsf64 {
                dd: DoubleReg::D15,
                dm: DoubleReg::D14,
                rounding: Some(FPRounding::NegInf),
                exact: false,
            }
        }
    );
    assert_eq!(
        decode_32(0xfef8_fa4f),
        Instruction::VRINT_f32 {
            params: VRintParamsf32 {
                sd: SingleReg::S31,
                sm: SingleReg::S30,
                rounding: Some(FPRounding::TieAway),
                exact: false,
            }
        }
    );
}

#[test]
fn test_decode_vmaxnm_vminnm() {
    // vmaxnm.f32 s1, s2, s3
    assert_eq!(
        decode_32(0xfec1_0a21),
        Instruction::VMINMAXNM_f32 {
            params: VMinMaxNumParamsf32 {
                sd: SingleReg::S1,
                sn: SingleReg::S2,
                sm: SingleReg::S3,
                max: true,
            }
        }
    );
    // vminnm.f32 s1, s2, s3
    assert_eq!(
        decode_32(0xfec1_0a61),
        Instruction::VMINMAXNM_f32 {
            params: VMinMaxNumParamsf32 {
                sd: SingleReg::S1,
                sn: SingleReg::S2,
                sm: SingleReg::S3,
                max: false,
            }
        }
    );
    // vmaxnm.f64 d1, d2, d3
    assert_eq!(
        decode_32(0xfe82_1b03),
        Instruction::VMINMAXNM_f64 {
            params: VMinMaxNumParamsf64 {
                dd: DoubleReg::D1,
                dn: DoubleReg::D2,
                dm: DoubleReg::D3,
                max: true,
            }
        }
    );
    // vminnm.f64 d1, d2, d3
    assert_eq!(
        decode_32(0xfe82_1b43),
        Instruction::VMINMAXNM_f64 {
            params: VMinMaxNumParamsf64 {
                dd: DoubleReg::D1,
                dn: DoubleReg::D2,
                dm: DoubleReg::D3,
                max: false,
            }
        }
    );
}

#[test]
fn test_decode_vfnma() {
    // vfnma.f32 s15, s12, s10
    assert_eq!(
        decode_32(0xeed6_7a45),
        Instruction::VFNMA_f32 {
            params: VAddSubParamsf32 {
                sd: SingleReg::S15,
                sn: SingleReg::S12,
                sm: SingleReg::S10,
            }
        }
    );
    // vfnma.f64 d1, d2, d3
    assert_eq!(
        decode_32(0xee92_1b43),
        Instruction::VFNMA_f64 {
            params: VAddSubParamsf64 {
                dd: DoubleReg::D1,
                dn: DoubleReg::D2,
                dm: DoubleReg::D3,
            }
        }
    );
}

#[test]
fn test_decode_vcvt_with_a_named_rounding() {
    let single = |reg| ExtensionReg::Single { reg };
    let cases = [
        // vcvta.s32.f32 s1, s2
        (0xfefc_0ac1, FPRounding::TieAway, false, false),
        // vcvta.u32.f32 s1, s2
        (0xfefc_0a41, FPRounding::TieAway, true, false),
        // vcvtn.s32.f32 s1, s2
        (0xfefd_0ac1, FPRounding::TieEven, false, false),
        // vcvtp.s32.f32 s1, s2
        (0xfefe_0ac1, FPRounding::PosInf, false, false),
        // vcvtm.s32.f32 s1, s2
        (0xfeff_0ac1, FPRounding::NegInf, false, false),
    ];
    for (opcode, rounding, unsigned, dp_operation) in cases {
        assert_eq!(
            decode_32(opcode),
            Instruction::VCVT {
                params: VCVTParams {
                    d: single(SingleReg::S1),
                    m: single(SingleReg::S2),
                    dp_operation,
                    to_integer: true,
                    unsigned,
                    rounding: Some(rounding),
                }
            },
            "{opcode:#010x}"
        );
    }
    // vcvtm.u32.f64 s1, d2
    assert_eq!(
        decode_32(0xfeff_0b42),
        Instruction::VCVT {
            params: VCVTParams {
                d: single(SingleReg::S1),
                m: ExtensionReg::Double { reg: DoubleReg::D2 },
                dp_operation: true,
                to_integer: true,
                unsigned: true,
                rounding: Some(FPRounding::NegInf),
            }
        }
    );
}

#[test]
fn test_decode_vcvtr_takes_the_fpscr_rounding() {
    // vcvtr.s32.f32 s15, s0
    assert_eq!(
        decode_32(0xeefd_7a40),
        Instruction::VCVT {
            params: VCVTParams {
                d: ExtensionReg::Single {
                    reg: SingleReg::S15
                },
                m: ExtensionReg::Single { reg: SingleReg::S0 },
                dp_operation: false,
                to_integer: true,
                unsigned: false,
                rounding: None,
            }
        }
    );
}

#[test]
fn test_the_vrint_and_vcvt_encodings_do_not_catch_their_neighbours() {
    // vsel, vmov immediate, vcvt between float and double, vcmp, and the integer vcvt all sit
    // next to the new patterns.
    let neighbours = [
        (0xfe00_0a00u32, "vseleq.f32"),
        (0xeeb7_0a00, "vmov.f32"),
        (0xeeb7_7bc7, "vcvt.f32.f64"),
        (0xeeb4_7a67, "vcmp.f32"),
        (0xeefd_7ac0, "vcvt.s32.f32"),
    ];
    for (opcode, mnemonic) in neighbours {
        let text = decode_32(opcode).to_string();
        assert!(text.starts_with(mnemonic), "{opcode:#010x} -> {text}");
    }
}

#[test]
fn test_decode_vcvt_with_fraction_bits() {
    let single = |reg| ExtensionReg::Single { reg };
    let double = |reg| ExtensionReg::Double { reg };
    let cases = [
        // vcvt.s32.f32 s15, s15, #16
        (0xeefe_7ac8, single(SingleReg::S15), true, false, 32, 16),
        // vcvt.f32.s32 s13, s13, #13
        (0xeefa_6ae9, single(SingleReg::S13), false, false, 32, 13),
        // vcvt.u16.f32 s0, s0, #3
        (0xeebf_0a66, single(SingleReg::S0), true, true, 16, 3),
        // vcvt.s16.f64 d1, d1, #5
        (0xeebe_1b65, double(DoubleReg::D1), true, false, 16, 5),
        // vcvt.f64.u32 d2, d2, #32
        (0xeebb_2bc0, double(DoubleReg::D2), false, true, 32, 32),
        // vcvt.f32.u16 s1, s1, #1
        (0xeefb_0a67, single(SingleReg::S1), false, true, 16, 1),
        // vcvt.s16.f32 s0, s0, #0 and vcvt.u16.f64 d1, d1, #0: no fraction bits is the most
        // the immediate can take away.
        (0xeebe_0a48, single(SingleReg::S0), true, false, 16, 0),
        (0xeebf_1b48, double(DoubleReg::D1), true, true, 16, 0),
        // vcvt.f64.s16 d3, d3, #16
        (0xeeba_3b40, double(DoubleReg::D3), false, false, 16, 16),
    ];
    for (opcode, reg, to_fixed, unsigned, size, fraction_bits) in cases {
        assert_eq!(
            decode_32(opcode),
            Instruction::VCVT_fixed {
                params: VCVTFixedParams {
                    reg,
                    to_fixed,
                    unsigned,
                    size,
                    fraction_bits,
                }
            },
            "{opcode:#010x}"
        );
    }
}

#[test]
fn test_decode_vcvt_with_more_fraction_bits_than_the_size_is_undefined() {
    // A 16-bit conversion with imm5 = 17 would have 16 - 17 fraction bits.
    assert!(matches!(decode_32(0xeebe_0a68), Instruction::UDF { .. }));
}

// The text a trace shows for each of the new instructions, in the assembler's own spelling.
#[test]
fn test_the_new_floating_point_instructions_are_shown_as_assembly() {
    let cases = [
        (0xfeb8_7a67, "vrinta.f32 S14, S15"),
        (0xfeb9_7b48, "vrintn.f64 D7, D8"),
        (0xfeba_7a67, "vrintp.f32 S14, S15"),
        (0xfebb_7b48, "vrintm.f64 D7, D8"),
        (0xeeb6_7a67, "vrintr.f32 S14, S15"),
        (0xeeb7_7b48, "vrintx.f64 D7, D8"),
        (0xeeb6_7ae7, "vrintz.f32 S14, S15"),
        (0xfec1_0a21, "vmaxnm.f32 S1, S2, S3"),
        (0xfe82_1b43, "vminnm.f64 D1, D2, D3"),
        (0xeed6_7a45, "vfnma.f32 S15, S12, S10"),
        (0xee92_1b43, "vfnma.f64 D1, D2, D3"),
        (0xfefc_0ac1, "vcvta.s32.f32 S1, S2"),
        (0xfefd_0b42, "vcvtn.u32.f64 S1, D2"),
        (0xfefe_0ac1, "vcvtp.s32.f32 S1, S2"),
        (0xfeff_0b42, "vcvtm.u32.f64 S1, D2"),
        (0xeefd_7a40, "vcvtr.s32.f32 S15, S0"),
        (0xeefd_7ac0, "vcvt.s32.f32 S15, S0"),
        (0xeefe_7ac8, "vcvt.s32.f32 S15, S15, #16"),
        (0xeefa_6ae9, "vcvt.f32.s32 S13, S13, #13"),
        (0xeebb_2bc0, "vcvt.f64.u32 D2, D2, #32"),
    ];
    for (opcode, expected) in cases {
        let text = decode_32(opcode).to_string();
        assert!(
            text.eq_ignore_ascii_case(expected),
            "{opcode:#010x}: {text} against {expected}"
        );
    }
}
