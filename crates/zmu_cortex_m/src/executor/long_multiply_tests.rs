//! The long multiplies of the DSP extension: SMLALBB, SMLALBT, SMLALTB and SMLALTT (A7.7.139),
//! SMLALD and SMLALDX (A7.7.140), SMLSLD and SMLSLDX (A7.7.143), and UMAAL (A7.7.202).
//!
//! The opcodes come from the diagrams and llvm-mc, with the operands in the manual's order,
//! `RdLo`, `RdHi`, Rn, Rm. The expected values come from `model`, which works on unbounded integers
//! and wraps the 64-bit result only at the end. None of them sets Q or the flags: the 64-bit
//! addition wraps without being detected.

use super::dsp_test_support::{
    Case, LONG_LAYOUTS, assert_untouched, cases, core_with, is_udf, run,
};
use crate::Processor;
use crate::core::condition::Condition;
use crate::core::register::{Apsr, BaseReg, Reg};
use crate::decoder::decode_32;

/// Every instruction here, by name: the name decides everything, so the encoder and the model
/// read it
const NAMES: [&str; 9] = [
    "smlalbb", "smlalbt", "smlaltb", "smlaltt", "smlald", "smlaldx", "smlsld", "smlsldx", "umaal",
];

/// `RdLo` and `RdHi` in hw2<15:12> and <11:8>; hw1 is 1111 1011 1 op1 Rn, with hw2<7:4> = 10 N M
/// for SMLAL<x><y>, 110 X for SMLALD and SMLSLD, and 0110 for UMAAL
fn encode(name: &str, rdlo: u32, rdhi: u32, rn: u32, rm: u32) -> u32 {
    let (hw1, bits) = match name {
        "smlalbb" => (0xfbc0, 0x80),
        "smlalbt" => (0xfbc0, 0x90),
        "smlaltb" => (0xfbc0, 0xa0),
        "smlaltt" => (0xfbc0, 0xb0),
        "smlald" => (0xfbc0, 0xc0),
        "smlaldx" => (0xfbc0, 0xd0),
        "smlsld" => (0xfbd0, 0xc0),
        "smlsldx" => (0xfbd0, 0xd0),
        "umaal" => (0xfbe0, 0x60),
        _ => unreachable!(),
    };
    (hw1 | rn) << 16 | rdlo << 12 | rdhi << 8 | bits | rm
}

/// The pseudocode of each: the 64-bit result, whose two halves go to `RdHi` and `RdLo`
fn model(name: &str, rn: u32, rm: u32, rdlo: u32, rdhi: u32) -> u64 {
    let accumulator = i128::from(((u64::from(rdhi) << 32) | u64::from(rdlo)) as i64);
    let half = |value: u32, shift: u32| i128::from((value >> shift) as u16 as i16);
    let result = if name == "umaal" {
        i128::from(rn) * i128::from(rm) + i128::from(rdhi) + i128::from(rdlo)
    } else if let Some(xy) = name
        .strip_prefix("smlal")
        .filter(|rest| rest.chars().all(|c| c == 'b' || c == 't'))
    {
        // the letters are N then M: B is the bottom halfword and T the top
        let n_shift = if xy.starts_with('t') { 16 } else { 0 };
        let m_shift = if xy.ends_with('t') { 16 } else { 0 };
        half(rn, n_shift) * half(rm, m_shift) + accumulator
    } else {
        let swap = name.ends_with('x');
        let operand2 = if swap { rm.rotate_right(16) } else { rm };
        let product1 = half(rn, 0) * half(operand2, 0);
        let product2 = half(rn, 16) * half(operand2, 16);
        let sum = if name.starts_with("smlsld") {
            product1 - product2
        } else {
            product1 + product2
        };
        sum + accumulator
    };
    (result & 0xffff_ffff_ffff_ffff) as u64
}

