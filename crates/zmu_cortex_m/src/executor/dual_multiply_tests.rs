//! SMUAD, SMUSD, SMLAD and SMLSD with their X forms (A7.7.147, .151, .137 and .142): two signed
//! 16 x 16 products, added or subtracted, with an accumulate for SMLAD and SMLSD.
//!
//! The opcodes come from the diagrams (1111 1011 0 op1 Rn, Ra Rd 000 M Rm, with op1 = 010 for
//! the adds and 100 for the subtracts) and llvm-mc, and the expected values from `model`, which
//! works on unbounded integers.

use super::dsp_test_support::{
    Case, LAYOUTS, Q_BIT, assert_untouched, cases, core_with, is_udf, run,
};
use crate::Processor;
use crate::core::condition::Condition;
use crate::core::register::{Apsr, BaseReg, Reg};
use crate::decoder::decode_32;

/// SMUAD and SMUSD are SMLAD and SMLSD with Ra = 1111
const NO_ACCUMULATE: u32 = 15;

fn encode(subtract: bool, rd: u32, rn: u32, rm: u32, ra: u32, swap: bool) -> u32 {
    let hw1 = if subtract { 0xfb40 } else { 0xfb20 } | rn;
    hw1 << 16 | ra << 12 | rd << 8 | u32::from(swap) << 4 | rm
}

/// The pseudocode: the products of the bottom halfwords and of the top halfwords (Rm's rotated by
/// 16 for the X forms), their sum or difference, and Ra added. Q is set when the sum is not the
/// 32-bit signed number that is stored.
fn model(subtract: bool, rn: u32, rm: u32, ra: Option<u32>, swap: bool) -> (u32, bool) {
    let operand2 = if swap { rm.rotate_right(16) } else { rm };
    let half = |value: u32, shift: u32| i128::from((value >> shift) as u16 as i16);
    let product1 = half(rn, 0) * half(operand2, 0);
    let product2 = half(rn, 16) * half(operand2, 16);
    let mut result = if subtract {
        product1 - product2
    } else {
        product1 + product2
    };
    if let Some(ra) = ra {
        result += i128::from(ra as i32);
    }
    let rd = (result & 0xffff_ffff) as u32;
    (rd, result != i128::from(rd as i32))
}

fn check(core: &mut Processor, subtract: bool, accumulate: bool, swap: bool, case: &Case) {
    let [rd, rn_number, rm_number, ra_number] = case.layout;
    let ra_field = if accumulate { ra_number } else { NO_ACCUMULATE };
    let opcode = encode(subtract, rd, rn_number, rm_number, ra_field, swap);
    case.load(core);
    let before = core.r0_12;
    let (rn, rm, ra) = (
        core.get_r(Reg::from(rn_number as u8)),
        core.get_r(Reg::from(rm_number as u8)),
        core.get_r(Reg::from(ra_number as u8)),
    );
    run(core, opcode);
    let (expected, overflow) = model(subtract, rn, rm, accumulate.then_some(ra), swap);
    let context = format!("{opcode:#010x} {}", case.describe());
    assert_eq!(core.get_r(Reg::from(rd as u8)), expected, "Rd of {context}");
    // SMUSD cannot overflow, and the others only set Q, never clear it
    let q = if overflow { Q_BIT } else { 0 };
    assert_eq!(core.psr.value, case.psr | q, "flags of {context}");
    assert_untouched(core, &before, &[rd], &context);
}

