use super::*;

#[test]
fn test_decode_bfc() {
    //  f36f 011f       bfc     r1, #0, #32
    assert_eq!(
        decode_32(0xf36f_011f),
        Instruction::BFC {
            params: BfcParams {
                rd: Reg::R1,
                lsbit: 0,
                msbit: 31,
            }
        }
    );
}

#[test]
fn test_decode_bfi_w() {
    // 0xf363_0407 BFI R4, R3, #0, #8
    assert_eq!(
        decode_32(0xf363_0407),
        Instruction::BFI {
            params: BfiParams {
                rd: Reg::R4,
                rn: Reg::R3,
                lsbit: 0,
                width: 8,
            }
        }
    );
}

#[test]
fn test_decode_clz_w() {
    //0xfab0_f180 -> CLZ R1, R0
    assert_eq!(
        decode_32(0xfab0_f180),
        Instruction::CLZ {
            params: Reg2RdRmParams {
                rd: Reg::R1,
                rm: Reg::R0,
            }
        }
    );
}

#[test]
fn test_decode_sbfx() {
    // SBFX    r3, r3, #0, #1
    assert_eq!(
        decode_32(0xf343_0300),
        Instruction::SBFX {
            params: BfxParams {
                rd: Reg::R3,
                rn: Reg::R3,
                lsb: 0,
                widthminus1: 0,
            }
        }
    );
}

#[test]
fn test_decode_sxth_reg() {
    // SXTH R1,R1
    match decode_16(0xb209) {
        Instruction::SXTH { params, thumb32 } => {
            assert_eq!(params.rd, Reg::R1);
            assert_eq!(params.rm, Reg::R1);
            assert!(!thumb32);
            assert_eq!(params.rotation, 0);
        }
        _ => {
            unreachable!();
        }
    }
}

#[test]
fn test_decode_sxth_w() {
    // SXTH.W R10, R10
    assert_eq!(
        decode_32(0xfa0f_fa8a),
        Instruction::SXTH {
            params: Reg2UsizeParams {
                rd: Reg::R10,
                rm: Reg::R10,
                rotation: 0
            },
            thumb32: true,
        }
    );
}

#[test]
#[cfg(feature = "has-dsp-ext")]
fn test_decode_uadd8() {
    // fa82 f24c       uadd8   r2, r2, ip
    assert_eq!(
        decode_32(0xfa82_f24c),
        Instruction::PARALLEL {
            params: Reg3NoSetFlagsParams {
                rd: Reg::R2,
                rn: Reg::R2,
                rm: Reg::R12,
            },
            op: ParallelOp {
                signed: false,
                result: ParallelResult::Wrap,
                lanes: ParallelLanes::Add8,
            },
        }
    );
}

// Without the DSP extension the UADD8 encoding is not in the decoder table;
// it must fall through to UDF so that non-DSP builds treat it as undefined.
#[test]
#[cfg(not(feature = "has-dsp-ext"))]
fn test_decode_uadd8_without_dsp_ext_is_udf() {
    // fa82 f24c  UADD8 R2, R2, IP  — DSP-only encoding
    match decode_32(0xfa82_f24c) {
        Instruction::UDF { thumb32, .. } => assert!(thumb32),
        other => panic!("expected UDF for UADD8 encoding without DSP extension, got {other:?}"),
    }
}

#[test]
fn test_decode_ubfx() {
    // UBFX R1, R0, #1, #1
    assert_eq!(
        decode_32(0xf3c0_0140),
        Instruction::UBFX {
            params: BfxParams {
                rd: Reg::R1,
                rn: Reg::R0,
                lsb: 1,
                widthminus1: 0,
            }
        }
    );
}

#[test]
#[cfg(feature = "has-dsp-ext")]
fn test_decode_uxtab_() {
    //0xfa54_f480 UXTAB.W R4, R4, R0

    assert_eq!(
        decode_32(0xfa54_f480),
        Instruction::UXTAB {
            params: Reg3UsizeParams {
                rd: Reg::R4,
                rn: Reg::R4,
                rm: Reg::R0,
                rotation: 0
            }
        }
    );
}

