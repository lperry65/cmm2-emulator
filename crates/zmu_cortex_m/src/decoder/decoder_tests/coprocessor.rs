//! The coprocessor space (A5.3.18, Table A5-30): hw1 in 0xEC00 to 0xEFFF and 0xFC00 to 0xFFFF. For
//! a coprocessor other than CP10 and CP11 the table gives MCR, MRC, MCRR, MRRC, CDP, LDC and STC
//! and their `2` forms, and everything else of the space is UNDEFINED; the registers the pages
//! call UNPREDICTABLE decode as UDF. CP10 and CP11 are the FPU: an encoding that is not one of
//! its instructions must not come out as a coprocessor instruction for it.
//!
//! The expected decode comes from `expected`, the table and the pages written out on their own.

use super::*;
use crate::core::instruction::CoprocKind;

#[derive(Debug, PartialEq)]
enum Expected {
    Coprocessor(CoprocKind),
    Undefined,
}

/// Table A5-30 for hw1 and hw2, for a coprocessor number that is not 10 or 11
fn expected(hw1: u32, hw2: u32) -> Expected {
    use CoprocKind::*;
    let second = hw1 >> 12 == 0xf;
    let (p, u, d, w, l) = (
        hw1 >> 8 & 1,
        hw1 >> 7 & 1,
        hw1 >> 6 & 1,
        hw1 >> 5 & 1,
        hw1 >> 4 & 1,
    );
    let (rn, rt) = (hw1 & 0xf, hw2 >> 12);
    let pick = |first: CoprocKind, other: CoprocKind| if second { other } else { first };
    let sp_or_pc = |register: u32| register == 13 || register == 15;
    let (kind, refused) = if hw1 >> 8 & 3 == 3 {
        return Expected::Undefined; // op1 = 11xxxx is not in the table
    } else if hw1 >> 9 & 1 == 0 {
        if p == 0 && u == 0 && w == 0 {
            // op1 = 0001x0 and 0001x1: MCRR and MRRC; with D = 0 it is not allocated
            match (d, l) {
                (0, _) => return Expected::Undefined,
                (_, 0) => (pick(MCRR, MCRR2), sp_or_pc(rt) || sp_or_pc(rn)),
                _ => (pick(MRRC, MRRC2), sp_or_pc(rt) || sp_or_pc(rn) || rt == rn),
            }
        } else if l == 1 {
            // the literal form has the rules of A7.7.40: P = 1 and W = 0
            (pick(LDC, LDC2), rn == 15 && (p == 0 || w == 1))
        } else {
            (pick(STC, STC2), rn == 15)
        }
    } else if hw2 >> 4 & 1 == 0 {
        (pick(CDP, CDP2), false)
    } else if l == 0 {
        (pick(MCR, MCR2), sp_or_pc(rt))
    } else {
        (pick(MRC, MRC2), rt == 13)
    };
    if refused {
        Expected::Undefined
    } else {
        Expected::Coprocessor(kind)
    }
}

fn classify(instruction: &Instruction) -> (Expected, Option<u8>) {
    match instruction {
        Instruction::COPROC { kind, coproc } => (Expected::Coprocessor(*kind), Some(*coproc)),
        Instruction::UDF { thumb32: true, .. } => (Expected::Undefined, None),
        other => panic!("{other:?}"),
    }
}

#[test]
fn test_the_opcodes_are_llvms_and_decode_to_their_kind() {
    // llvm-mc, for p7
    for (opcode, kind) in [
        (0xee22_1793_u32, CoprocKind::MCR),
        (0xfe22_1793, CoprocKind::MCR2),
        (0xee32_1793, CoprocKind::MRC),
        (0xfe32_1793, CoprocKind::MRC2),
        (0xee32_f793, CoprocKind::MRC),
        (0xec42_1713, CoprocKind::MCRR),
        (0xfc42_1713, CoprocKind::MCRR2),
        (0xec52_1713, CoprocKind::MRRC),
        (0xfc52_1713, CoprocKind::MRRC2),
        (0xed92_1702, CoprocKind::LDC),
        (0xfd92_1702, CoprocKind::LDC2),
        (0xedf2_1702, CoprocKind::LDC),
        (0xed82_1702, CoprocKind::STC),
        (0xfc22_1702, CoprocKind::STC2),
        (0xede2_1702, CoprocKind::STC),
        (0xee12_1783, CoprocKind::CDP),
        (0xfe12_1783, CoprocKind::CDP2),
        (0xed9f_1702, CoprocKind::LDC),
    ] {
        assert_eq!(
            decode_32(opcode),
            Instruction::COPROC { kind, coproc: 7 },
            "{opcode:#010x}"
        );
    }
}

