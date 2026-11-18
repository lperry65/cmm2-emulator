//! USAD8 and USADA8 (A7.7.211 and A7.7.212).
//!
//! The opcodes are written from the diagram (1111 1011 0111 Rn, Ra Rd 0000 Rm) and checked
//! against llvm-mc, and the expected values come from `model`, the manual's pseudocode on
//! unbounded integers.

use super::dsp_test_support::{
    Case, LAYOUTS, Q_BIT, assert_untouched, cases, core_with, is_udf, run,
};
use crate::Processor;
use crate::core::condition::Condition;
use crate::core::register::{Apsr, BaseReg, Reg};
use crate::decoder::decode_32;

/// USAD8 is USADA8 with Ra = 1111
const NO_ACCUMULATE: u32 = 15;

fn encode(rd: u32, rn: u32, rm: u32, ra: u32) -> u32 {
    (0xfb70 | rn) << 16 | ra << 12 | rd << 8 | rm
}

/// The four absolute differences of the unsigned bytes, summed, with Ra added for USADA8
fn model(rn: u32, rm: u32, ra: Option<u32>) -> u32 {
    let byte = |value: u32, lane: u32| i128::from((value >> (8 * lane)) & 0xff);
    let mut result = i128::from(ra.unwrap_or(0));
    for lane in 0..4 {
        result += (byte(rn, lane) - byte(rm, lane)).abs();
    }
    (result & 0xffff_ffff) as u32
}

fn check(core: &mut Processor, case: &Case) {
    let [rd, rn_number, rm_number, ra_number] = case.layout;
    let accumulate = case.values[3] & 1 == 1;
    let ra_field = if accumulate { ra_number } else { NO_ACCUMULATE };
    let opcode = encode(rd, rn_number, rm_number, ra_field);
    case.load(core);
    let before = core.r0_12;
    let (rn, rm, ra) = (
        core.get_r(Reg::from(rn_number as u8)),
        core.get_r(Reg::from(rm_number as u8)),
        core.get_r(Reg::from(ra_number as u8)),
    );
    run(core, opcode);
    let context = format!("{opcode:#010x} {}", case.describe());
    assert_eq!(
        core.get_r(Reg::from(rd as u8)),
        model(rn, rm, accumulate.then_some(ra)),
        "Rd of {context}"
    );
    // neither GE nor Q nor any other flag is touched
    assert_eq!(core.psr.value, case.psr, "flags of {context}");
    assert_untouched(core, &before, &[rd], &context);
}

#[test]
fn test_the_opcodes_are_the_manuals_and_llvms_and_decode_to_the_right_name() {
    // llvm-mc: usad8 r1, r2, r3 and usada8 r1, r2, r3, r4
    assert_eq!(encode(1, 2, 3, NO_ACCUMULATE), 0xfb72_f103);
    assert_eq!(encode(1, 2, 3, 4), 0xfb72_4103);
    assert_eq!(decode_32(0xfb72_f103).to_string(), "usad8 r1, r2, r3");
    assert_eq!(decode_32(0xfb72_4103).to_string(), "usada8 r1, r2, r3, r4");
}

#[test]
fn test_the_edge_and_random_values_match_the_model_in_every_layout() {
    let mut core = Processor::new();
    for case in cases(&LAYOUTS, 4, 3000, 0x5a5a_1234_9876_0001) {
        check(&mut core, &case);
    }
}

#[test]
fn test_the_worked_examples() {
    // name, Rn, Rm, Ra, Rd
    let examples = [
        // each byte differs by 1: 4
        (0x0102_0304, 0x0203_0405, None, 4),
        // the larger byte may be in either operand: |0 - 255| + |255 - 0| + |1 - 1| + |7 - 2|
        (0x00ff_0107, 0xff00_0102, None, 255 + 255 + 5),
        (0xffff_ffff, 0, None, 4 * 255),
        (0, 0xffff_ffff, None, 4 * 255),
        (0x1234_5678, 0x1234_5678, None, 0),
        // the accumulate wraps in 32 bits
        (0xffff_ffff, 0, Some(0xffff_ffff), 4 * 255 - 1),
        (0x0000_0005, 0x0000_0002, Some(100), 103),
    ];
    for (rn, rm, ra, expected) in examples {
        let ra_field = if ra.is_some() { 4 } else { NO_ACCUMULATE };
        let mut core = core_with(&[(2, rn), (3, rm), (4, ra.unwrap_or(0))], 0);
        run(&mut core, encode(1, 2, 3, ra_field));
        assert_eq!(core.get_r(Reg::R1), expected, "{rn:#x} {rm:#x} {ra:?}");
        assert_eq!(
            model(rn, rm, ra),
            expected,
            "model of {rn:#x} {rm:#x} {ra:?}"
        );
    }
}

#[test]
fn test_neither_q_nor_ge_is_set_by_a_sum_that_overflows_a_byte_or_a_word() {
    let mut core = core_with(&[(2, 0xffff_ffff), (3, 0), (4, 0xffff_ffff)], 0);
    run(&mut core, encode(1, 2, 3, 4));
    assert_eq!(core.psr.value & Q_BIT, 0);
    assert_eq!(core.psr.value, 0);
}

#[test]
fn test_a_failed_it_condition_skips_the_instruction() {
    use super::ExecutorHelper;
    let mut core = core_with(&[(2, 3), (3, 9)], 0);
    core.psr.set_z_bit(true);
    core.set_itstate(((Condition::NE.value() as u8) << 4) | 0b1000);
    let outcome = core
        .execute_internal(&decode_32(encode(1, 2, 3, NO_ACCUMULATE)))
        .unwrap();
    assert!(matches!(outcome, super::ExecuteSuccess::NotTaken));
    assert_eq!(core.get_r(Reg::R1), super::dsp_test_support::sentinel(1));
}

#[test]
fn test_the_registers_the_manual_forbids_are_undefined() {
    for bad in [13, 15] {
        // d, n and m in {13, 15}, for both
        for ra in [NO_ACCUMULATE, 4] {
            for opcode in [
                encode(bad, 2, 3, ra),
                encode(1, bad, 3, ra),
                encode(1, 2, bad, ra),
            ] {
                assert!(is_udf(&decode_32(opcode)), "{opcode:#010x}");
            }
        }
    }
    // Ra = SP is UNPREDICTABLE for USADA8, and Ra = PC is USAD8
    assert!(is_udf(&decode_32(encode(1, 2, 3, 13))));
    assert_eq!(
        decode_32(encode(1, 2, 3, 15)).to_string(),
        "usad8 r1, r2, r3"
    );
}

#[test]
fn test_the_neighbouring_encodings_are_not_these_instructions() {
    // hw2<7:4> must be 0000, and the other op1 values of the multiply group are other things
    for opcode in [0xfb72_f113u32, 0xfb72_f183, 0xfb72_f1f3, 0xfb62_f103] {
        let text = decode_32(opcode).to_string();
        assert!(
            !text.starts_with("usad8") && !text.starts_with("usada8"),
            "{opcode:#010x}"
        );
    }
}
