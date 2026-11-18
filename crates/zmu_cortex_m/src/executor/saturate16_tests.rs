//! SSAT16 and USAT16 (A7.7.153 and A7.7.214): two halfwords saturated on their own, and Q set if
//! either was. They were decoded as SSAT and USAT with ASR #32 before, which gave a wrong result.
//!
//! The opcodes come from the diagrams (1111 0 (0) 11 0 U 1 0 Rn, 0000 Rd 00 (0)(0) `sat_imm`) and
//! llvm-mc, and the expected values from `model`.

use super::dsp_test_support::{
    Case, LAYOUTS, Q_BIT, assert_untouched, cases, core_with, is_udf, run, saturate_model,
};
use crate::Processor;
use crate::core::instruction::Instruction;
use crate::core::register::{Apsr, BaseReg, Reg};
use crate::decoder::decode_32;

/// `field` is `sat_imm`: the bit position less 1 for SSAT16 (1 to 16), the position for USAT16
/// (0 to 15)
fn encode(unsigned: bool, rd: u32, rn: u32, field: u32) -> u32 {
    (0xf320 | u32::from(unsigned) << 7 | rn) << 16 | rd << 8 | field
}

/// Each halfword is a signed number, saturated to the signed or unsigned range, and the result
/// is that range's halfword. Q is the second value.
fn model(unsigned: bool, field: u32, rn: u32) -> (u32, bool) {
    let bits = if unsigned { field } else { field + 1 };
    let lane = |value: u32| saturate_model(i128::from(value as u16 as i16), bits, !unsigned);
    let (low, low_saturated) = lane(rn);
    let (high, high_saturated) = lane(rn >> 16);
    let halfword = |value: i128| (value & 0xffff) as u32;
    (
        halfword(high) << 16 | halfword(low),
        low_saturated || high_saturated,
    )
}

fn check(core: &mut Processor, unsigned: bool, field: u32, case: &Case) {
    let [rd, rn_number, ..] = case.layout;
    let opcode = encode(unsigned, rd, rn_number, field);
    case.load(core);
    let before = core.r0_12;
    let rn = core.get_r(Reg::from(rn_number as u8));
    run(core, opcode);
    let (expected, saturated) = model(unsigned, field, rn);
    let context = format!("{opcode:#010x} {}", case.describe());
    assert_eq!(core.get_r(Reg::from(rd as u8)), expected, "Rd of {context}");
    let q = if saturated { Q_BIT } else { 0 };
    assert_eq!(core.psr.value, case.psr | q, "flags of {context}");
    assert_untouched(core, &before, &[rd], &context);
}

#[test]
fn test_the_opcodes_are_the_manuals_and_llvms_and_decode_to_the_right_name() {
    // llvm-mc, with Rd = r1 and Rn = r2
    for (unsigned, field, opcode, text) in [
        (false, 0, 0xf322_0100, "ssat16 r1, #1, r2"),
        (false, 15, 0xf322_010f, "ssat16 r1, #16, r2"),
        (true, 0, 0xf3a2_0100, "usat16 r1, #0, r2"),
        (true, 15, 0xf3a2_010f, "usat16 r1, #15, r2"),
    ] {
        assert_eq!(encode(unsigned, 1, 2, field), opcode, "{text}");
        assert_eq!(decode_32(opcode).to_string(), text, "{opcode:#010x}");
    }
}

#[test]
fn test_every_saturation_position_matches_the_model_in_every_layout() {
    let mut core = Processor::new();
    for case in cases(&LAYOUTS, 2, 400, 0x1616_2929_3a3a_4b4b) {
        for unsigned in [false, true] {
            for field in 0..16 {
                check(&mut core, unsigned, field, &case);
            }
        }
    }
}