#[test]
fn test_mcr_has_bit_4_set_and_cdp_has_it_clear() {
    // the table had MCR with bit 4 clear, which is CDP: CDP decoded as MCR, and real MCR as UDF
    assert_eq!(
        decode_32(0xee22_1783),
        Instruction::COPROC {
            kind: CoprocKind::CDP,
            coproc: 7
        }
    );
    assert_eq!(
        decode_32(0xee22_1793),
        Instruction::COPROC {
            kind: CoprocKind::MCR,
            coproc: 7
        }
    );
}

#[test]
fn test_the_coprocessor_space_decodes_as_the_manual_says_for_every_absent_coprocessor() {
    let mut compared = 0;
    let mut coprocessor_instructions = 0;
    for hw1 in (0xec00..=0xefff_u32).chain(0xfc00..=0xffff) {
        for coproc in (0..10_u32).chain(12..16) {
            for rt in [0_u32, 1, 13, 15] {
                for low in [0x00_u32, 0x10, 0x3f, 0x93, 0xff] {
                    let hw2 = rt << 12 | coproc << 8 | low;
                    let (got, number) = classify(&decode_32(hw1 << 16 | hw2));
                    let want = expected(hw1, hw2);
                    assert_eq!(got, want, "{:#010x}", hw1 << 16 | hw2);
                    if got != Expected::Undefined {
                        assert_eq!(number, Some(coproc as u8), "{:#010x}", hw1 << 16 | hw2);
                        coprocessor_instructions += 1;
                    }
                    compared += 1;
                }
            }
        }
    }
    assert_eq!(compared, 2048 * 14 * 4 * 5);
    assert!(
        coprocessor_instructions > 100_000,
        "{coprocessor_instructions}"
    );
}

#[test]
fn test_the_space_for_cp10_and_cp11_is_the_fp_instructions_or_the_tables_generic_ones() {
    // an FP instruction, UDF, or an encoding no FP instruction matched, which the executor turns
    // into NOCP or UNDEFINSTR by CPACR
    for hw1 in (0xec00..=0xefff_u32).chain(0xfc00..=0xffff) {
        for coproc in [10_u32, 11] {
            for rt in [0_u32, 1, 13, 15] {
                for low in [0x00_u32, 0x10, 0x3f, 0x93, 0xff] {
                    let opcode = hw1 << 16 | rt << 12 | coproc << 8 | low;
                    let instruction = decode_32(opcode);
                    // a coprocessor instruction here is a generic encoding that no FP instruction
                    // matched, and then it is one of the table's
                    if let Instruction::COPROC {
                        kind,
                        coproc: number,
                    } = instruction
                    {
                        assert_eq!(number, coproc as u8);
                        assert_eq!(
                            expected(hw1, rt << 12 | coproc << 8 | low),
                            Expected::Coprocessor(kind)
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn test_the_unpredictable_register_rules_of_each_page() {
    // MCR: Rt = SP or PC. MRC: Rt = SP only (PC is APSR_nzcv). MCRR: SP or PC in either. MRRC: the
    // same and Rt = Rt2. STC: Rn = PC. LDC literal: W = 1.
    let undefined = |opcode: u32| {
        assert!(
            matches!(decode_32(opcode), Instruction::UDF { .. }),
            "{opcode:#010x}"
        );
    };
    let defined = |opcode: u32| {
        assert!(
            matches!(decode_32(opcode), Instruction::COPROC { .. }),
            "{opcode:#010x}"
        );
    };
    undefined(0xee22_d793); // mcr p7, #1, sp, ...
    undefined(0xee22_f793); // mcr p7, #1, pc, ...
    defined(0xee22_e793); // lr
    undefined(0xee32_d793); // mrc with sp
    defined(0xee32_f793); // mrc to apsr_nzcv
    undefined(0xec42_d713); // mcrr Rt = sp
    undefined(0xec4d_1713); // mcrr Rt2 = sp
    undefined(0xec4f_1713); // mcrr Rt2 = pc
    undefined(0xec52_2712 | 0x0000_0001); // mrrc with Rt = Rt2 (r2, r2)
    defined(0xec52_1713);
    undefined(0xed8f_1702); // stc with Rn = pc
    undefined(0xedbf_1702); // ldc literal with write back
    defined(0xed9f_1702);
    undefined(0xec1f_1702); // ldc literal with P = 0 and W = 0, D = 0: not allocated
}

#[test]
fn test_the_unallocated_load_and_store_space_is_undefined() {
    // P = 0, U = 0, W = 0 and D = 0, for LDC and STC, with Rn = r2 and the coprocessor p7
    assert!(matches!(decode_32(0xec02_1702), Instruction::UDF { .. }));
    assert!(matches!(decode_32(0xec12_1702), Instruction::UDF { .. }));
    assert!(matches!(decode_32(0xfc02_1702), Instruction::UDF { .. }));
    // op1 = 11xxxx
    assert!(matches!(decode_32(0xef00_1700), Instruction::UDF { .. }));
    assert!(matches!(decode_32(0xff12_1793), Instruction::UDF { .. }));
}
