//! SMMUL, SMMLA and SMMLS, and their R forms (A7.7.146, .144 and .145).
//!
//! The opcodes are written from the diagrams (and checked against llvm-mc); the expected values
//! come from `model`, which does the manual's arithmetic on unbounded integers.

use super::ExecutorHelper;
use super::dsp_test_support::{EDGES32, Rng};
use crate::Processor;
use crate::core::instruction::Instruction;
use crate::core::register::{Apsr, BaseReg, Reg};
use crate::decoder::decode_32;

/// The opcode llvm-mc gives for `name r1, r2, r3` and, with an accumulate, `, r4`.
const OPCODES: [(&str, u32); 6] = [
    ("smmul", 0xfb52_f103),
    ("smmulr", 0xfb52_f113),
    ("smmla", 0xfb52_4103),
    ("smmlar", 0xfb52_4113),
    ("smmls", 0xfb62_4103),
    ("smmlsr", 0xfb62_4113),
];

/// 1111 1011 0101 Rn / Ra Rd 000 R Rm for SMMUL (Ra = 1111) and SMMLA, and 1111 1011 0110 Rn for
/// SMMLS. R is hw2<4>.
fn encode(name: &str, rd: u32, rn: u32, rm: u32, ra: u32) -> u32 {
    let round = u32::from(name.ends_with('r'));
    let (hw1, ra) = match name.trim_end_matches('r') {
        "smmul" => (0xfb50, 0b1111),
        "smmla" => (0xfb50, ra),
        "smmls" => (0xfb60, ra),
        _ => unreachable!(),
    };
    (hw1 | rn) << 16 | ra << 12 | rd << 8 | round << 4 | rm
}

/// The pseudocode: the product of the signed operands, Ra << 32 added or subtracted, 0x80000000
/// added for the R forms, and bits 63:32 taken. Integers are unbounded here, so a sum past 2^63
/// is not wrapped until the bits are taken.
fn model(name: &str, rn: u32, rm: u32, ra: u32) -> u32 {
    let product = i128::from(rn as i32) * i128::from(rm as i32);
    let accumulator = i128::from(ra as i32) << 32;
    let mut result = match name.trim_end_matches('r') {
        "smmul" => product,
        "smmla" => accumulator + product,
        "smmls" => accumulator - product,
        _ => unreachable!(),
    };
    if name.ends_with('r') {
        result += 0x8000_0000;
    }
    ((result >> 32) & 0xffff_ffff) as u32
}

/// Rd, Rn, Rm, Ra: all different, then Rd = Rn, Rd = Rm, Rd = Ra, Rn = Rm, Ra = Rn, Ra = Rm, all
/// the same
const LAYOUTS: [(u32, u32, u32, u32); 8] = [
    (1, 2, 3, 4),
    (2, 2, 3, 4),
    (3, 2, 3, 4),
    (4, 2, 3, 4),
    (1, 2, 2, 4),
    (1, 2, 3, 2),
    (1, 2, 3, 3),
    (2, 2, 2, 2),
];

/// N and C set, Z and V clear, and GE in a pattern: none of them may move, and neither may Q,
/// which is run both set and clear
const FLAGS: u32 = 0xa005_0000;

