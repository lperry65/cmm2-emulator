//! SSAT and USAT (A7.7.152 and A7.7.213): the shift forms, the saturation range, and Q.
//!
//! The opcodes come from the diagram (1111 0 (0) 11 0 U 0 sh 0 Rn, 0 imm3 Rd imm2 (0) `sat_imm`,
//! the bit 7 of hw1 being U) and llvm-mc; the expected values from `model`, which clamps an
//! unbounded integer. `saturate16_tests.rs` has SSAT16 and USAT16, which share the encoding space
//! where `sh` is 1 and the shift amount is 0.

use super::dsp_test_support::{
    FLAG_BITS, LANE_EDGES, Q_BIT, Rng, assert_untouched, core_with, is_udf, mixed, run,
    saturate_model,
};
use crate::core::instruction::Instruction;
use crate::core::register::{Apsr, BaseReg, Reg};
use crate::decoder::decode_32;

/// `sh` is 1 for ASR, and the shift amount is imm3:imm2. `field` is `sat_imm`: the bit position
/// less 1 for SSAT, the bit position itself for USAT.
fn encode(unsigned: bool, asr: bool, amount: u32, rd: u32, rn: u32, field: u32) -> u32 {
    let hw1 = 0xf300 | u32::from(unsigned) << 7 | u32::from(asr) << 5 | rn;
    (hw1 << 16) | (amount >> 2) << 12 | rd << 8 | (amount & 3) << 6 | field
}

/// The pseudocode: shift the 32-bit value (the carry is not used), saturate the signed result of
/// that, and say whether it had to be. Q is the second value.
fn model(unsigned: bool, asr: bool, amount: u32, field: u32, rn: u32) -> (u32, bool) {
    let operand = if asr {
        i128::from(rn as i32) >> amount
    } else {
        i128::from((rn << amount) as i32)
    };
    let bits = if unsigned { field } else { field + 1 };
    let (result, saturated) = saturate_model(operand, bits, !unsigned);
    ((result & 0xffff_ffff) as u32, saturated)
}

fn check(unsigned: bool, asr: bool, amount: u32, field: u32, rn_value: u32, flags: u32) {
    let (rd, rn) = if flags & 1 == 1 { (1, 2) } else { (2, 2) };
    let opcode = encode(unsigned, asr, amount, rd, rn, field);
    let mut core = core_with(&[(rn, rn_value)], flags & FLAG_BITS);
    let before = core.r0_12;
    run(&mut core, opcode);
    let (expected, saturated) = model(unsigned, asr, amount, field, rn_value);
    let context = format!("{opcode:#010x} rn {rn_value:#010x} flags {flags:#010x}");
    assert_eq!(core.get_r(Reg::from(rd as u8)), expected, "Rd of {context}");
    let q = if saturated { Q_BIT } else { 0 };
    assert_eq!(
        core.psr.value,
        (flags & FLAG_BITS) | q,
        "flags of {context}"
    );
    assert_untouched(&core, &before, &[rd], &context);
}

#[test]
fn test_the_opcodes_are_the_manuals_and_llvms_and_display_with_their_shift() {
    // llvm-mc: (unsigned, asr, amount, field, opcode, text), with Rd = r1 and Rn = r2
    let cases = [
        (false, false, 0, 0, 0xf302_0100, "ssat r1, #1, r2"),
        (false, false, 0, 31, 0xf302_011f, "ssat r1, #32, r2"),
        (false, false, 3, 7, 0xf302_01c7, "ssat r1, #8, r2, lsl #3"),
        (false, false, 31, 7, 0xf302_71c7, "ssat r1, #8, r2, lsl #31"),
        (false, true, 1, 7, 0xf322_0147, "ssat r1, #8, r2, asr #1"),
        (false, true, 31, 7, 0xf322_71c7, "ssat r1, #8, r2, asr #31"),
        (true, false, 0, 0, 0xf382_0100, "usat r1, #0, r2"),
        (true, false, 0, 31, 0xf382_011f, "usat r1, #31, r2"),
        (true, false, 3, 8, 0xf382_01c8, "usat r1, #8, r2, lsl #3"),
        (true, true, 31, 8, 0xf3a2_71c8, "usat r1, #8, r2, asr #31"),
    ];
    for (unsigned, asr, amount, field, opcode, text) in cases {
        assert_eq!(encode(unsigned, asr, amount, 1, 2, field), opcode, "{text}");
        assert_eq!(decode_32(opcode).to_string(), text, "{opcode:#010x}");
    }
}

