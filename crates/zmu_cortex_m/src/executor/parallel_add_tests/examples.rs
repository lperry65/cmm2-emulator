//! The opcodes, the game's instruction, and the worked examples.

use super::model::{encode, model, split};
use super::*;

#[test]
fn test_the_opcodes_are_the_manuals_and_llvms_and_decode_to_the_right_name() {
    for (name, opcode) in OPCODES {
        assert_eq!(encode(name, 1, 2, 3), opcode, "{name}");
        let decoded = decode_32(opcode);
        assert!(
            matches!(decoded, Instruction::PARALLEL { .. }),
            "{name}: {decoded:?}"
        );
        assert_eq!(decoded.to_string(), format!("{name} r1, r2, r3"));
    }
}

#[test]
fn test_the_game_instruction_is_uqsub8() {
    // fac0 f051 is the word at 0x08146b6c in Galactic Raiders: uqsub8 r0, r0, r1
    assert_eq!(decode_32(0xfac0_f051).to_string(), "uqsub8 r0, r0, r1");
    // bytes 10 05 FF 00 minus 0A 08 01 01, each floored at 0: 06 00 FE 00
    let core = run(
        0xfac0_f051,
        &[(Reg::R0, 0x1005_ff00), (Reg::R1, 0x0a08_0101)],
        0b0101,
        true,
    );
    assert_eq!(core.get_r(Reg::R0), 0x0600_fe00);
    assert_eq!(ge_of(&core), 0b0101);
    assert!(core.psr.get_q());
}

#[test]
fn test_only_the_plain_forms_write_ge_and_none_writes_q() {
    for (name, opcode) in OPCODES {
        let (prefix, _) = split(name);
        for (ge, q) in [
            (0b0000, false),
            (0b1111, true),
            (0b0110, false),
            (0b1001, true),
        ] {
            let core = run(
                opcode,
                &[(Reg::R2, 0x7f80_ff01), (Reg::R3, 0x0180_ff01)],
                ge,
                q,
            );
            assert_eq!(core.psr.get_q(), q, "{name} must not touch Q");
            if prefix != "s" && prefix != "u" {
                assert_eq!(ge_of(&core), ge, "{name} must leave GE alone");
            }
        }
    }
}

#[test]
fn test_the_worked_examples() {
    // each: opcode name, Rn, Rm, expected Rd, expected GE (None when GE is left alone)
    let cases: [(&str, u32, u32, u32, Option<u32>); 16] = [
        // ADD8: 0xff + 0x01 wraps to 0 and carries; 0x7f + 0x01 is 0x80 and does not carry
        ("uadd8", 0x7fff_7fff, 0x0101_0101, 0x8000_8000, Some(0b0101)),
        // signed: 0x7f + 0x01 is +128 (>= 0, GE set); 0xff + 0x01 is -1 + 1 = 0 (GE set);
        // 0x80 + 0xff is -129 (GE clear)
        ("sadd8", 0x7fff_ff80, 0x0101_01ff, 0x8000_007f, Some(0b1110)),
        // SUB8 sets GE when the difference is >= 0, so equal bytes set it
        ("usub8", 0x0510_ff00, 0x0610_fe01, 0xff00_01ff, Some(0b0110)),
        ("ssub8", 0x8001_7f00, 0x0100_8001, 0x7f01_ffff, Some(0b0110)),
        // ASX: low = Rn low - Rm high, high = Rn high + Rm low
        ("uasx", 0x0003_0005, 0x0002_0007, 0x000a_0003, Some(0b0011)),
        // SAX: low = Rn low + Rm high, high = Rn high - Rm low
        ("usax", 0x0003_0005, 0x0002_0007, 0xfffc_0007, Some(0b0000)),
        ("sasx", 0x8000_0001, 0x0001_8000, 0x0000_0000, Some(0b0011)),
        ("ssax", 0x7fff_8000, 0x8000_0001, 0x7ffe_0000, Some(0b1100)),
        // saturating
        ("qadd16", 0x7fff_8000, 0x0001_ffff, 0x7fff_8000, None),
        ("qsub8", 0x7f80_017f, 0xff7f_80ff, 0x7f80_7f7f, None),
        ("uqadd8", 0xf001_ff80, 0x2002_0180, 0xff03_ffff, None),
        ("uqsax", 0xffff_0001, 0xffff_0002, 0xfffd_ffff, None),
        // halving: signed -1 + -1 = -2 halved is -1; unsigned 0 - 1 = -1 is 0x1ffff, and its
        // bits 16:1 are 0xffff
        ("shadd16", 0xffff_0003, 0xffff_0004, 0xffff_0003, None),
        ("uhsub16", 0x0000_0005, 0x0001_0004, 0xffff_0000, None),
        ("uhadd8", 0xffff_ff01, 0xffff_ff02, 0xffff_ff01, None),
        ("shsax", 0x0004_0006, 0x0002_0008, 0xfffe_0004, None),
    ];
    for (name, rn, rm, expected, ge) in cases {
        let core = run(
            encode(name, 1, 2, 3),
            &[(Reg::R2, rn), (Reg::R3, rm)],
            0b1001,
            false,
        );
        assert_eq!(
            core.get_r(Reg::R1),
            expected,
            "{name} {rn:#010x} {rm:#010x}"
        );
        assert_eq!(ge_of(&core), ge.unwrap_or(0b1001), "GE of {name}");
        // and the model agrees with the numbers worked out by hand
        assert_eq!(model(name, rn, rm), (expected, ge), "model of {name}");
    }
}

#[test]
fn test_a_failed_it_condition_skips_the_instruction() {
    let mut core = Processor::new();
    core.set_r(Reg::R1, 0x55);
    core.set_r(Reg::R2, 1);
    core.psr.value.set_bits(16..20, 0b0101);
    core.psr.set_z_bit(true);
    // IT NE with Z set: the condition fails
    core.set_itstate(((crate::core::condition::Condition::NE.value() as u8) << 4) | 0b1000);
    let outcome = core
        .execute_internal(&decode_32(encode("uadd8", 1, 2, 2)))
        .unwrap();
    assert!(matches!(outcome, crate::executor::ExecuteSuccess::NotTaken));
    assert_eq!(core.get_r(Reg::R1), 0x55);
    assert_eq!(ge_of(&core), 0b0101);
}
