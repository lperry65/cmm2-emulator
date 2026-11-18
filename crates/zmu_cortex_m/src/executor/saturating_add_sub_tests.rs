//! QADD, QSUB, QDADD and QDSUB (A7.7.102, .109, .106 and .107).
//!
//! The opcodes are written from the diagrams (and checked against llvm-mc); the expected values
//! come from `model`, which is the manual's pseudocode. The manual's operands are `<Rd>, <Rm>,
//! <Rn>`: the result is Rm plus or minus Rn, and Rn is the register in hw1<3:0>.

use super::ExecutorHelper;
use super::dsp_test_support::{EDGES32, Rng};
use crate::Processor;
use crate::core::instruction::Instruction;
use crate::core::register::{Apsr, BaseReg, Reg};
use crate::decoder::decode_32;

/// The opcode llvm-mc gives for `name r1, r2, r3`: Rd = r1, Rm = r2, Rn = r3.
const OPCODES: [(&str, u32); 4] = [
    ("qadd", 0xfa83_f182),
    ("qdadd", 0xfa83_f192),
    ("qsub", 0xfa83_f1a2),
    ("qdsub", 0xfa83_f1b2),
];

/// 1111 1010 1000 Rn / 1111 Rd 10 op2 Rm, with op2 from Table A5-27: 00 QADD, 01 QDADD, 10 QSUB,
/// 11 QDSUB.
fn encode(name: &str, rd: u32, rm: u32, rn: u32) -> u32 {
    let op2 = match name {
        "qadd" => 0b00,
        "qdadd" => 0b01,
        "qsub" => 0b10,
        "qdsub" => 0b11,
        _ => unreachable!(),
    };
    (0xfa80 | rn) << 16 | 0xf080 | rd << 8 | op2 << 4 | rm
}

/// The pseudocode: `SignedSatQ` of the sum or difference, doubling R[n] first (and saturating that)
/// for the two D forms. Returns the result and whether either step saturated.
fn model(name: &str, rm: u32, rn: u32) -> (u32, bool) {
    let (m, n) = (i64::from(rm as i32), i64::from(rn as i32));
    let sat = |value: i64| {
        let kept = value.clamp(-(1 << 31), (1 << 31) - 1);
        (kept, kept != value)
    };
    let (kept, saturated) = match name {
        "qadd" => sat(m + n),
        "qsub" => sat(m - n),
        "qdadd" => {
            let (doubled, first) = sat(2 * n);
            let (kept, second) = sat(m + doubled);
            (kept, first || second)
        }
        "qdsub" => {
            let (doubled, first) = sat(2 * n);
            let (kept, second) = sat(m - doubled);
            (kept, first || second)
        }
        _ => unreachable!(),
    };
    (kept as u32, saturated)
}

/// Rd, Rm, Rn: all different, Rd = Rm, Rd = Rn, Rm = Rn, and all the same
const LAYOUTS: [(u32, u32, u32); 5] = [(1, 2, 3), (2, 2, 3), (3, 2, 3), (1, 3, 3), (4, 4, 4)];

/// N and C set, Z and V clear, and GE in a pattern, so that a field that moved shows
const FLAGS: u32 = 0xa105_0000;

fn check(core: &mut Processor, name: &str, layout: (u32, u32, u32), rm: u32, rn: u32, q: bool) {
    let (rd, rm_number, rn_number) = layout;
    let rn = if rm_number == rn_number { rm } else { rn };
    for (register, value) in [(Reg::R0, 0x1111_1111), (Reg::R5, 0x5555_5555)] {
        core.set_r(register, value);
    }
    core.set_r(Reg::from(rm_number as u8), rm);
    core.set_r(Reg::from(rn_number as u8), rn);
    core.psr.value = FLAGS;
    core.psr.set_q(q);
    core.execute_internal(&decode_32(encode(name, rd, rm_number, rn_number)))
        .unwrap();
    let (expected, saturated) = model(name, rm, rn);
    let context = format!("{name} {layout:?} rm {rm:#010x} rn {rn:#010x} q {q}");
    assert_eq!(core.get_r(Reg::from(rd as u8)), expected, "Rd of {context}");
    // Q is sticky: set by a saturation, and never cleared by an instruction that does not
    assert_eq!(core.psr.get_q(), q || saturated, "Q of {context}");
    assert_eq!(
        core.psr.value & !0x0800_0000,
        FLAGS,
        "the other flags and GE of {context}"
    );
    for (register, value) in [(Reg::R0, 0x1111_1111), (Reg::R5, 0x5555_5555)] {
        assert_eq!(core.get_r(register), value, "{register:?} of {context}");
    }
}

#[test]
fn test_the_opcodes_are_the_manuals_and_llvms_and_decode_to_the_right_name() {
    for (name, opcode) in OPCODES {
        assert_eq!(encode(name, 1, 2, 3), opcode, "{name}");
        let decoded = decode_32(opcode);
        assert!(
            matches!(decoded, Instruction::QSAT { .. }),
            "{name}: {decoded:?}"
        );
        // the manual's order: Rd, Rm, Rn
        assert_eq!(decoded.to_string(), format!("{name} r1, r2, r3"));
    }
}