fn check(core: &mut Processor, name: &str, case: &Case) {
    let [rdlo, rdhi, rn_number, rm_number] = case.layout;
    let opcode = encode(name, rdlo, rdhi, rn_number, rm_number);
    case.load(core);
    let before = core.r0_12;
    let read = |core: &crate::Processor, number: u32| core.get_r(Reg::from(number as u8));
    let (rn, rm) = (read(core, rn_number), read(core, rm_number));
    let (lo, hi) = (read(core, rdlo), read(core, rdhi));
    run(core, opcode);
    let expected = model(name, rn, rm, lo, hi);
    let context = format!("{name} {opcode:#010x} {}", case.describe());
    assert_eq!(read(core, rdlo), expected as u32, "RdLo of {context}");
    assert_eq!(
        read(core, rdhi),
        (expected >> 32) as u32,
        "RdHi of {context}"
    );
    // no Q and no flags
    assert_eq!(core.psr.value, case.psr, "flags of {context}");
    assert_untouched(core, &before, &[rdlo, rdhi], &context);
}

#[test]
fn test_the_opcodes_are_the_manuals_and_llvms_and_decode_to_the_right_name() {
    // llvm-mc: `name r1, r2, r3, r4` is RdLo = r1, RdHi = r2, Rn = r3, Rm = r4
    let opcodes = [
        ("smlalbb", 0xfbc3_1284),
        ("smlalbt", 0xfbc3_1294),
        ("smlaltb", 0xfbc3_12a4),
        ("smlaltt", 0xfbc3_12b4),
        ("smlald", 0xfbc3_12c4),
        ("smlaldx", 0xfbc3_12d4),
        ("smlsld", 0xfbd3_12c4),
        ("smlsldx", 0xfbd3_12d4),
        ("umaal", 0xfbe3_1264),
    ];
    for (name, opcode) in opcodes {
        assert_eq!(encode(name, 1, 2, 3, 4), opcode, "{name}");
        assert_eq!(
            decode_32(opcode).to_string(),
            format!("{name} r1, r2, r3, r4")
        );
    }
}

#[test]
fn test_the_edge_and_random_values_match_the_model_in_every_layout() {
    let mut core = Processor::new();
    for case in cases(&LONG_LAYOUTS, 4, 2500, 0x2e2e_9d9d_6464_1b1b) {
        for name in NAMES {
            check(&mut core, name, &case);
        }
    }
}

#[test]
fn test_the_worked_examples() {
    // (name, Rn, Rm, RdLo, RdHi, result)
    let examples: [(&str, u32, u32, u32, u32, u64); 18] = [
        // 3 * 5 + 100
        ("smlalbb", 0x0002_0003, 0x0004_0005, 100, 0, 115),
        // the halves are picked by the letters, N then M: the top of Rn is 2, the bottom of Rm 5
        ("smlaltb", 0x0002_0003, 0x0004_0005, 100, 0, 110),
        ("smlalbt", 0x0002_0003, 0x0004_0005, 100, 0, 112),
        ("smlaltt", 0x0002_0003, 0x0004_0005, 100, 0, 108),
        // signed: -1 * 1 is -1 added to a 64-bit accumulate, borrowing from RdHi
        ("smlalbb", 0xffff, 1, 0, 1, 0x0000_0000_ffff_ffff),
        // the 64-bit addition wraps without a flag
        ("smlalbb", 1, 1, 0xffff_ffff, 0xffff_ffff, 0),
        (
            "smlalbb",
            0x8000,
            0x7fff,
            0,
            0x8000_0000,
            0x7fff_ffff_c000_8000,
        ),
        // SMLALD: 3 * 5 + 2 * 4 + accumulate, and swapped 3 * 4 + 2 * 5
        ("smlald", 0x0002_0003, 0x0004_0005, 1, 0, 24),
        ("smlaldx", 0x0002_0003, 0x0004_0005, 1, 0, 23),
        // SMLSLD: 3 * 5 - 2 * 4 and swapped 3 * 4 - 2 * 5
        ("smlsld", 0x0002_0003, 0x0004_0005, 10, 0, 17),
        ("smlsldx", 0x0002_0003, 0x0004_0005, 10, 0, 12),
        // 2^30 + 2^30 into a 64-bit accumulate: no overflow, no Q
        ("smlald", 0x8000_8000, 0x8000_8000, 0, 0, 0x8000_0000),
        ("smlsld", 0x8000_8000, 0x8000_8000, 0, 0, 0),
        // the accumulate wraps at 2^64
        (
            "smlald",
            0x0000_0001,
            0x0000_0001,
            0xffff_ffff,
            0xffff_ffff,
            0,
        ),
        (
            "smlsld",
            0x0000_0001,
            0x0000_0001,
            0,
            0x8000_0000,
            0x8000_0000_0000_0001,
        ),
        // UMAAL: unsigned 32 x 32 plus both registers cannot overflow 64 bits
        (
            "umaal",
            0xffff_ffff,
            0xffff_ffff,
            0xffff_ffff,
            0xffff_ffff,
            0xffff_ffff_ffff_ffff,
        ),
        ("umaal", 3, 5, 7, 11, 33),
        ("umaal", 0x8000_0000, 2, 1, 0, 0x1_0000_0001),
    ];
    for (name, rn, rm, lo, hi, expected) in examples {
        let mut core = core_with(&[(3, rn), (4, rm), (1, lo), (2, hi)], 0);
        run(&mut core, encode(name, 1, 2, 3, 4));
        let context = format!("{name} {rn:#x} {rm:#x} lo {lo:#x} hi {hi:#x}");
        let got = u64::from(core.get_r(Reg::R2)) << 32 | u64::from(core.get_r(Reg::R1));
        assert_eq!(got, expected, "{context}");
        assert_eq!(model(name, rn, rm, lo, hi), expected, "model of {context}");
        assert_eq!(core.psr.value, 0, "flags of {context}");
    }
}