#[test]
fn test_every_shift_and_saturation_position_matches_the_model() {
    let mut rng = Rng(0x0bad_cafe_1234_5678);
    for unsigned in [false, true] {
        for asr in [false, true] {
            // ASR #0 is not a shift: sh = 1 with an amount of 0 is SSAT16 and USAT16
            for amount in u32::from(asr)..32 {
                for field in 0..32 {
                    for _ in 0..12 {
                        check(unsigned, asr, amount, field, mixed(&mut rng), rng.next());
                    }
                    // operands on each side of both ends of the range, found by undoing the shift
                    let bits = if unsigned { field } else { field + 1 };
                    let (low, high) = if unsigned {
                        (0i64, (1i64 << bits) - 1)
                    } else {
                        (-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1)
                    };
                    for point in [high - 1, high, high + 1, low - 1, low, low + 1] {
                        let rn = if asr {
                            point << amount
                        } else {
                            point >> amount
                        };
                        check(unsigned, asr, amount, field, rn as u32, rng.next());
                    }
                }
            }
        }
    }
    for edge in LANE_EDGES {
        for field in 0..32 {
            check(false, false, 0, field, edge, 0);
            check(true, false, 0, field, edge, Q_BIT);
        }
    }
}

#[test]
fn test_the_worked_examples() {
    // (unsigned, asr, amount, field, Rn, Rd, Q)
    let examples = [
        (false, false, 0, 7, 127, 127, false),
        (false, false, 0, 7, 128, 127, true),
        (false, false, 0, 7, -128i32 as u32, -128i32 as u32, false),
        (false, false, 0, 7, -129i32 as u32, -128i32 as u32, true),
        // a bit position of 1 is the range -1 to 0
        (false, false, 0, 0, 0, 0, false),
        (false, false, 0, 0, 1, 0, true),
        (false, false, 0, 0, -1i32 as u32, -1i32 as u32, false),
        (false, false, 0, 0, -2i32 as u32, -1i32 as u32, true),
        // 32 is the whole word: nothing saturates
        (false, false, 0, 31, 0x8000_0000, 0x8000_0000, false),
        (false, false, 0, 31, 0x7fff_ffff, 0x7fff_ffff, false),
        (true, false, 0, 8, 255, 255, false),
        (true, false, 0, 8, 256, 255, true),
        (true, false, 0, 8, -1i32 as u32, 0, true),
        // a bit position of 0 is the range 0 to 0
        (true, false, 0, 0, 0, 0, false),
        (true, false, 0, 0, 5, 0, true),
        (true, false, 0, 31, 0x7fff_ffff, 0x7fff_ffff, false),
        (true, false, 0, 31, 0x8000_0000, 0, true),
        // the shift comes first, in 32 bits: 3 << 31 wraps to 0x8000_0000, which is negative
        (false, false, 31, 7, 3, -128i32 as u32, true),
        (false, false, 24, 7, 1, 127, true),
        (false, false, 24, 7, 0, 0, false),
        // ASR keeps the sign: 0x7fff >> 8 = 0x7f is in range, 0x8000 >> 8 = 0x80 is not
        (false, true, 8, 7, 0x7fff, 127, false),
        (false, true, 8, 7, 0x8000, 127, true),
        (false, true, 8, 7, 0x8000_0000, -128i32 as u32, true),
        (false, true, 31, 0, 0x8000_0000, -1i32 as u32, false),
        (true, true, 31, 0, 0x8000_0000, 0, true),
        (true, true, 31, 0, 0x7fff_ffff, 0, false),
    ];
    for (unsigned, asr, amount, field, rn, expected, q) in examples {
        let mut core = core_with(&[(2, rn)], 0);
        run(&mut core, encode(unsigned, asr, amount, 1, 2, field));
        let context = format!("unsigned {unsigned} asr {asr} #{amount} field {field} rn {rn:#x}");
        assert_eq!(core.get_r(Reg::R1), expected, "{context}");
        assert_eq!(core.psr.get_q(), q, "Q of {context}");
        assert_eq!(
            model(unsigned, asr, amount, field, rn),
            (expected, q),
            "{context}"
        );
    }
}

