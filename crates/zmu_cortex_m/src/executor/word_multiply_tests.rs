//! SMULWB, SMULWT, SMLAWB and SMLAWT (A7.7.150 and A7.7.141): a word times a halfword, the top 32
//! bits of the 48-bit product, with the accumulate added to those and Q set if that overflows.
//!
//! The opcodes come from the diagrams (1111 1011 0011 Rn, Ra Rd 000 M Rm) and llvm-mc, and the
//! expected values from `model`, which works on unbounded integers.

use super::dsp_test_support::{
    Case, LAYOUTS, Q_BIT, assert_untouched, cases, core_with, is_udf, run,
};
use crate::Processor;
use crate::core::condition::Condition;
use crate::core::instruction::Instruction;
use crate::core::register::{Apsr, BaseReg, Reg};
use crate::decoder::decode_32;

/// SMULW is SMLAW with Ra = 1111
const NO_ACCUMULATE: u32 = 15;

fn encode(rd: u32, rn: u32, rm: u32, ra: u32, top: bool) -> u32 {
    (0xfb30 | rn) << 16 | ra << 12 | rd << 8 | u32::from(top) << 4 | rm
}

/// The pseudocode: Rn times the signed halfword of Rm, plus Ra << 16 for SMLAW, and bits 47:16
/// of that. Q is set when what was above bit 47 of the sum is not the sign of bit 47.
fn model(rn: u32, rm: u32, ra: Option<u32>, top: bool) -> (u32, bool) {
    let operand2 = if top { rm >> 16 } else { rm & 0xffff } as u16 as i16;
    let mut result = i128::from(rn as i32) * i128::from(operand2);
    if let Some(ra) = ra {
        result += i128::from(ra as i32) << 16;
    }
    let rd = ((result >> 16) & 0xffff_ffff) as u32;
    (rd, ra.is_some() && (result >> 16) != i128::from(rd as i32))
}

fn check(core: &mut Processor, accumulate: bool, top: bool, case: &Case) {
    let [rd, rn_number, rm_number, ra_number] = case.layout;
    let ra_field = if accumulate { ra_number } else { NO_ACCUMULATE };
    let opcode = encode(rd, rn_number, rm_number, ra_field, top);
    case.load(core);
    let before = core.r0_12;
    let (rn, rm, ra) = (
        core.get_r(Reg::from(rn_number as u8)),
        core.get_r(Reg::from(rm_number as u8)),
        core.get_r(Reg::from(ra_number as u8)),
    );
    run(core, opcode);
    let (expected, overflow) = model(rn, rm, accumulate.then_some(ra), top);
    let context = format!("{opcode:#010x} {}", case.describe());
    assert_eq!(core.get_r(Reg::from(rd as u8)), expected, "Rd of {context}");
    let q = if overflow { Q_BIT } else { 0 };
    assert_eq!(core.psr.value, case.psr | q, "flags of {context}");
    assert_untouched(core, &before, &[rd], &context);
}

#[test]
fn test_the_opcodes_are_the_manuals_and_llvms_and_decode_to_the_right_name() {
    // llvm-mc, with r1, r2, r3 and, for the accumulating forms, r4
    for (accumulate, top, opcode, text) in [
        (false, false, 0xfb32_f103, "smulwb r1, r2, r3"),
        (false, true, 0xfb32_f113, "smulwt r1, r2, r3"),
        (true, false, 0xfb32_4103, "smlawb r1, r2, r3, r4"),
        (true, true, 0xfb32_4113, "smlawt r1, r2, r3, r4"),
    ] {
        let ra = if accumulate { 4 } else { NO_ACCUMULATE };
        assert_eq!(encode(1, 2, 3, ra, top), opcode, "{text}");
        assert_eq!(decode_32(opcode).to_string(), text, "{opcode:#010x}");
    }
}

#[test]
fn test_the_edge_and_random_values_match_the_model_in_every_layout() {
    let mut core = Processor::new();
    for case in cases(&LAYOUTS, 4, 2500, 0x3131_7272_a9a9_0505) {
        for accumulate in [false, true] {
            for top in [false, true] {
                check(&mut core, accumulate, top, &case);
            }
        }
    }
}