#[test]
fn test_the_edge_values_match_the_model_in_every_layout_with_q_set_or_clear() {
    let mut core = Processor::new();
    for (name, _) in OPCODES {
        for layout in LAYOUTS {
            for rm in EDGES32 {
                for rn in EDGES32 {
                    for q in [false, true] {
                        check(&mut core, name, layout, rm, rn, q);
                    }
                }
            }
        }
    }
}

#[test]
fn test_random_values_match_the_model() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut core = Processor::new();
    for (name, _) in OPCODES {
        for case in 0..5000 {
            let (rm, rn) = (rng.next(), rng.next());
            check(
                &mut core,
                name,
                LAYOUTS[case % LAYOUTS.len()],
                rm,
                rn,
                rng.next() & 1 == 1,
            );
        }
    }
}

#[test]
fn test_the_worked_examples() {
    // name, Rm, Rn, result, Q set by this instruction
    let cases: [(&str, u32, u32, u32, bool); 16] = [
        ("qadd", 5, 7, 12, false),
        ("qadd", 0x7fff_ffff, 1, 0x7fff_ffff, true),
        ("qadd", 0x8000_0000, 0xffff_ffff, 0x8000_0000, true),
        ("qadd", 0x7fff_fffe, 1, 0x7fff_ffff, false),
        ("qsub", 10, 3, 7, false),
        ("qsub", 0x8000_0000, 1, 0x8000_0000, true),
        ("qsub", 0x7fff_ffff, 0xffff_ffff, 0x7fff_ffff, true),
        // 0 - (-2^31) is 2^31, one over the top
        ("qsub", 0, 0x8000_0000, 0x7fff_ffff, true),
        // Rn doubles first: 2^29 doubled is 2^30, and 2^30 + 2^30 is 2^31, one over
        ("qdadd", 0x4000_0000, 0x2000_0000, 0x7fff_ffff, true),
        ("qdadd", 0x2000_0000, 0x2000_0000, 0x6000_0000, false),
        // the doubling saturates: 2^30 doubled is 2^31, so it is 2^31 - 1
        ("qdadd", 0, 0x4000_0000, 0x7fff_ffff, true),
        // -2^30 doubled is -2^31 exactly, which fits; -2^30 - 1 doubled does not
        ("qdadd", 0, 0xc000_0000, 0x8000_0000, false),
        ("qdadd", 0, 0xbfff_ffff, 0x8000_0000, true),
        // the doubling saturates to 2^31 - 1 (Q), and 0 minus that is -(2^31 - 1)
        ("qdsub", 0, 0x4000_0000, 0x8000_0001, true),
        ("qdsub", 0, 0x8000_0000, 0x7fff_ffff, true),
        ("qdsub", 100, 0x0000_0010, 68, false),
    ];
    for (name, rm, rn, expected, saturated) in cases {
        for q in [false, true] {
            let mut core = Processor::new();
            core.set_r(Reg::R2, rm);
            core.set_r(Reg::R3, rn);
            core.psr.set_q(q);
            core.execute_internal(&decode_32(encode(name, 1, 2, 3)))
                .unwrap();
            assert_eq!(core.get_r(Reg::R1), expected, "{name} {rm:#x} {rn:#x}");
            assert_eq!(
                core.psr.get_q(),
                q || saturated,
                "Q of {name} {rm:#x} {rn:#x} q {q}"
            );
            assert_eq!(
                model(name, rm, rn),
                (expected, saturated),
                "model of {name}"
            );
        }
    }
}

#[test]
fn test_a_failed_it_condition_skips_the_instruction_and_leaves_q() {
    let mut core = Processor::new();
    core.set_r(Reg::R1, 0x55);
    core.set_r(Reg::R2, 0x7fff_ffff);
    core.set_r(Reg::R3, 1);
    core.psr.set_z_bit(true);
    // IT NE with Z set: the condition fails
    core.set_itstate(((crate::core::condition::Condition::NE.value() as u8) << 4) | 0b1000);
    let outcome = core
        .execute_internal(&decode_32(encode("qadd", 1, 2, 3)))
        .unwrap();
    assert!(matches!(outcome, super::ExecuteSuccess::NotTaken));
    assert_eq!(core.get_r(Reg::R1), 0x55);
    assert!(!core.psr.get_q());
}

fn is_udf(instruction: &Instruction) -> bool {
    matches!(instruction, Instruction::UDF { .. })
}

#[test]
fn test_sp_and_pc_as_any_register_are_undefined() {
    for (name, _) in OPCODES {
        assert!(!is_udf(&decode_32(encode(name, 1, 2, 3))), "{name}");
        for bad in [13, 15] {
            for opcode in [
                encode(name, bad, 2, 3),
                encode(name, 1, bad, 3),
                encode(name, 1, 2, bad),
            ] {
                assert!(is_udf(&decode_32(opcode)), "{name} {opcode:#010x}");
            }
        }
    }
}

#[test]
fn test_the_neighbouring_encodings_are_not_this_instruction() {
    // hw2<7:6> = 11 and 00 are other spaces, and hw2<15:12> != 1111 is undefined
    for opcode in [0xfa83_f1c2, 0xfa83_f102] {
        assert!(
            !matches!(decode_32(opcode), Instruction::QSAT { .. }),
            "{opcode:#010x}"
        );
    }
    assert!(is_udf(&decode_32(0xfa83_0182)));
}