#[test]
fn test_a_failed_it_condition_skips_the_instruction() {
    use super::ExecutorHelper;
    for name in NAMES {
        let mut core = core_with(&[(3, 3), (4, 9)], 0);
        core.psr.set_z_bit(true);
        core.set_itstate(((Condition::NE.value() as u8) << 4) | 0b1000);
        let outcome = core
            .execute_internal(&decode_32(encode(name, 1, 2, 3, 4)))
            .unwrap();
        assert!(matches!(outcome, super::ExecuteSuccess::NotTaken), "{name}");
        for register in [1, 2] {
            assert_eq!(
                core.get_r(Reg::from(register as u8)),
                super::dsp_test_support::sentinel(register),
                "{name}"
            );
        }
    }
}

#[test]
fn test_the_registers_the_manual_forbids_are_undefined() {
    for name in NAMES {
        for bad in [13, 15] {
            for opcode in [
                encode(name, bad, 2, 3, 4),
                encode(name, 1, bad, 3, 4),
                encode(name, 1, 2, bad, 4),
                encode(name, 1, 2, 3, bad),
            ] {
                assert!(is_udf(&decode_32(opcode)), "{name} {opcode:#010x}");
            }
        }
        // RdHi = RdLo is UNPREDICTABLE
        assert!(is_udf(&decode_32(encode(name, 1, 1, 3, 4))), "{name}");
    }
}

#[test]
fn test_the_unallocated_neighbours_are_undefined_and_the_allocated_ones_are_the_old_ones() {
    // Table A5-29: op1 = 100 has op2 = 0000, 10xx and 110x, 101 has 110x, and 110 has 0000 and
    // 0110; hw2<7:4> is op2. Everything else in these three rows is UNDEFINED.
    for hw1 in [0xfbc3u32, 0xfbd3, 0xfbe3] {
        for op2 in 0..16u32 {
            let opcode = hw1 << 16 | 0x1204 | op2 << 4;
            let allocated = match hw1 {
                0xfbc3 => op2 == 0 || op2 >> 2 == 0b10 || op2 >> 1 == 0b110,
                0xfbd3 => op2 >> 1 == 0b110,
                _ => op2 == 0 || op2 == 0b0110,
            };
            let text = decode_32(opcode).to_string();
            assert_eq!(
                !is_udf(&decode_32(opcode)),
                allocated,
                "{opcode:#010x} {text}"
            );
        }
    }
    // SMLAL and UMLAL themselves are what they were
    assert_eq!(decode_32(0xfbc3_1204).to_string(), "smlal r1, r2, r3, r4");
    assert_eq!(decode_32(0xfbe3_1204).to_string(), "umlal r1, r2, r3, r4");
}