fn check(
    core: &mut Processor,
    name: &str,
    layout: (u32, u32, u32, u32),
    operands: (u32, u32, u32),
    q: bool,
) {
    let (rd, rn_number, rm_number, ra_number) = layout;
    let (rn, rm, ra) = operands;
    let accumulates = !name.starts_with("smmul");
    // registers that share a number share a value
    let rm = if rm_number == rn_number { rn } else { rm };
    let ra = match ra_number {
        n if n == rn_number => rn,
        n if n == rm_number => rm,
        _ => ra,
    };
    for (register, value) in [(Reg::R0, 0x1111_1111), (Reg::R6, 0x6666_6666)] {
        core.set_r(register, value);
    }
    core.set_r(Reg::from(ra_number as u8), ra);
    core.set_r(Reg::from(rn_number as u8), rn);
    core.set_r(Reg::from(rm_number as u8), rm);
    core.psr.value = FLAGS;
    core.psr.set_q(q);
    core.execute_internal(&decode_32(encode(
        name, rd, rn_number, rm_number, ra_number,
    )))
    .unwrap();
    let expected = model(name, rn, rm, if accumulates { ra } else { 0 });
    let context = format!("{name} {layout:?} rn {rn:#010x} rm {rm:#010x} ra {ra:#010x} q {q}");
    assert_eq!(core.get_r(Reg::from(rd as u8)), expected, "Rd of {context}");
    // no Q, no GE, no flags
    assert_eq!(
        core.psr.value & !0x0800_0000,
        FLAGS,
        "the other flags and GE of {context}"
    );
    assert_eq!(core.psr.get_q(), q, "Q of {context}");
    for (register, value) in [(Reg::R0, 0x1111_1111), (Reg::R6, 0x6666_6666)] {
        assert_eq!(core.get_r(register), value, "{register:?} of {context}");
    }
}

#[test]
fn test_the_opcodes_are_the_manuals_and_llvms_and_decode_to_the_right_name() {
    for (name, opcode) in OPCODES {
        assert_eq!(encode(name, 1, 2, 3, 4), opcode, "{name}");
        let decoded = decode_32(opcode);
        assert!(
            matches!(decoded, Instruction::SMM { .. }),
            "{name}: {decoded:?}"
        );
        let operands = if name.starts_with("smmul") {
            "r1, r2, r3"
        } else {
            "r1, r2, r3, r4"
        };
        assert_eq!(decoded.to_string(), format!("{name} {operands}"));
    }
}

#[test]
fn test_the_edge_values_match_the_model_in_every_layout() {
    let mut core = Processor::new();
    for (name, _) in OPCODES {
        for layout in LAYOUTS {
            for rn in EDGES32 {
                for rm in EDGES32 {
                    for ra in EDGES32 {
                        check(&mut core, name, layout, (rn, rm, ra), rn & 1 == 1);
                    }
                }
            }
        }
    }
}

#[test]
fn test_random_values_and_products_that_round_match_the_model() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let mut core = Processor::new();
    for (name, _) in OPCODES {
        for case in 0..5000 {
            let (mut rn, mut rm, ra) = (rng.next(), rng.next(), rng.next());
            if case % 3 == 0 {
                // 2^k * 2^(31-k) = 2^31, the product whose low word is the rounding constant,
                // with each operand negated or not
                let k = rng.next() % 32;
                let sign = |bit: u32, value: u32| {
                    if bit & 1 == 1 {
                        value.wrapping_neg()
                    } else {
                        value
                    }
                };
                (rn, rm) = (sign(rng.next(), 1 << k), sign(rng.next(), 1 << (31 - k)));
            }
            check(
                &mut core,
                name,
                LAYOUTS[case % LAYOUTS.len()],
                (rn, rm, ra),
                case % 2 == 1,
            );
        }
    }
}