#[test]
fn test_the_opcodes_are_the_manuals_and_llvms_and_decode_to_the_right_name() {
    // llvm-mc, with r1, r2, r3 and, for the accumulating forms, r4
    let cases = [
        (false, false, false, 0xfb22_f103, "smuad r1, r2, r3"),
        (false, false, true, 0xfb22_f113, "smuadx r1, r2, r3"),
        (true, false, false, 0xfb42_f103, "smusd r1, r2, r3"),
        (true, false, true, 0xfb42_f113, "smusdx r1, r2, r3"),
        (false, true, false, 0xfb22_4103, "smlad r1, r2, r3, r4"),
        (false, true, true, 0xfb22_4113, "smladx r1, r2, r3, r4"),
        (true, true, false, 0xfb42_4103, "smlsd r1, r2, r3, r4"),
        (true, true, true, 0xfb42_4113, "smlsdx r1, r2, r3, r4"),
    ];
    for (subtract, accumulate, swap, opcode, text) in cases {
        let ra = if accumulate { 4 } else { NO_ACCUMULATE };
        assert_eq!(encode(subtract, 1, 2, 3, ra, swap), opcode, "{text}");
        assert_eq!(decode_32(opcode).to_string(), text, "{opcode:#010x}");
    }
}

#[test]
fn test_the_edge_and_random_values_match_the_model_in_every_layout() {
    let mut core = Processor::new();
    for case in cases(&LAYOUTS, 4, 2500, 0x4d4d_0f0f_8181_c3c3) {
        for subtract in [false, true] {
            for accumulate in [false, true] {
                for swap in [false, true] {
                    check(&mut core, subtract, accumulate, swap, &case);
                }
            }
        }
    }
}

#[test]
fn test_the_worked_examples() {
    // (subtract, swap, Rn, Rm, Ra, Rd, Q)
    let examples = [
        // 3 * 5 + 2 * 7: the bottom halves with each other, the top halves with each other
        (false, false, 0x0002_0003, 0x0007_0005, None, 29, false),
        // swapped: the bottom of Rn with the top of Rm, and the top with the bottom: 3 * 7 + 2 * 5
        (false, true, 0x0002_0003, 0x0007_0005, None, 31, false),
        // 3 * 5 - 2 * 7
        (true, false, 0x0002_0003, 0x0007_0005, None, 1, false),
        // 3 * 7 - 2 * 5
        (true, true, 0x0002_0003, 0x0007_0005, None, 11, false),
        // signed halves: -1 * 1 + -1 * 1
        (
            false,
            false,
            0xffff_ffff,
            0x0001_0001,
            None,
            0xffff_fffe,
            false,
        ),
        // the accumulate is signed and added last
        (
            false,
            false,
            0x0002_0003,
            0x0007_0005,
            Some(0xffff_fff6),
            19,
            false,
        ),
        (true, false, 0x0002_0003, 0x0007_0005, Some(100), 101, false),
        // -32768 * -32768 twice is 2^31, one past the largest 32-bit number
        (
            false,
            false,
            0x8000_8000,
            0x8000_8000,
            None,
            0x8000_0000,
            true,
        ),
        // for the subtract the same products cancel, and nothing can overflow
        (true, false, 0x8000_8000, 0x8000_8000, None, 0, false),
        (
            true,
            false,
            0x8000_0000,
            0x8000_0000,
            None,
            0xc000_0000,
            false,
        ),
        // the sum is exact, so an accumulate that brings 2^31 back in range leaves Q alone
        (
            false,
            false,
            0x8000_8000,
            0x8000_8000,
            Some(0x8000_0000),
            0,
            false,
        ),
        (
            false,
            false,
            0x8000_8000,
            0x8000_8000,
            Some(0xffff_ffff),
            0x7fff_ffff,
            false,
        ),
        // negative overflow through the accumulate: -32768 + -2^31 wraps to 0x7fff_8000, with Q
        (
            false,
            false,
            0x0000_8000,
            0x0000_0001,
            Some(0x8000_0000),
            0x7fff_8000,
            true,
        ),
        (
            true,
            false,
            0x0000_8000,
            0x0000_0001,
            Some(0x8000_0000),
            0x7fff_8000,
            true,
        ),
        // and a negative accumulate that stays in range
        (
            false,
            false,
            0x0000_0001,
            0x0000_0001,
            Some(0x8000_0000),
            0x8000_0001,
            false,
        ),
        (
            true,
            false,
            0x0000_0000,
            0x0000_0000,
            Some(0x8000_0000),
            0x8000_0000,
            false,
        ),
        (
            true,
            false,
            0x0000_0001,
            0x0000_0001,
            Some(0x8000_0000),
            0x8000_0001,
            false,
        ),
        // 2^30 - (-32768 * 32767) is 2147450880, and the accumulate takes it past 2^31
        (
            true,
            false,
            0x8000_8000,
            0x7fff_8000,
            Some(0x7fff_ffff),
            0xffff_7fff,
            true,
        ),
    ];
    for (subtract, swap, rn, rm, ra, expected, q) in examples {
        let ra_field = if ra.is_some() { 4 } else { NO_ACCUMULATE };
        let mut core = core_with(&[(2, rn), (3, rm), (4, ra.unwrap_or(0))], 0);
        run(&mut core, encode(subtract, 1, 2, 3, ra_field, swap));
        let context = format!("sub {subtract} swap {swap} {rn:#x} {rm:#x} {ra:?}");
        assert_eq!(core.get_r(Reg::R1), expected, "{context}");
        assert_eq!(core.psr.get_q(), q, "Q of {context}");
        assert_eq!(
            model(subtract, rn, rm, ra, swap),
            (expected, q),
            "model of {context}"
        );
    }
}

