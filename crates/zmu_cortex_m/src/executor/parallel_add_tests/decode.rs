//! Encodings that must not decode as a parallel add or subtract.

use super::model::encode;
use super::*;

#[test]
fn test_the_unallocated_encodings_are_undefined() {
    for op1 in 0..8u32 {
        for unsigned in 0..2u32 {
            for op2 in 0..4u32 {
                let allocated = ![0b011, 0b111].contains(&op1) && op2 != 0b11;
                let opcode = (0xfa82 | op1 << 4) << 16 | 0xf103 | unsigned << 6 | op2 << 4;
                assert_eq!(
                    !is_udf(&decode_32(opcode)),
                    allocated,
                    "op1 {op1:03b} U {unsigned} op2 {op2:02b} ({opcode:#010x})"
                );
            }
        }
    }
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
fn test_the_neighbouring_encodings_are_not_taken() {
    // hw2 bit 7 set is the miscellaneous space (QADD, REV, SEL, CLZ), not a parallel add
    for opcode in [
        0xfa82_f183,
        0xfa82_f1c3,
        0xfa92_f1a3,
        0xfaa2_f183,
        0xfab2_f183,
    ] {
        assert!(
            !matches!(decode_32(opcode), Instruction::PARALLEL { .. }),
            "{opcode:#010x}"
        );
    }
    // hw2<15:12> != 1111 is undefined
    assert!(is_udf(&decode_32(0xfa82_0103)));
}