#[test]
#[cfg(feature = "has-dsp-ext")]
fn test_decode_extend_and_add() {
    let cases = [
        // uxtah r0, r0, r3
        (0xfa10_f083, "uxtah", Reg::R0, Reg::R0, Reg::R3, 0),
        // uxtah r3, r2, r3, ror #8
        (0xfa12_f393, "uxtah", Reg::R3, Reg::R2, Reg::R3, 8),
        // sxtah r3, r2, r3
        (0xfa02_f383, "sxtah", Reg::R3, Reg::R2, Reg::R3, 0),
        // sxtah r3, r2, r3, ror #16
        (0xfa02_f3a3, "sxtah", Reg::R3, Reg::R2, Reg::R3, 16),
        // sxtab r3, r2, r3, ror #24
        (0xfa42_f3b3, "sxtab", Reg::R3, Reg::R2, Reg::R3, 24),
        // uxtab r3, r2, r3
        (0xfa52_f383, "uxtab", Reg::R3, Reg::R2, Reg::R3, 0),
    ];
    for (opcode, name, rd, rn, rm, rotation) in cases {
        let params = Reg3UsizeParams {
            rd,
            rm,
            rn,
            rotation,
        };
        let expected = match name {
            "uxtah" => Instruction::UXTAH { params },
            "sxtah" => Instruction::SXTAH { params },
            "sxtab" => Instruction::SXTAB { params },
            _ => Instruction::UXTAB { params },
        };
        assert_eq!(decode_32(opcode), expected, "{opcode:#010x}");
    }
}

#[test]
#[cfg(feature = "has-dsp-ext")]
fn test_the_extend_and_add_and_pack_instructions_are_shown_as_assembly() {
    let cases = [
        (0xfa10_f083, "uxtah r0, r0, r3"),
        (0xfa12_f393, "uxtah r3, r2, r3, ror #8"),
        (0xfa02_f3a3, "sxtah r3, r2, r3, ror #16"),
        (0xfa42_f3b3, "sxtab r3, r2, r3, ror #24"),
        (0xfa52_f383, "uxtab r3, r2, r3"),
        (0xeacc_0400, "pkhbt r4, r12, r0"),
        (0xeacc_4460, "pkhtb r4, r12, r0, asr #17"),
        (0xeac2_11e3, "pkhtb r1, r2, r3, asr #7"),
        // An immediate of 0 is a shift by 32 for TB.
        (0xeac2_0123, "pkhtb r1, r2, r3, asr #32"),
        (0xeac2_11c3, "pkhbt r1, r2, r3, lsl #7"),
    ];
    for (opcode, expected) in cases {
        let text = decode_32(opcode).to_string();
        assert!(
            text.eq_ignore_ascii_case(expected),
            "{opcode:#010x}: {text} against {expected}"
        );
    }
}

#[test]
fn test_decode_extend_without_add_is_not_the_extend_and_add() {
    // With Rn = PC the encodings are the plain extends, which have their own decoders.
    // uxth.w r0, r3 and sxtb.w r0, r3
    assert!(matches!(
        decode_32(0xfa1f_f083),
        Instruction::UXTH { thumb32: true, .. }
    ));
    assert!(matches!(
        decode_32(0xfa4f_f083),
        Instruction::SXTB { thumb32: true, .. }
    ));
}

#[test]
#[cfg(feature = "has-dsp-ext")]
fn test_decode_pkhbt_and_pkhtb() {
    let params = |shift_t, shift_n| Reg3ShiftParams {
        rd: Reg::R4,
        rn: Reg::R12,
        rm: Reg::R0,
        shift_t,
        shift_n,
        setflags: SetFlags::False,
    };
    // pkhbt r4, r12, r0
    assert_eq!(
        decode_32(0xeacc_0400),
        Instruction::PKH {
            params: params(SRType::LSL, 0),
            tb_form: false
        }
    );
    // pkhbt r4, r12, r0, lsl #3
    assert_eq!(
        decode_32(0xeacc_04c0),
        Instruction::PKH {
            params: params(SRType::LSL, 3),
            tb_form: false
        }
    );
    // pkhtb r4, r12, r0, asr #17
    assert_eq!(
        decode_32(0xeacc_4460),
        Instruction::PKH {
            params: params(SRType::ASR, 17),
            tb_form: true
        }
    );
    // pkhtb r4, r12, r0, asr #32
    assert_eq!(
        decode_32(0xeacc_0420),
        Instruction::PKH {
            params: params(SRType::ASR, 32),
            tb_form: true
        }
    );
}

#[test]
fn test_decode_uxtb() {
    // UXTB R1,R1
    match decode_16(0xb2c9) {
        Instruction::UXTB { params, thumb32 } => {
            assert_eq!(params.rd, Reg::R1);
            assert_eq!(params.rm, Reg::R1);
            assert!(!thumb32);
            assert_eq!(params.rotation, 0);
        }
        _ => {
            unreachable!();
        }
    }
}

