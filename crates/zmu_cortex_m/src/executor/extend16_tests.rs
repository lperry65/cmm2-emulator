//! SXTB16, SXTAB16, UXTB16 and UXTAB16 (A7.7.183, .180, .222 and .219), and the rotations of the
//! byte and halfword extend-and-adds next to them (SXTAB, SXTAH, UXTAB, UXTAH: A7.7.179, .181,
//! .218 and .220).
//!
//! The opcodes come from the diagrams (1111 1010 0 S 1 U Rn, 1111 Rd 1 (0) rotate Rm) and
//! llvm-mc, the expected values from `model`.

use super::dsp_test_support::{Case, LAYOUTS, assert_untouched, cases, core_with, is_udf, run};
use crate::Processor;
use crate::core::register::{Apsr, BaseReg, Reg};
use crate::decoder::decode_32;

/// The extends, with the bits of hw1<7:4> that pick them: Rn = 1111 is the form without the add
const KINDS: [(&str, u32); 8] = [
    ("sxtab16", 0xfa20),
    ("uxtab16", 0xfa30),
    ("sxtab", 0xfa40),
    ("uxtab", 0xfa50),
    ("sxtah", 0xfa00),
    ("uxtah", 0xfa10),
    ("sxtb16", 0xfa2f),
    ("uxtb16", 0xfa3f),
];

/// hw2 is 1111 Rd 1 (0) rotate Rm; `rotate` is the field, 0 to 3, so the rotation is 8 times it
fn encode(hw1_base: u32, rd: u32, rn: u32, rm: u32, rotate: u32) -> u32 {
    // the plain forms have Rn = 1111 in hw1 already
    let hw1 = if hw1_base & 0xf == 0xf {
        hw1_base
    } else {
        hw1_base | rn
    };
    hw1 << 16 | 0xf000 | rd << 8 | 0x80 | rotate << 4 | rm
}

/// The pseudocode: rotate Rm right, take the byte or halfword lanes, extend and add per lane
fn model(name: &str, rn: u32, rm: u32, rotate: u32) -> u32 {
    let rotated = rm.rotate_right(8 * rotate);
    let signed = name.starts_with('s');
    let sign_extend = |value: u32, bits: u32| -> i64 {
        let value = i64::from(value & ((1 << bits) - 1));
        if signed && value >= 1 << (bits - 1) {
            value - (1 << bits)
        } else {
            value
        }
    };
    let has_add = !(name == "sxtb16" || name == "uxtb16");
    let rn = if has_add { rn } else { 0 };
    let word = |value: i64| (value & 0xffff_ffff) as u32;
    match name.trim_start_matches(['s', 'u']) {
        "xtab16" | "xtb16" => {
            // each halfword: Rn's lane plus the extended byte, wrapped to 16 bits
            let low = (i64::from(rn & 0xffff) + sign_extend(rotated, 8)) & 0xffff;
            let high = (i64::from(rn >> 16) + sign_extend(rotated >> 16, 8)) & 0xffff;
            word(high << 16 | low)
        }
        "xtab" => word(i64::from(rn) + sign_extend(rotated, 8)),
        "xtah" => word(i64::from(rn) + sign_extend(rotated, 16)),
        _ => unreachable!(),
    }
}

fn check(core: &mut Processor, name: &str, hw1_base: u32, rotate: u32, case: &Case) {
    let [rd, rn_number, rm_number, _] = case.layout;
    let opcode = encode(hw1_base, rd, rn_number, rm_number, rotate);
    case.load(core);
    let before = core.r0_12;
    let (rn, rm) = (
        core.get_r(Reg::from(rn_number as u8)),
        core.get_r(Reg::from(rm_number as u8)),
    );
    run(core, opcode);
    let context = format!("{name} {opcode:#010x} {}", case.describe());
    assert_eq!(
        core.get_r(Reg::from(rd as u8)),
        model(name, rn, rm, rotate),
        "Rd of {context}"
    );
    assert_eq!(core.psr.value, case.psr, "flags of {context}");
    assert_untouched(core, &before, &[rd], &context);
}

#[test]
fn test_the_opcodes_are_the_manuals_and_llvms_and_decode_to_the_right_name() {
    // llvm-mc, for `name r1, r2, r3, ror #rotation`, or `name r1, r3, ror #rotation` for the
    // plain forms: (name, rotate, opcode, text)
    let cases = [
        ("sxtab16", 0, 0xfa22_f183, "sxtab16 r1, r2, r3"),
        ("sxtab16", 2, 0xfa22_f1a3, "sxtab16 r1, r2, r3, ror #16"),
        ("uxtab16", 1, 0xfa32_f193, "uxtab16 r1, r2, r3, ror #8"),
        ("sxtab", 3, 0xfa42_f1b3, "sxtab r1, r2, r3, ror #24"),
        ("uxtab", 0, 0xfa52_f183, "uxtab r1, r2, r3"),
        ("sxtah", 1, 0xfa02_f193, "sxtah r1, r2, r3, ror #8"),
        ("uxtah", 2, 0xfa12_f1a3, "uxtah r1, r2, r3, ror #16"),
        ("sxtb16", 0, 0xfa2f_f183, "sxtb16 r1, r3"),
        ("sxtb16", 1, 0xfa2f_f193, "sxtb16 r1, r3, ror #8"),
        ("uxtb16", 3, 0xfa3f_f1b3, "uxtb16 r1, r3, ror #24"),
    ];
    for (name, rotate, opcode, text) in cases {
        let hw1_base = KINDS.iter().find(|(kind, _)| *kind == name).unwrap().1;
        assert_eq!(encode(hw1_base, 1, 2, 3, rotate), opcode, "{name}");
        assert_eq!(decode_32(opcode).to_string(), text, "{opcode:#010x}");
    }
}