#[test]
fn test_the_worked_examples() {
    // name, Rn, Rm, Ra, Rd
    let cases: [(&str, u32, u32, u32, u32); 14] = [
        // 0x10000 * 0x8000 = 2^31: the high word is 0, and rounding carries into it
        ("smmul", 0x0001_0000, 0x0000_8000, 0, 0),
        ("smmulr", 0x0001_0000, 0x0000_8000, 0, 1),
        // -2^31 floors to -1; with the rounding constant it is 0
        ("smmul", 0xffff_0000, 0x0000_8000, 0, 0xffff_ffff),
        ("smmulr", 0xffff_0000, 0x0000_8000, 0, 0),
        ("smmul", 0x7fff_ffff, 0x7fff_ffff, 0, 0x3fff_ffff),
        ("smmulr", 0x7fff_ffff, 0x7fff_ffff, 0, 0x3fff_ffff),
        ("smmul", 0x8000_0000, 0x8000_0000, 0, 0x4000_0000),
        ("smmla", 0x0001_0000, 0x0001_0000, 5, 6),
        ("smmla", 0x0001_0000, 0x0000_8000, 0, 0),
        ("smmlar", 0x0001_0000, 0x0000_8000, 0, 1),
        // (0x7fffffff << 32) + 2^60 passes 2^63 and wraps: bits 63:32 are 0x8fffffff
        ("smmla", 0x4000_0000, 0x4000_0000, 0x7fff_ffff, 0x8fff_ffff),
        ("smmls", 0x0001_0000, 0x0002_0000, 10, 8),
        // 10 << 32 minus 2^31 is 9.5 << 32, which floors to 9; rounded it is 10
        ("smmls", 0x0001_0000, 0x0000_8000, 10, 9),
        ("smmlsr", 0x0001_0000, 0x0000_8000, 10, 10),
    ];
    for (name, rn, rm, ra, expected) in cases {
        let mut core = Processor::new();
        core.set_r(Reg::R2, rn);
        core.set_r(Reg::R3, rm);
        core.set_r(Reg::R4, ra);
        core.execute_internal(&decode_32(encode(name, 1, 2, 3, 4)))
            .unwrap();
        assert_eq!(
            core.get_r(Reg::R1),
            expected,
            "{name} {rn:#x} {rm:#x} {ra:#x}"
        );
        assert_eq!(model(name, rn, rm, ra), expected, "model of {name}");
    }
}

#[test]
fn test_a_failed_it_condition_skips_the_instruction() {
    let mut core = Processor::new();
    core.set_r(Reg::R1, 0x55);
    core.set_r(Reg::R2, 3);
    core.set_r(Reg::R3, 4);
    core.psr.set_z_bit(true);
    core.set_itstate(((crate::core::condition::Condition::NE.value() as u8) << 4) | 0b1000);
    let outcome = core
        .execute_internal(&decode_32(encode("smmulr", 1, 2, 3, 15)))
        .unwrap();
    assert!(matches!(outcome, super::ExecuteSuccess::NotTaken));
    assert_eq!(core.get_r(Reg::R1), 0x55);
}

fn is_udf(instruction: &Instruction) -> bool {
    matches!(instruction, Instruction::UDF { .. })
}

#[test]
fn test_the_registers_the_manual_forbids_are_undefined() {
    for (name, _) in OPCODES {
        let accumulates = !name.starts_with("smmul");
        for bad in [13, 15] {
            // Rd, Rn and Rm may not be SP or PC
            for opcode in [
                encode(name, bad, 2, 3, 4),
                encode(name, 1, bad, 3, 4),
                encode(name, 1, 2, bad, 4),
            ] {
                assert!(is_udf(&decode_32(opcode)), "{name} {opcode:#010x}");
            }
        }
        if accumulates {
            // Ra may not be SP; for SMMLA, Ra = 1111 is SMMUL, for SMMLS it is UNPREDICTABLE
            assert!(
                is_udf(&decode_32(encode(name, 1, 2, 3, 13))),
                "{name} with Ra = SP"
            );
        }
    }
    assert!(is_udf(&decode_32(encode("smmls", 1, 2, 3, 15))));
    assert!(is_udf(&decode_32(encode("smmlsr", 1, 2, 3, 15))));
    // SMMLA with Ra = PC is SMMUL, a different instruction that is allowed
    assert_eq!(
        decode_32(encode("smmla", 1, 2, 3, 15)).to_string(),
        "smmul r1, r2, r3"
    );
}

#[test]
fn test_the_neighbouring_encodings_are_not_these_instructions() {
    // hw2<7:5> must be 000; 0xfb5x is not SMMUL/SMMLA with a bit set there
    for opcode in [0xfb52_f123, 0xfb52_f143, 0xfb52_f183, 0xfb62_4143] {
        assert!(
            !matches!(decode_32(opcode), Instruction::SMM { .. }),
            "{opcode:#010x}"
        );
    }
}