#[test]
fn test_q_is_sticky_and_the_carry_is_not_used() {
    let mut core = core_with(&[(2, 100)], Q_BIT | 0x2000_0000);
    run(&mut core, encode(false, false, 0, 1, 2, 7));
    assert_eq!(core.get_r(Reg::R1), 100);
    // nothing saturated, so Q stays set; the carry is neither read nor written
    assert_eq!(core.psr.value, Q_BIT | 0x2000_0000);
    for carry in [0, 0x2000_0000] {
        // -13 >> 1 = -7, which is in the range of SSAT #4
        let mut core = core_with(&[(2, 0xffff_fff3)], carry);
        run(&mut core, encode(false, true, 1, 1, 2, 3));
        assert_eq!(core.get_r(Reg::R1), 0xffff_fff9);
        assert_eq!(core.psr.value, carry);
    }
}

#[test]
fn test_a_failed_it_condition_skips_the_instruction() {
    use super::ExecutorHelper;
    use crate::core::condition::Condition;
    let mut core = core_with(&[(2, 1000)], 0);
    core.psr.set_z_bit(true);
    core.set_itstate(((Condition::NE.value() as u8) << 4) | 0b1000);
    let outcome = core
        .execute_internal(&decode_32(encode(false, false, 0, 1, 2, 7)))
        .unwrap();
    assert!(matches!(outcome, super::ExecuteSuccess::NotTaken));
    assert_eq!(core.get_r(Reg::R1), super::dsp_test_support::sentinel(1));
    assert!(!core.psr.get_q());
}

#[test]
fn test_the_registers_the_manual_forbids_are_undefined() {
    for unsigned in [false, true] {
        for bad in [13, 15] {
            for opcode in [
                encode(unsigned, false, 0, bad, 2, 7),
                encode(unsigned, false, 3, 1, bad, 7),
                encode(unsigned, true, 3, bad, 2, 7),
            ] {
                assert!(is_udf(&decode_32(opcode)), "{opcode:#010x}");
            }
        }
    }
}

#[test]
fn test_asr_by_zero_is_not_ssat_or_usat() {
    // sh = 1 and an amount of 0 is SSAT16 and USAT16 where the DSP extension is, and undefined
    // where it is not; it is not SSAT or USAT with ASR #32. With sat_imm bit 4 or 5 set, the
    // (0) bits of SSAT16 and USAT16, it is undefined with the extension too.
    for unsigned in [false, true] {
        for field in 0..64 {
            let decoded = decode_32(encode(unsigned, true, 0, 1, 2, field));
            assert!(
                !matches!(decoded, Instruction::SSAT { .. } | Instruction::USAT { .. }),
                "unsigned {unsigned} field {field}: {decoded:?}"
            );
            if field >= 16 {
                assert!(
                    is_udf(&decoded),
                    "unsigned {unsigned} field {field}: {decoded:?}"
                );
            }
        }
        // the nearest shift that is a shift
        assert!(!is_udf(&decode_32(encode(unsigned, true, 1, 1, 2, 7))));
    }
}