#[test]
fn test_decode_uxtb_w() {
    //0xfa5f_f989 UXTB.W R9, R9
    assert_eq!(
        decode_32(0xfa5f_f989),
        Instruction::UXTB {
            params: Reg2UsizeParams {
                rd: Reg::R9,
                rm: Reg::R9,
                rotation: 0,
            },
            thumb32: true,
        }
    );
}

#[test]
fn test_decode_rev_w() {
    // REV.W R2, R1
    assert_eq!(
        decode_32(0xfa91_f281),
        Instruction::REV {
            params: Reg2RdRmParams {
                rd: Reg::R2,
                rm: Reg::R1,
            },
            thumb32: true,
        }
    );
    // REV16.W R2, R1
    assert_eq!(
        decode_32(0xfa91_f291),
        Instruction::REV16 {
            params: Reg2RdRmParams {
                rd: Reg::R2,
                rm: Reg::R1,
            },
            thumb32: true,
        }
    );
    // REVSH.W R2, R1
    assert_eq!(
        decode_32(0xfa91_f2b1),
        Instruction::REVSH {
            params: Reg2RdRmParams {
                rd: Reg::R2,
                rm: Reg::R1,
            },
            thumb32: true,
        }
    );
}

#[test]
fn test_decode_rbit() {
    // RBIT R2, R1
    assert_eq!(
        decode_32(0xfa91_f2a1),
        Instruction::RBIT {
            params: Reg2RdRmParams {
                rd: Reg::R2,
                rm: Reg::R1,
            }
        }
    );
}

#[test]
fn test_decode_ssat_usat() {
    // SSAT R2, #8, R1
    assert_eq!(
        decode_32(0xf301_0207),
        Instruction::SSAT {
            params: SatParams {
                rd: Reg::R2,
                rn: Reg::R1,
                saturate_to: 8,
                shift_t: SRType::LSL,
                shift_n: 0,
            }
        }
    );
    // USAT R2, #8, R1, ASR #2
    assert_eq!(
        decode_32(0xf3a1_0288),
        Instruction::USAT {
            params: SatParams {
                rd: Reg::R2,
                rn: Reg::R1,
                saturate_to: 8,
                shift_t: SRType::ASR,
                shift_n: 2,
            }
        }
    );
}

// Without the DSP extension the extend-and-add and pack encodings are undefined, like SEL.
#[test]
#[cfg(not(feature = "has-dsp-ext"))]
fn test_decode_extend_and_add_and_pack_without_dsp_ext_are_udf() {
    // uxtab, sxtab, uxtah, sxtah, pkhbt, pkhtb
    for opcode in [
        0xfa54_f480u32,
        0xfa42_f3b3,
        0xfa12_f393,
        0xfa02_f3a3,
        0xeacc_0400,
        0xeacc_4460,
    ] {
        assert!(
            matches!(decode_32(opcode), Instruction::UDF { thumb32: true, .. }),
            "{opcode:#010x}"
        );
    }
}

#[test]
fn test_decode_bitfield_extract_past_bit_31() {
    // ubfx r1, r0, #28, #4 reaches bit 31 and is fine; a width of 5 runs past it and is
    // UNPREDICTABLE, so it decodes as undefined.
    assert_eq!(
        decode_32(0xf3c0_7103),
        Instruction::UBFX {
            params: BfxParams {
                rd: Reg::R1,
                rn: Reg::R0,
                lsb: 28,
                widthminus1: 3,
            }
        }
    );
    assert_eq!(
        decode_32(0xf340_7103),
        Instruction::SBFX {
            params: BfxParams {
                rd: Reg::R1,
                rn: Reg::R0,
                lsb: 28,
                widthminus1: 3,
            }
        }
    );
    assert_udf_32(0xf3c0_7104);
    assert_udf_32(0xf340_7104);
}

#[test]
fn test_decode_bfc_msbit_below_lsbit_is_udf() {
    // bfc r1, #28, #4 is fine; an msbit of 3 under an lsbit of 28 is UNPREDICTABLE.
    assert_eq!(
        decode_32(0xf36f_711f),
        Instruction::BFC {
            params: BfcParams {
                rd: Reg::R1,
                lsbit: 28,
                msbit: 31,
            }
        }
    );
    assert_udf_32(0xf36f_7103);
}

