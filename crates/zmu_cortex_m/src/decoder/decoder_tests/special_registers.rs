//! MSR (A7.7.83, B5.2.3) and MRS (A7.7.82, B5.2.2): the encodings the pages call UNPREDICTABLE
//! decode as UDF. MSR: mask 00, a mask other than 10 for a `SYSm` outside 0 to 3, Rn = SP or PC,
//! and a `SYSm` outside 0 to 3, 5 to 9 and 16 to 20 (16 and 20 only on Armv6-M). MRS: Rd = SP or
//! PC and the same `SYSm` set.
//!
//! The expected decode comes from `valid`, the rule of the pages written out on its own, over
//! every Rn or Rd, mask and `SYSm`.

use super::*;

/// `UInt(SYSm) IN {0..3,5..9,16..20}`
#[cfg(not(feature = "armv6m"))]
fn sysm_is_allocated(sysm: u32) -> bool {
    matches!(sysm, 0..=3 | 5..=9 | 16..=20)
}

#[cfg(feature = "armv6m")]
fn sysm_is_allocated(sysm: u32) -> bool {
    matches!(sysm, 0..=3 | 5..=9 | 16 | 20)
}

/// how many `SYSm` above 3 are allocated
const HIGH_SYSM: u32 = if cfg!(feature = "armv6m") { 7 } else { 10 };

fn msr(rn: u32, mask: u32, sysm: u32) -> u32 {
    (0xf380 | rn) << 16 | 0x8000 | mask << 10 | sysm
}

fn mrs(rd: u32, sysm: u32) -> u32 {
    0xf3ef_8000 | rd << 8 | sysm
}

#[test]
fn test_the_opcodes_are_llvms() {
    // llvm-mc: msr apsr_nzcvq, r1 / msr msp, r1 / msr control, r1 / mrs r1, apsr / mrs r1, control
    // / msr basepri_max, r2
    assert_eq!(msr(1, 0b10, 0), 0xf381_8800);
    assert_eq!(msr(1, 0b10, 8), 0xf381_8808);
    assert_eq!(msr(1, 0b10, 20), 0xf381_8814);
    assert_eq!(mrs(1, 0), 0xf3ef_8100);
    assert_eq!(mrs(1, 20), 0xf3ef_8114);
    assert_eq!(msr(2, 0b10, 18), 0xf382_8812);
}

#[test]
fn test_msr_over_every_register_mask_and_sysm() {
    let mut valid = 0;
    for rn in 0..16_u32 {
        for mask in 0..4_u32 {
            for sysm in 0..256_u32 {
                let opcode = msr(rn, mask, sysm);
                // B5.2.3: mask == '00' || (mask != '10' && !(SYSm IN {0..3})), and
                // n IN {13,15} || !(SYSm IN {0..3,5..9,16..20})
                let unpredictable = mask == 0
                    || (mask != 0b10 && sysm > 3)
                    || rn == 13
                    || rn == 15
                    || !sysm_is_allocated(sysm);
                match decode_32(opcode) {
                    Instruction::UDF { .. } => assert!(unpredictable, "{opcode:#010x}"),
                    Instruction::MSR_reg { params } => {
                        assert!(!unpredictable, "{opcode:#010x}");
                        assert_eq!(params.rn, Reg::from(rn as u8), "{opcode:#010x}");
                        assert_eq!(u32::from(params.mask), mask, "{opcode:#010x}");
                        assert_eq!(u32::from(params.sysm), sysm, "{opcode:#010x}");
                        valid += 1;
                    }
                    other => panic!("{opcode:#010x}: {other:?}"),
                }
            }
        }
    }
    // 14 registers; SYSm 0 to 3 take 3 masks, the 13 others of 5 to 9 and 16 to 20 take only 10
    assert_eq!(valid, 14 * (4 * 3 + HIGH_SYSM));
}

#[test]
fn test_mrs_over_every_register_and_sysm() {
    let mut valid = 0;
    for rd in 0..16_u32 {
        for sysm in 0..256_u32 {
            let opcode = mrs(rd, sysm);
            // B5.2.2: d IN {13,15} || !(SYSm IN {0..3,5..9,16..20})
            let unpredictable = rd == 13 || rd == 15 || !sysm_is_allocated(sysm);
            match decode_32(opcode) {
                Instruction::UDF { .. } => assert!(unpredictable, "{opcode:#010x}"),
                Instruction::MRS { params } => {
                    assert!(!unpredictable, "{opcode:#010x}");
                    assert_eq!(params.rd, Reg::from(rd as u8), "{opcode:#010x}");
                    assert_eq!(u32::from(params.sysm), sysm, "{opcode:#010x}");
                    valid += 1;
                }
                other => panic!("{opcode:#010x}: {other:?}"),
            }
        }
    }
    assert_eq!(valid, 14 * (4 + HIGH_SYSM));
}

#[test]
fn test_the_cases_the_batch_1_review_found() {
    // reserved SYSm 10 (0xf380880a) with mask 10 and Rn = r0: UNPREDICTABLE
    assert!(matches!(decode_32(0xf380_880a), Instruction::UDF { .. }));
    // msr msp, sp and msr msp, pc
    assert!(matches!(decode_32(0xf38d_8808), Instruction::UDF { .. }));
    assert!(matches!(decode_32(0xf38f_8808), Instruction::UDF { .. }));
    // mrs sp, msp and mrs pc, msp
    assert!(matches!(decode_32(0xf3ef_8d08), Instruction::UDF { .. }));
    assert!(matches!(decode_32(0xf3ef_8f08), Instruction::UDF { .. }));
    // and the ordinary ones are as they were
    assert!(matches!(
        decode_32(0xf380_8808),
        Instruction::MSR_reg { .. }
    ));
    assert!(matches!(decode_32(0xf3ef_8008), Instruction::MRS { .. }));
}