#[test]
fn test_every_extend_matches_the_model_in_every_layout_and_rotation() {
    let mut core = Processor::new();
    for (name, hw1_base) in KINDS {
        for case in cases(&LAYOUTS, 3, 1500, 0x77aa_0f0f_1357_9bdf) {
            for rotate in 0..4 {
                check(&mut core, name, hw1_base, rotate, &case);
            }
        }
    }
}

#[test]
fn test_the_worked_examples() {
    // name, rotate, Rn, Rm, Rd
    let examples = [
        // the bytes at 7:0 and 23:16 extend to halfwords: 0x80 is -128, 0x7f is 127
        ("sxtb16", 0, 0, 0x007f_0080, 0x007f_ff80),
        ("uxtb16", 0, 0, 0xff7f_ff80, 0x007f_0080),
        // rotation by 8 brings bytes 1 and 3 down
        ("sxtb16", 1, 0, 0x8001_7f02, 0xff80_007f),
        ("uxtb16", 1, 0, 0x8001_7f02, 0x0080_007f),
        // rotation by 16 and 24
        ("uxtb16", 2, 0, 0x1122_3344, 0x0044_0022),
        ("uxtb16", 3, 0, 0x1122_3344, 0x0033_0011),
        // each halfword sum wraps on its own, with no carry into the other lane
        ("uxtab16", 0, 0xffff_ffff, 0x0001_0001, 0x0000_0000),
        ("sxtab16", 0, 0x0001_0001, 0x00ff_00ff, 0x0000_0000),
        ("sxtab16", 0, 0x7fff_7fff, 0x0001_0001, 0x8000_8000),
        // SXTAB, SXTAH, UXTAB, UXTAH with the rotations
        ("sxtab", 1, 10, 0x0000_ff00, 9),
        ("uxtab", 1, 10, 0x0000_ff00, 265),
        ("sxtah", 2, 0x1_0000, 0x8000_1234, 0x1_0000 - 0x8000),
        ("uxtah", 2, 0x1_0000, 0x8000_1234, 0x1_0000 + 0x8000),
        ("uxtah", 3, 0xffff_0000, 0x1234_5678, 0xffff_7812),
    ];
    for (name, rotate, rn, rm, expected) in examples {
        let hw1_base = KINDS.iter().find(|(kind, _)| *kind == name).unwrap().1;
        let mut core = core_with(&[(2, rn), (3, rm)], 0);
        run(&mut core, encode(hw1_base, 1, 2, 3, rotate));
        assert_eq!(
            core.get_r(Reg::R1),
            expected,
            "{name} {rotate} {rn:#x} {rm:#x}"
        );
        assert_eq!(model(name, rn, rm, rotate), expected, "model of {name}");
    }
}

#[test]
fn test_a_failed_it_condition_skips_the_instruction() {
    use super::ExecutorHelper;
    use crate::core::condition::Condition;
    let mut core = core_with(&[(2, 3), (3, 9)], 0);
    core.psr.set_z_bit(true);
    core.set_itstate(((Condition::NE.value() as u8) << 4) | 0b1000);
    let outcome = core
        .execute_internal(&decode_32(encode(0xfa20, 1, 2, 3, 0)))
        .unwrap();
    assert!(matches!(outcome, super::ExecuteSuccess::NotTaken));
    assert_eq!(core.get_r(Reg::R1), super::dsp_test_support::sentinel(1));
}

#[test]
fn test_the_registers_the_manual_forbids_are_undefined() {
    for (name, hw1_base) in KINDS {
        let adds = hw1_base & 0xf != 0xf;
        for bad in [13, 15] {
            // Rd and Rm may not be SP or PC
            for opcode in [
                encode(hw1_base, bad, 2, 3, 0),
                encode(hw1_base, 1, 2, bad, 0),
            ] {
                assert!(is_udf(&decode_32(opcode)), "{name} {opcode:#010x}");
            }
        }
        if adds {
            // Rn = SP is UNPREDICTABLE, and Rn = PC is the form with no add
            assert!(
                is_udf(&decode_32(encode(hw1_base, 1, 13, 3, 0))),
                "{name} Rn = SP"
            );
        }
    }
    // the add forms with Rn = PC are the plain ones
    assert_eq!(
        decode_32(encode(0xfa20, 1, 15, 3, 0)).to_string(),
        "sxtb16 r1, r3"
    );
    assert_eq!(
        decode_32(encode(0xfa30, 1, 15, 3, 1)).to_string(),
        "uxtb16 r1, r3, ror #8"
    );
    assert_eq!(
        decode_32(encode(0xfa40, 1, 15, 3, 0))
            .to_string()
            .to_lowercase(),
        "sxtb.w r1, r3"
    );
}

#[test]
fn test_the_neighbouring_encodings_are_not_these_instructions() {
    // hw2<7> must be 1 and hw2<6> 0 (the (0) of the diagram), and hw2<15:12> must be 1111
    for opcode in [
        0xfa22_f103u32,
        0xfa22_f1c3,
        0xfa22_f1e3,
        0xfa22_e183,
        0xfa22_0183,
    ] {
        let text = decode_32(opcode).to_string();
        assert!(!text.contains("xt"), "{opcode:#010x} is {text}");
    }
}
