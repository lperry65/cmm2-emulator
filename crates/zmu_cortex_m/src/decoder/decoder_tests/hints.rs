//! The 32-bit hint space (A5.3.4, Table A5-14, and A7.7.31, A7.7.32): `1111 0011 1010 1111` then
//! `10x0 xxxx xxxx xxxx`. NOP, YIELD, WFE, WFI, SEV, CSDB and DBG are decoded as themselves, the
//! unallocated encodings execute as NOPs, and an op1 other than 000 is UNDEFINED. None of it is a
//! branch: the hint space has the bits of the conditional branch T3 with a condition of 111x,
//! which the manual leaves out of that branch (Table A5-13, op = not x111xxx).
//!
//! The expected decode comes from `expected`, which is the table written out on its own.

use super::*;

const HW1: u32 = 0xf3af;

fn opcode(hw2: u32) -> u32 {
    HW1 << 16 | hw2
}

#[derive(Debug, PartialEq)]
enum Hint {
    Nop,
    Yield,
    Wfe,
    Wfi,
    Sev,
    Csdb,
    Dbg(u8),
    Undefined,
    Branch,
}

/// Table A5-13 and A5-14 for hw1 = 0xf3af and any hw2. hw2<14:12> is the table's op1: 0x0 here is
/// the hint space (and 010 would be UDF, 0x1 a branch, 1x1 a branch with link; those need other
/// hw1 values to be UDF or are not hints). hw2<13> and hw2<11> are the (0) bits of the diagrams.
fn expected(hw2: u32) -> Hint {
    if hw2 >> 15 == 0 {
        unreachable!("not a branch or miscellaneous control encoding");
    }
    let op1_of_the_branches = hw2 >> 12 & 0b101;
    match op1_of_the_branches {
        0b001 | 0b101 => return Hint::Branch, // B T4 (0x1) and BL (1x1)
        0b100 => return Hint::Undefined,      // 1x0 is not allocated
        _ => {}
    }
    if hw2 >> 8 & 0b111 != 0 {
        return Hint::Undefined; // op1 of the hint table, hw2<10:8>, not 000
    }
    match hw2 & 0xff {
        0x00 => Hint::Nop,
        0x01 => Hint::Yield,
        0x02 => Hint::Wfe,
        0x03 => Hint::Wfi,
        0x04 => Hint::Sev,
        0x14 => Hint::Csdb,
        op2 if op2 >> 4 == 0xf => Hint::Dbg((op2 & 0xf) as u8),
        _ => Hint::Nop, // unallocated hints execute as NOPs
    }
}

fn classify(instruction: &Instruction) -> Hint {
    match instruction {
        Instruction::NOP { thumb32: true } => Hint::Nop,
        Instruction::YIELD { thumb32: true } => Hint::Yield,
        Instruction::WFE { thumb32: true } => Hint::Wfe,
        Instruction::WFI { thumb32: true } => Hint::Wfi,
        Instruction::SEV { thumb32: true } => Hint::Sev,
        Instruction::CSDB => Hint::Csdb,
        Instruction::DBG { option } => Hint::Dbg(*option),
        Instruction::UDF { .. } => Hint::Undefined,
        Instruction::B_t13 { .. } | Instruction::B_t24 { .. } | Instruction::BL { .. } => {
            Hint::Branch
        }
        other => panic!("not a hint, a branch or undefined: {other:?}"),
    }
}

#[test]
fn test_the_opcodes_are_llvms() {
    // llvm-mc: nop.w, yield.w, wfe.w, wfi.w, sev.w, csdb, dbg #0, dbg #15
    for (hw2, hint) in [
        (0x8000, Hint::Nop),
        (0x8001, Hint::Yield),
        (0x8002, Hint::Wfe),
        (0x8003, Hint::Wfi),
        (0x8004, Hint::Sev),
        (0x8014, Hint::Csdb),
        (0x80f0, Hint::Dbg(0)),
        (0x80ff, Hint::Dbg(15)),
    ] {
        assert_eq!(
            classify(&decode_32(opcode(hw2))),
            hint,
            "{:#010x}",
            opcode(hw2)
        );
    }
}