#[test]
fn test_q_is_sticky_and_no_other_flag_moves() {
    let mut core = core_with(&[(2, 1), (3, 1)], Q_BIT | 0xa00a_0000);
    run(&mut core, encode(false, 1, 2, 3, NO_ACCUMULATE, false));
    assert_eq!(core.get_r(Reg::R1), 1);
    assert_eq!(core.psr.value, Q_BIT | 0xa00a_0000);
}

#[test]
fn test_a_failed_it_condition_skips_the_instruction() {
    use super::ExecutorHelper;
    let mut core = core_with(&[(2, 3), (3, 9)], 0);
    core.psr.set_z_bit(true);
    core.set_itstate(((Condition::NE.value() as u8) << 4) | 0b1000);
    let outcome = core
        .execute_internal(&decode_32(encode(false, 1, 2, 3, 4, true)))
        .unwrap();
    assert!(matches!(outcome, super::ExecuteSuccess::NotTaken));
    assert_eq!(core.get_r(Reg::R1), super::dsp_test_support::sentinel(1));
}

#[test]
fn test_the_registers_the_manual_forbids_are_undefined() {
    for subtract in [false, true] {
        for bad in [13, 15] {
            for ra in [NO_ACCUMULATE, 4] {
                for opcode in [
                    encode(subtract, bad, 2, 3, ra, false),
                    encode(subtract, 1, bad, 3, ra, true),
                    encode(subtract, 1, 2, bad, ra, false),
                ] {
                    assert!(is_udf(&decode_32(opcode)), "{opcode:#010x}");
                }
            }
        }
        // Ra = SP is UNPREDICTABLE for SMLAD and SMLSD, and Ra = PC is SMUAD and SMUSD
        assert!(is_udf(&decode_32(encode(subtract, 1, 2, 3, 13, false))));
        let text = decode_32(encode(subtract, 1, 2, 3, 15, false)).to_string();
        assert!(
            text.starts_with(if subtract { "smusd" } else { "smuad" }),
            "{text}"
        );
    }
}

#[test]
fn test_the_neighbouring_encodings_are_not_these_instructions() {
    // hw2<7:5> must be 000
    for opcode in [
        0xfb22_f123u32,
        0xfb22_f143,
        0xfb22_f183,
        0xfb42_f123,
        0xfb42_4183,
    ] {
        let text = decode_32(opcode).to_string();
        assert!(
            !text.starts_with("smu") && !text.starts_with("sml"),
            "{opcode:#010x} {text}"
        );
    }
}
