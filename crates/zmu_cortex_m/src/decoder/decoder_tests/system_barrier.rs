use super::*;

#[test]
fn test_decode_dmb() {
    //  f3bf 8f5f       dmb sy
    assert_eq!(decode_32(0xf3bf_8f5f), Instruction::DMB);
}

#[test]
fn test_decode_it() {
    // ITT MI
    assert_eq!(
        decode_16(0xbf44),
        Instruction::IT {
            x: Some(ITCondition::Then),
            y: None,
            z: None,
            firstcond: Condition::MI,
            mask: 0x4,
        }
    );
}

#[test]
fn test_decode_itt_cc() {
    // 0xbf3c ITTCC
    assert_eq!(
        decode_16(0xbf3c),
        Instruction::IT {
            x: Some(ITCondition::Then),
            y: None,
            z: None,
            firstcond: Condition::CC,
            mask: 0b1100,
        }
    );
}

#[test]
fn test_decode_itttt_cc() {
    // 0xbf3f ITTTT CC
    assert_eq!(
        decode_16(0xbf3f),
        Instruction::IT {
            x: Some(ITCondition::Then),
            y: Some(ITCondition::Then),
            z: Some(ITCondition::Then),
            firstcond: Condition::CC,
            mask: 0b1111,
        }
    );
}

// MSR (A7.7.83): the mask in hw2<11:10> picks the APSR fields, 01 the GE bits, 10 the flags NZCVQ
// and 11 both. The three were one encoding, mask 10 only; 01 and 11 decoded as a branch.
#[test]
fn test_decode_msr_apsr_masks() {
    // msr apsr_g, r1 / msr apsr_nzcvq, r1 / msr apsr_nzcvqg, r1, from llvm-mc
    for (opcode, mask) in [
        (0xf381_8400, 0b01),
        (0xf381_8800, 0b10),
        (0xf381_8c00, 0b11),
    ] {
        assert_eq!(
            decode_32(opcode),
            Instruction::MSR_reg {
                params: MsrParams {
                    rn: Reg::R1,
                    sysm: 0,
                    mask
                }
            },
            "{opcode:#010x}"
        );
    }
    // msr control, r1 is f381 8814
    assert!(matches!(
        decode_32(0xf381_8814),
        Instruction::MSR_reg {
            params: MsrParams {
                mask: 0b10,
                sysm: 20,
                ..
            }
        }
    ));
}

#[test]
fn test_decode_msr_with_an_unpredictable_mask_is_udf() {
    // mask 00 is UNPREDICTABLE, and so is any mask but 10 for a SYSm other than 0 to 3
    for opcode in [0xf381_8000, 0xf381_8414, 0xf381_8c14, 0xf381_8408] {
        assert!(
            matches!(decode_32(opcode), Instruction::UDF { .. }),
            "{opcode:#010x}"
        );
    }
}