#[test]
fn test_csdb_and_dbg_are_not_branches_and_not_undefined() {
    // the audit's two findings: CSDB decoded as a taken branch, `dbg #0` as UDF
    assert_eq!(decode_32(0xf3af_8014), Instruction::CSDB);
    assert_eq!(decode_32(0xf3af_80f0), Instruction::DBG { option: 0 });
    assert_eq!(decode_32(0xf3af_80f0).to_string(), "dbg #0");
    assert_eq!(decode_32(0xf3af_8014).to_string(), "csdb");
}

#[test]
fn test_every_hw2_of_the_hint_space_decodes_as_the_manual_says() {
    let mut counts = std::collections::BTreeMap::new();
    for hw2 in 0x8000..=0xffff_u32 {
        let want = expected(hw2);
        let got = classify(&decode_32(opcode(hw2)));
        assert_eq!(got, want, "{:#010x}", opcode(hw2));
        *counts
            .entry(format!("{:?}", std::mem::discriminant(&want)))
            .or_insert(0) += 1;
    }
    // every kind of result was met
    assert!(counts.len() >= 9, "{counts:?}");
}

#[test]
fn test_unallocated_hints_are_nops_and_the_allocated_ones_are_not_nops() {
    let nops = (0x8000..=0x80ff_u32)
        .filter(|hw2| matches!(decode_32(opcode(*hw2)), Instruction::NOP { thumb32: true }))
        .count();
    // 256 op2 values, less YIELD, WFE, WFI, SEV, CSDB and the 16 DBGs; NOP itself is one of them
    assert_eq!(nops, 256 - 5 - 16);
}

#[test]
fn test_op1_other_than_zero_is_undefined() {
    for op1 in 1..8_u32 {
        for op2 in [0x00, 0x01, 0x14, 0xf0, 0x55] {
            assert!(
                is_udf(&decode_32(opcode(0x8000 | op1 << 8 | op2))),
                "op1 {op1} op2 {op2:#x}"
            );
        }
    }
}

fn is_udf(instruction: &Instruction) -> bool {
    matches!(instruction, Instruction::UDF { .. })
}

/// B T3 has a condition in hw1<9:6>. Table A5-13 gives the conditional branch the op values that
/// are not x111xxx, so 1110 and 1111 are the hints, MSR, MRS, the barriers and UDF, and never a
/// branch: no opcode with that condition may decode as one.
#[test]
fn test_a_condition_of_111x_is_never_a_conditional_branch() {
    for s in 0..2_u32 {
        for cond in [0b1110_u32, 0b1111] {
            for imm6 in 0..64_u32 {
                let hw1 = 0xf000 | s << 10 | cond << 6 | imm6;
                for hw2 in (0x8000..=0xafff_u32)
                    .step_by(3)
                    .chain([0x8000, 0x8014, 0x80f0, 0xafff])
                {
                    if hw2 >> 12 & 1 == 1 {
                        continue; // bit 12 set is B T4, a branch whatever hw1<9:6> is
                    }
                    let instruction = decode_32(hw1 << 16 | hw2);
                    assert!(
                        !matches!(
                            instruction,
                            Instruction::B_t13 { .. } | Instruction::B_t24 { .. }
                        ),
                        "{:#010x} decoded as {instruction:?}",
                        hw1 << 16 | hw2
                    );
                }
            }
        }
    }
}

#[test]
fn test_the_conditional_branch_with_the_conditions_it_has_is_unchanged() {
    // beq.w +0x100 (llvm-mc: f000 8080) and bne.w: cond 0000 and 0001; and 1101 (ble)
    for (opcode, cond) in [
        (0xf000_8080_u32, Condition::EQ),
        (0xf040_8080, Condition::NE),
        (0xf340_8080, Condition::LE),
    ] {
        match decode_32(opcode) {
            Instruction::B_t13 {
                params,
                thumb32: true,
            } => assert_eq!(params.cond, cond),
            other => panic!("{opcode:#010x}: {other:?}"),
        }
    }
}