#[test]
fn test_the_worked_examples() {
    // (unsigned, field, Rn, Rd, Q): the halfwords are saturated separately
    let examples = [
        // -129 and 128 to 8 bits: -128 and 127
        (false, 7, 0x0080_ff7f, 0x007f_ff80, true),
        // both inside the range: nothing changes, and Q is not set
        (false, 7, 0x007f_ff80, 0x007f_ff80, false),
        // one outside is enough for Q
        (false, 7, 0x0001_0080, 0x0001_007f, true),
        (false, 7, 0x8000_0001, 0xff80_0001, true),
        // 16 bits is the whole halfword
        (false, 15, 0x8000_7fff, 0x8000_7fff, false),
        // 1 bit is the range -1 to 0
        (false, 0, 0x0001_ffff, 0x0000_ffff, true),
        (false, 0, 0xfffe_0000, 0xffff_0000, true),
        // 256 and -1 to 8 bits unsigned: 255 and 0
        (true, 8, 0xffff_0100, 0x0000_00ff, true),
        (true, 8, 0x00ff_0000, 0x00ff_0000, false),
        // 0 bits: everything is 0, and Q is set unless both are 0
        (true, 0, 0x0000_0000, 0, false),
        (true, 0, 0x0001_0000, 0, true),
        (true, 0, 0x0000_8000, 0, true),
        // 15 bits: 0x7fff stays, 0x8000 is negative
        (true, 15, 0x7fff_7fff, 0x7fff_7fff, false),
        (true, 15, 0x7fff_8000, 0x7fff_0000, true),
    ];
    for (unsigned, field, rn, expected, q) in examples {
        let mut core = core_with(&[(2, rn)], 0);
        run(&mut core, encode(unsigned, 1, 2, field));
        let context = format!("unsigned {unsigned} field {field} rn {rn:#010x}");
        assert_eq!(core.get_r(Reg::R1), expected, "{context}");
        assert_eq!(core.psr.get_q(), q, "Q of {context}");
        assert_eq!(
            model(unsigned, field, rn),
            (expected, q),
            "model of {context}"
        );
    }
}

#[test]
fn test_q_is_sticky_and_no_other_flag_moves() {
    let mut core = core_with(&[(2, 0x0001_0002)], Q_BIT | 0xa00a_0000);
    run(&mut core, encode(false, 1, 2, 7));
    assert_eq!(core.get_r(Reg::R1), 0x0001_0002);
    assert_eq!(core.psr.value, Q_BIT | 0xa00a_0000);
}

#[test]
fn test_a_failed_it_condition_skips_the_instruction() {
    use super::ExecutorHelper;
    use crate::core::condition::Condition;
    let mut core = core_with(&[(2, 0x7fff_7fff)], 0);
    core.psr.set_z_bit(true);
    core.set_itstate(((Condition::NE.value() as u8) << 4) | 0b1000);
    let outcome = core
        .execute_internal(&decode_32(encode(false, 1, 2, 7)))
        .unwrap();
    assert!(matches!(outcome, super::ExecuteSuccess::NotTaken));
    assert_eq!(core.get_r(Reg::R1), super::dsp_test_support::sentinel(1));
    assert!(!core.psr.get_q());
}

#[test]
fn test_the_registers_the_manual_forbids_are_undefined() {
    for unsigned in [false, true] {
        for bad in [13, 15] {
            for opcode in [encode(unsigned, bad, 2, 7), encode(unsigned, 1, bad, 7)] {
                assert!(is_udf(&decode_32(opcode)), "{opcode:#010x}");
            }
        }
    }
}

#[test]
fn test_these_are_not_ssat_or_usat_and_the_bits_that_must_be_zero_are() {
    for unsigned in [false, true] {
        let decoded = decode_32(encode(unsigned, 1, 2, 7));
        assert!(
            !matches!(decoded, Instruction::SSAT { .. } | Instruction::USAT { .. }),
            "{decoded:?}"
        );
        // sat_imm bits 4 and 5 are (0): with either set it is neither of these
        for field in [0x17, 0x27, 0x37] {
            assert!(
                is_udf(&decode_32(encode(unsigned, 1, 2, field))),
                "{field:#x}"
            );
        }
        // hw2<15> is 0 and hw2<14:12> is the shift amount's high bits: not zero is SSAT or USAT
        let shifted = decode_32(encode(unsigned, 1, 2, 7) | 0x1000);
        assert!(matches!(
            shifted,
            Instruction::SSAT { .. } | Instruction::USAT { .. }
        ));
        // hw2<7:6> is the shift amount's low bits, so with either set it is a shift too
        assert!(matches!(
            decode_32(encode(unsigned, 1, 2, 7) | 0x40),
            Instruction::SSAT { .. } | Instruction::USAT { .. }
        ));
    }
}