// None of the 36 parallel adds and subtracts is there without the DSP extension.
#[test]
#[cfg(not(feature = "has-dsp-ext"))]
fn test_decode_parallel_add_and_subtract_without_dsp_ext_are_udf() {
    for op1 in [0b000, 0b001, 0b010, 0b100, 0b101, 0b110] {
        for (unsigned, op2) in [(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2)] {
            let opcode = (0xfa82_u32 | op1 << 4) << 16 | 0xf103 | unsigned << 6 | op2 << 4;
            assert!(
                matches!(decode_32(opcode), Instruction::UDF { .. }),
                "{opcode:#010x}"
            );
        }
    }
}

// Neither the saturating adds and subtracts nor the most significant word multiplies are there
// without the DSP extension.
#[test]
#[cfg(not(feature = "has-dsp-ext"))]
fn test_decode_qadd_family_and_smmul_family_without_dsp_ext_are_udf() {
    for opcode in [
        0xfa83_f182, // qadd r1, r2, r3
        0xfa83_f192, // qdadd
        0xfa83_f1a2, // qsub
        0xfa83_f1b2, // qdsub
        0xfb52_f103, // smmul
        0xfb52_f113, // smmulr
        0xfb52_4103, // smmla
        0xfb52_4113, // smmlar
        0xfb62_4103, // smmls
        0xfb62_4113, // smmlsr
    ] {
        assert!(
            matches!(decode_32(opcode), Instruction::UDF { .. }),
            "{opcode:#010x}"
        );
    }
}

// Without the DSP extension, the encodings of issue 77, batch 3 are undefined: SSAT16 and USAT16
// are in the space of SSAT and USAT with `sh = 1` and no shift amount, which is UNDEFINED there
// (A7.7.152, A7.7.213), and not SSAT or USAT with ASR #32.
#[test]
#[cfg(not(feature = "has-dsp-ext"))]
fn test_decode_batch3_without_dsp_ext_are_udf() {
    for opcode in [
        0xfb72_f103u32, // usad8 r1, r2, r3
        0xfb72_4103,    // usada8 r1, r2, r3, r4
        0xfa2f_f183,    // sxtb16 r1, r3
        0xfa22_f183,    // sxtab16 r1, r2, r3
        0xfa3f_f193,    // uxtb16 r1, r3, ror #8
        0xfa32_f193,    // uxtab16 r1, r2, r3, ror #8
        0xf322_010f,    // ssat16 r1, #16, r2
        0xf3a2_010f,    // usat16 r1, #15, r2
        0xfb32_f103,    // smulwb r1, r2, r3
        0xfb32_4113,    // smlawt r1, r2, r3, r4
        0xfbc3_1284,    // smlalbb r1, r2, r3, r4
        0xfbc3_12b4,    // smlaltt
        0xfb22_f103,    // smuad r1, r2, r3
        0xfb22_4113,    // smladx
        0xfb42_f113,    // smusdx
        0xfb42_4103,    // smlsd
        0xfbc3_12d4,    // smlaldx
        0xfbd3_12c4,    // smlsld
        0xfbe3_1264,    // umaal r1, r2, r3, r4
    ] {
        assert_udf_32(opcode);
    }
}

#[test]
#[cfg(feature = "has-dsp-ext")]
fn test_decode_ssat16_and_usat16_are_not_ssat_and_usat_with_asr_32() {
    use crate::core::instruction::Sat16Params;
    assert_eq!(
        decode_32(0xf322_010f),
        Instruction::SAT16 {
            params: Sat16Params {
                rd: Reg::R1,
                rn: Reg::R2,
                saturate_to: 16,
            },
            signed: true,
        }
    );
    assert_eq!(
        decode_32(0xf3a2_0100),
        Instruction::SAT16 {
            params: Sat16Params {
                rd: Reg::R1,
                rn: Reg::R2,
                saturate_to: 0,
            },
            signed: false,
        }
    );
}

// With or without the extension, SSAT and USAT with `sh = 1` and no shift amount, that are not
// SSAT16 and USAT16 (sat_imm bit 4 set, which the SSAT and USAT table patterns let through to
// the decoder's refusal), are undefined, not an ASR #32.
#[test]
fn test_decode_ssat_usat_with_sh_and_no_amount_are_not_asr_32() {
    for opcode in [0xf322_011fu32, 0xf322_0117, 0xf3a2_0110, 0xf3a2_011f] {
        assert_udf_32(opcode);
    }
}