#[test]
fn test_the_worked_examples() {
    // (top, Rn, Rm, Ra, Rd, Q)
    let examples = [
        // 65536 * 2 = 131072, and the top 32 of the 48 bits is 2
        (false, 0x0001_0000, 0x0000_0002, None, 0x0000_0002, false),
        // only the chosen halfword counts
        (false, 0x0001_0000, 0x7fff_0002, None, 0x0000_0002, false),
        (true, 0x0001_0000, 0x0002_7fff, None, 0x0000_0002, false),
        // the low 16 bits of the product are dropped, rounding down: -1 * 1 is -1, not 0
        (false, 0xffff_ffff, 0x0000_0001, None, 0xffff_ffff, false),
        (false, 0x0000_ffff, 0x0000_0001, None, 0x0000_0000, false),
        // -2^31 * -2^15 = 2^46, and -2^31 * -1 = 2^31, which is 0x8000 after the shift
        (false, 0x8000_0000, 0x0000_8000, None, 0x4000_0000, false),
        (true, 0x8000_0000, 0xffff_0000, None, 0x0000_8000, false),
        // the accumulate is added to the high part
        (false, 0x0001_0000, 0x0000_0002, Some(5), 0x0000_0007, false),
        // no overflow past 32 bits is possible in the product, but the sum can
        (
            false,
            0x0001_0000,
            0x0000_0001,
            Some(0x7fff_ffff),
            0x8000_0000,
            true,
        ),
        (
            false,
            0xffff_0000,
            0x0000_0001,
            Some(0x8000_0000),
            0x7fff_ffff,
            true,
        ),
        (
            false,
            0x0001_0000,
            0x0000_0001,
            Some(0x7fff_fffe),
            0x7fff_ffff,
            false,
        ),
        (
            true,
            0x0001_0000,
            0x0001_0000,
            Some(0xffff_ffff),
            0x0000_0000,
            false,
        ),
    ];
    for (top, rn, rm, ra, expected, q) in examples {
        let ra_field = if ra.is_some() { 4 } else { NO_ACCUMULATE };
        let mut core = core_with(&[(2, rn), (3, rm), (4, ra.unwrap_or(0))], 0);
        run(&mut core, encode(1, 2, 3, ra_field, top));
        let context = format!("top {top} {rn:#x} {rm:#x} {ra:?}");
        assert_eq!(core.get_r(Reg::R1), expected, "{context}");
        assert_eq!(core.psr.get_q(), q, "Q of {context}");
        assert_eq!(model(rn, rm, ra, top), (expected, q), "model of {context}");
    }
}

#[test]
fn test_smulw_never_sets_q_and_q_is_sticky() {
    // 0x7fffffff * 0x7fff has no overflow in 48 bits and SMULW has no accumulate
    let mut core = core_with(&[(2, 0x7fff_ffff), (3, 0x7fff)], 0);
    run(&mut core, encode(1, 2, 3, NO_ACCUMULATE, false));
    assert_eq!(core.psr.value, 0);
    // SMLAW without an overflow leaves a Q that is already set
    let mut core = core_with(&[(2, 1), (3, 1), (4, 1)], Q_BIT | 0xa00a_0000);
    run(&mut core, encode(1, 2, 3, 4, false));
    assert_eq!(core.psr.value, Q_BIT | 0xa00a_0000);
}

#[test]
fn test_a_failed_it_condition_skips_the_instruction() {
    use super::ExecutorHelper;
    let mut core = core_with(&[(2, 3), (3, 9)], 0);
    core.psr.set_z_bit(true);
    core.set_itstate(((Condition::NE.value() as u8) << 4) | 0b1000);
    let outcome = core
        .execute_internal(&decode_32(encode(1, 2, 3, 4, false)))
        .unwrap();
    assert!(matches!(outcome, super::ExecuteSuccess::NotTaken));
    assert_eq!(core.get_r(Reg::R1), super::dsp_test_support::sentinel(1));
}

#[test]
fn test_the_registers_the_manual_forbids_are_undefined() {
    for top in [false, true] {
        for bad in [13, 15] {
            for ra in [NO_ACCUMULATE, 4] {
                for opcode in [
                    encode(bad, 2, 3, ra, top),
                    encode(1, bad, 3, ra, top),
                    encode(1, 2, bad, ra, top),
                ] {
                    assert!(is_udf(&decode_32(opcode)), "{opcode:#010x}");
                }
            }
        }
        // Ra = SP is UNPREDICTABLE for SMLAW, and Ra = PC is SMULW
        assert!(is_udf(&decode_32(encode(1, 2, 3, 13, top))));
        assert!(
            decode_32(encode(1, 2, 3, 15, top))
                .to_string()
                .starts_with("smulw")
        );
    }
}

#[test]
fn test_the_neighbouring_encodings_are_not_these_instructions() {
    // hw2<7:5> must be 000, and op1 = 010 and 100 are the dual multiplies
    for opcode in [0xfb32_f123u32, 0xfb32_f143, 0xfb32_f183, 0xfb32_f1f3] {
        let text = decode_32(opcode).to_string();
        assert!(
            !text.starts_with("smulw") && !text.starts_with("smlaw"),
            "{opcode:#010x}"
        );
    }
    assert!(!matches!(
        decode_32(0xfb22_f103),
        Instruction::SMUL { .. } | Instruction::SMLA { .. }
    ));
}
