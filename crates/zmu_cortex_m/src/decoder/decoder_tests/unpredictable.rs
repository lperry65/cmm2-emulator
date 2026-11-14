use super::*;
use crate::core::instruction::Reg2FullParams;

// Each row is an instruction with a normal destination register, and the bit position of the
// destination field in the opcode. Putting PC (0b1111) in that field is UNPREDICTABLE, and so
// it decodes as undefined. The decoder must not hand the executor a plain register write to PC.
const WRITES_A_REGISTER: &[(&str, u32, usize)] = &[
    ("adc.w r1, r2, r3", 0xeb42_0103, 8),
    ("adc r1, r2, #1", 0xf142_0101, 8),
    ("add.w r1, r2, #1", 0xf102_0101, 8),
    ("addw r1, r2, #1", 0xf202_0101, 8),
    ("and.w r1, r2, r3", 0xea02_0103, 8),
    ("and r1, r2, #1", 0xf002_0101, 8),
    ("asr.w r1, r2, #3", 0xea4f_01e2, 8),
    ("asr.w r1, r2, r3", 0xfa42_f103, 8),
    ("bfc r1, #4, #4", 0xf36f_1107, 8),
    ("bfi r1, r2, #4, #4", 0xf362_1107, 8),
    ("bic.w r1, r2, r3", 0xea22_0103, 8),
    ("bic r1, r2, #1", 0xf022_0101, 8),
    ("clz r1, r2", 0xfab2_f182, 8),
    ("eor.w r1, r2, r3", 0xea82_0103, 8),
    ("eor r1, r2, #1", 0xf082_0101, 8),
    ("ldrb r1, [r2, #4]!", 0xf812_1f04, 12),
    ("ldrh r1, [r2, #4]!", 0xf832_1f04, 12),
    ("ldrsb r1, [r2, #4]!", 0xf912_1f04, 12),
    ("ldrsh r1, [r2, #4]!", 0xf932_1f04, 12),
    ("ldrd r1, r4, [r2, #8]", 0xe9d2_1402, 12),
    ("ldrd r4, r1, [r2, #8]", 0xe9d2_4102, 8),
    ("ldrex r1, [r2]", 0xe852_1f00, 12),
    ("ldrexb r1, [r2]", 0xe8d2_1f4f, 12),
    ("ldrexh r1, [r2]", 0xe8d2_1f5f, 12),
    ("lsl.w r1, r2, #3", 0xea4f_01c2, 8),
    ("lsl.w r1, r2, r3", 0xfa02_f103, 8),
    ("lsr.w r1, r2, #3", 0xea4f_01d2, 8),
    ("lsr.w r1, r2, r3", 0xfa22_f103, 8),
    ("ror.w r1, r2, #3", 0xea4f_01f2, 8),
    ("ror.w r1, r2, r3", 0xfa62_f103, 8),
    ("mla r1, r2, r3, r4", 0xfb02_4103, 8),
    ("mls r1, r2, r3, r4", 0xfb02_4113, 8),
    ("movt r1, #1234", 0xf2c0_41d2, 8),
    ("mov.w r1, #0x12000", 0xf44f_3190, 8),
    ("movw r1, #0x1234", 0xf241_2134, 8),
    ("mrs r1, apsr", 0xf3ef_8100, 8),
    ("mvn r1, #1", 0xf06f_0101, 8),
    ("mvn.w r1, r2", 0xea6f_0102, 8),
    ("mul r1, r2, r3", 0xfb02_f103, 8),
    ("orn r1, r2, r3", 0xea62_0103, 8),
    ("orn r1, r2, #1", 0xf062_0101, 8),
    ("orr.w r1, r2, r3", 0xea42_0103, 8),
    ("orr r1, r2, #1", 0xf042_0101, 8),
    ("rbit r1, r2", 0xfa92_f1a2, 8),
    ("rev.w r1, r2", 0xfa92_f182, 8),
    ("rev16.w r1, r2", 0xfa92_f192, 8),
    ("revsh.w r1, r2", 0xfa92_f1b2, 8),
    ("rrx r1, r2", 0xea4f_0132, 8),
    ("rsb r1, r2, #1", 0xf1c2_0101, 8),
    ("rsb r1, r2, r3", 0xebc2_0103, 8),
    ("sbc r1, r2, #1", 0xf162_0101, 8),
    ("sbc.w r1, r2, r3", 0xeb62_0103, 8),
    ("sbfx r1, r2, #4, #4", 0xf342_1103, 8),
    ("sdiv r1, r2, r3", 0xfb92_f1f3, 8),
    ("smlal r4, r1, r2, r3", 0xfbc2_4103, 8),
    ("smlal r1, r4, r2, r3", 0xfbc2_1403, 12),
    ("smull r4, r1, r2, r3", 0xfb82_4103, 8),
    ("smull r1, r4, r2, r3", 0xfb82_1403, 12),
    ("ssat r1, #8, r2", 0xf302_0107, 8),
    ("strex r1, r2, [r3]", 0xe843_2100, 8),
    ("strexb r1, r2, [r3]", 0xe8c3_2f41, 0),
    ("strexh r1, r2, [r3]", 0xe8c3_2f51, 0),
    ("sub.w r1, r2, #1", 0xf1a2_0101, 8),
    ("subw r1, r2, #1", 0xf2a2_0101, 8),
    ("sub.w r1, r2, r3", 0xeba2_0103, 8),
    ("sxtb.w r1, r2", 0xfa4f_f182, 8),
    ("sxth.w r1, r2", 0xfa0f_f182, 8),
    ("ubfx r1, r2, #4, #4", 0xf3c2_1103, 8),
    ("udiv r1, r2, r3", 0xfbb2_f1f3, 8),
    ("umlal r4, r1, r2, r3", 0xfbe2_4103, 8),
    ("umlal r1, r4, r2, r3", 0xfbe2_1403, 12),
    ("umull r4, r1, r2, r3", 0xfba2_4103, 8),
    ("umull r1, r4, r2, r3", 0xfba2_1403, 12),
    ("usat r1, #8, r2", 0xf382_0108, 8),
    ("uxtb.w r1, r2", 0xfa5f_f182, 8),
    ("uxth.w r1, r2", 0xfa1f_f182, 8),
];

// These are there only with the DSP extension, and are undefined without it.
#[cfg(feature = "has-dsp-ext")]
const DSP_WRITES_A_REGISTER: &[(&str, u32, usize)] = &[
    ("pkhbt r1, r2, r3", 0xeac2_0103, 8),
    ("smlabb r1, r2, r3, r4", 0xfb12_4103, 8),
    ("smulbb r1, r2, r3", 0xfb12_f103, 8),
    ("sxtab r1, r2, r3", 0xfa42_f183, 8),
    ("sxtah r1, r2, r3", 0xfa02_f183, 8),
    ("uadd8 r1, r2, r3", 0xfa82_f143, 8),
    ("sel r1, r2, r3", 0xfaa2_f183, 8),
    ("uxtab r1, r2, r3", 0xfa52_f183, 8),
    ("uxtah r1, r2, r3", 0xfa12_f183, 8),
];

fn is_udf(instruction: &Instruction) -> bool {
    matches!(instruction, Instruction::UDF { .. })
}

fn assert_pc_destination_is_udf(rows: &[(&str, u32, usize)]) {
    for &(asm, opcode, shift) in rows {
        assert!(
            !is_udf(&decode_32(opcode)),
            "{asm} ({opcode:#010x}) should decode"
        );
        let with_pc = opcode | (0xf << shift);
        assert!(
            is_udf(&decode_32(with_pc)),
            "{asm} with PC as the destination ({with_pc:#010x}) should be UDF"
        );
    }
}

#[test]
fn test_destination_pc_decodes_as_udf() {
    assert_pc_destination_is_udf(WRITES_A_REGISTER);
}

#[cfg(feature = "has-dsp-ext")]
#[test]
fn test_dsp_destination_pc_decodes_as_udf() {
    assert_pc_destination_is_udf(DSP_WRITES_A_REGISTER);
}

#[test]
fn test_destinations_that_may_be_pc_still_decode() {
    // ldr.w pc, [r1, #4]
    assert!(!is_udf(&decode_32(0xf8d1_f004)));
    // ldr.w pc, [r1, r2]
    assert!(!is_udf(&decode_32(0xf851_f002)));
    // ldr.w pc, [sp], #4 (pop {pc})
    assert!(!is_udf(&decode_32(0xf85d_fb04)));
    // ldm.w sp!, {r0, pc}
    assert!(!is_udf(&decode_32(0xe8bd_8001)));
}

#[test]
fn test_write_back_to_pc_decodes_as_udf() {
    // strd r1, r2, [r3, #8]! with the base changed to PC
    assert!(!is_udf(&decode_32(0xe9e3_1202)));
    assert!(is_udf(&decode_32(0xe9ef_1202)));
    // str r1, [r3, #4]! with the base changed to PC
    assert!(!is_udf(&decode_32(0xf843_1f04)));
    assert!(is_udf(&decode_32(0xf84f_1f04)));
    // stm.w r3!, {r1, r2} with the base changed to PC
    assert!(!is_udf(&decode_32(0xe8a3_0006)));
    assert!(is_udf(&decode_32(0xe8af_0006)));
}

fn is_nop(instruction: &Instruction) -> bool {
    matches!(instruction, Instruction::NOP { thumb32: true })
}

// A load of a halfword or signed byte with Rt = PC is not an instruction in its plain forms but a
// memory hint (Table A5-21 and A5-22): an unallocated hint for LDRH and LDRSH, PLI for LDRSB. The
// hint does nothing here, like PLD. Only the forms with write back are UNPREDICTABLE.
#[test]
fn test_halfword_and_signed_loads_into_pc_are_hints() {
    for opcode in [
        0xf8b0_f004, // ldrh.w pc, [r0, #4]
        0xf830_fc04, // ldrh pc, [r0, #-4]
        0xf830_f001, // ldrh.w pc, [r0, r1]
        0xf830_f011, // ldrh.w pc, [r0, r1, lsl #1]
        0xf8bf_f004, // ldrh pc, [pc, #4]
        0xf83f_f004, // ldrh pc, [pc, #-4]
        0xf9b0_f004, // ldrsh pc, [r0, #4]
        0xf930_fc04, // ldrsh pc, [r0, #-4]
        0xf930_f001, // ldrsh.w pc, [r0, r1]
        0xf930_f011, // ldrsh.w pc, [r0, r1, lsl #1]
        0xf9bf_f004, // ldrsh pc, [pc, #4]
        0xf93f_f004, // ldrsh pc, [pc, #-4]
    ] {
        assert!(is_nop(&decode_32(opcode)), "{opcode:#010x}");
    }
    // the write back forms are UNPREDICTABLE
    for opcode in [0xf830_ff04, 0xf830_fb04, 0xf930_ff04, 0xf830_fd04] {
        assert!(is_udf(&decode_32(opcode)), "{opcode:#010x}");
    }
}

#[test]
fn test_pli_is_a_hint_in_every_form() {
    for opcode in [
        0xf991_f004, // pli [r1, #4]
        0xf911_fc04, // pli [r1, #-4]
        0xf911_f002, // pli [r1, r2]
        0xf911_f012, // pli [r1, r2, lsl #1]
        0xf99f_f004, // pli [pc, #4]
        0xf91f_f004, // pli [pc, #-4]
    ] {
        assert!(is_nop(&decode_32(opcode)), "{opcode:#010x}");
    }
}

#[test]
fn test_pld_with_pc_as_the_base_is_the_literal_form() {
    use crate::core::instruction::Instruction::PLD_lit;
    assert_eq!(
        decode_32(0xf89f_f004),
        PLD_lit {
            imm32: 4,
            add: true
        }
    );
    // pld [pc, #-4]: U is 0, and the offset is the 12 bits, not a register
    assert_eq!(
        decode_32(0xf81f_f004),
        PLD_lit {
            imm32: 4,
            add: false
        }
    );
    // and the same through the negative 8-bit form
    assert_eq!(
        decode_32(0xf81f_fc04),
        PLD_lit {
            imm32: 0xc04,
            add: false
        }
    );
}

fn literal(make: fn(Reg2FullParams) -> Instruction, rt: Reg, imm32: u32, add: bool) -> Instruction {
    make(Reg2FullParams {
        rt,
        rn: Reg::PC,
        imm32,
        index: true,
        add,
        wback: false,
    })
}

fn ldrh(params: Reg2FullParams) -> Instruction {
    Instruction::LDRH_imm {
        params,
        thumb32: true,
    }
}
fn ldrsh(params: Reg2FullParams) -> Instruction {
    Instruction::LDRSH_imm {
        params,
        thumb32: true,
    }
}
fn ldrb(params: Reg2FullParams) -> Instruction {
    Instruction::LDRB_imm {
        params,
        thumb32: true,
    }
}
fn ldrsb(params: Reg2FullParams) -> Instruction {
    Instruction::LDRSB_imm {
        params,
        thumb32: true,
    }
}

// With Rn = PC the encodings are the literal forms, whatever the bits below say: U is bit 23 and
// the offset the 12 bits, so that a register form or an unprivileged form with Rn = PC is the
// literal load of its own size and not LDRx_reg of PC or an undefined instruction.
#[test]
fn test_byte_and_halfword_loads_with_pc_as_the_base_are_literal_loads() {
    // the plain literal forms
    assert_eq!(decode_32(0xf8bf_1004), literal(ldrh, Reg::R1, 4, true));
    assert_eq!(decode_32(0xf83f_1004), literal(ldrh, Reg::R1, 4, false));
    assert_eq!(decode_32(0xf9bf_1004), literal(ldrsh, Reg::R1, 4, true));
    assert_eq!(decode_32(0xf89f_1004), literal(ldrb, Reg::R1, 4, true));
    assert_eq!(decode_32(0xf91f_1004), literal(ldrsb, Reg::R1, 4, false));
    // the register form with Rn = PC (U = 0, and the offset is bits 11:0)
    assert_eq!(decode_32(0xf83f_1001), literal(ldrh, Reg::R1, 1, false));
    assert_eq!(decode_32(0xf93f_1001), literal(ldrsh, Reg::R1, 1, false));
    assert_eq!(decode_32(0xf81f_1001), literal(ldrb, Reg::R1, 1, false));
    assert_eq!(decode_32(0xf91f_1001), literal(ldrsb, Reg::R1, 1, false));
    // the unprivileged form with Rn = PC (op2 = 1110)
    assert_eq!(decode_32(0xf83f_1e04), literal(ldrh, Reg::R1, 0xe04, false));
    assert_eq!(
        decode_32(0xf93f_1e04),
        literal(ldrsh, Reg::R1, 0xe04, false)
    );
    assert_eq!(decode_32(0xf81f_1e04), literal(ldrb, Reg::R1, 0xe04, false));
    assert_eq!(
        decode_32(0xf91f_1e04),
        literal(ldrsb, Reg::R1, 0xe04, false)
    );
    // and with Rt = PC they are the hints
    assert!(is_nop(&decode_32(0xf83f_fe04)));
    assert!(is_nop(&decode_32(0xf93f_fe04)));
    assert!(is_nop(&decode_32(0xf83f_f001)));
    assert!(is_nop(&decode_32(0xf91f_f001))); // PLI
    assert_eq!(
        decode_32(0xf81f_f001),
        Instruction::PLD_lit {
            imm32: 1,
            add: false
        }
    );
}

// The 8-bit offset forms with P = 0 and W = 0 are not LDRx or STRx: that space is other
// instructions, and these encodings are UNDEFINED.
#[test]
fn test_offset_forms_without_index_or_write_back_are_undefined() {
    for opcode in [
        0xf850_1804, // ldr r1, [r0] with P = 0, W = 0
        0xf810_1804, // ldrb
        0xf830_1804, // ldrh
        0xf910_1804, // ldrsb
        0xf930_1804, // ldrsh
        0xf840_1804, // str
        0xf800_1804, // strb
        0xf820_1804, // strh
    ] {
        assert!(is_udf(&decode_32(opcode)), "{opcode:#010x}");
        // and with P = 1 they are the pre-indexed forms
        assert!(!is_udf(&decode_32(opcode | 0x400)), "{opcode:#010x} with P");
    }
}

// Rt = SP is UNPREDICTABLE for the byte and halfword loads (all addressing forms, the literal one
// too), for STRB and STRH, for LDRD and STRD (Rt and Rt2, and Rt = Rt2 for the loads), for
// LDREX* and STREX* (Rt, and Rd of the stores). LDR and STR may use SP (STR may not store PC). Each
// row is a valid encoding with R1 in the field, the position of the field, and the register put in.
#[test]
fn test_registers_the_manual_forbids_decode_as_udf() {
    // (opcode, field shift, forbidden register numbers)
    let rows: &[(u32, usize, &[u32])] = &[
        (0xf890_1004, 12, &[13]),     // ldrb.w r1, [r0, #4]
        (0xf8b0_1004, 12, &[13]),     // ldrh.w
        (0xf990_1004, 12, &[13]),     // ldrsb.w
        (0xf9b0_1004, 12, &[13]),     // ldrsh.w
        (0xf810_1001, 12, &[13]),     // ldrb.w r1, [r0, r1]
        (0xf89f_1004, 12, &[13]),     // ldrb.w r1, [pc, #4]
        (0xf8bf_1004, 12, &[13]),     // ldrh.w r1, [pc, #4]
        (0xf9bf_1004, 12, &[13]),     // ldrsh.w r1, [pc, #4]
        (0xf880_1004, 12, &[13, 15]), // strb.w
        (0xf8a0_1004, 12, &[13, 15]), // strh.w
        (0xf800_1002, 12, &[13, 15]), // strb.w r1, [r0, r2]
        (0xf820_1002, 12, &[13, 15]), // strh.w r1, [r0, r2]
        (0xf8c0_1004, 12, &[15]),     // str.w r1, [r0, #4]: SP may be stored
        (0xf840_1002, 12, &[15]),     // str.w r1, [r0, r2]
        (0xe9d0_1200, 12, &[13]),     // ldrd r1, r2, [r0]
        (0xe9d0_1200, 8, &[13, 15]),
        (0xe9c0_1200, 12, &[13, 15]), // strd
        (0xe9c0_1200, 8, &[13, 15]),
        (0xe850_1f00, 12, &[13]),     // ldrex r1, [r0]
        (0xe8d0_1f4f, 12, &[13]),     // ldrexb
        (0xe840_1200, 12, &[13, 15]), // strex r2, r1, [r0]
        (0xe840_1200, 8, &[13, 15]),
        (0xe8c0_1f52, 12, &[13, 15]), // strexh r2, r1, [r0]
        (0xe8c0_1f52, 0, &[13, 15]),
    ];
    for &(opcode, shift, forbidden) in rows {
        assert!(!is_udf(&decode_32(opcode)), "{opcode:#010x} should decode");
        for &register in forbidden {
            let patched = opcode & !(0xf << shift) | register << shift;
            assert!(
                is_udf(&decode_32(patched)),
                "{patched:#010x} (register {register} at bit {shift}) should be UDF"
            );
        }
    }
    // LDR and STR of SP are allowed, and LDRD with the same register twice is not
    assert!(!is_udf(&decode_32(0xf8d0_d004))); // ldr.w sp, [r0, #4]
    assert!(!is_udf(&decode_32(0xf8c0_d004))); // str.w sp, [r0, #4]
    assert!(is_udf(&decode_32(0xe9d0_1100))); // ldrd r1, r1, [r0]
}

// The register forms of the loads and stores have 000000 in bits 11:6. LDRB's was 00000 and so took
// the encodings with 000001 there, which are undefined.
#[test]
fn test_register_forms_need_zeros_in_bits_11_to_6() {
    for opcode in [
        0xf850_1002, // ldr.w r1, [r0, r2]
        0xf830_1002, // ldrh
        0xf810_1002, // ldrb
        0xf910_1002, // ldrsb
        0xf930_1002, // ldrsh
        0xf840_1002, // str
        0xf820_1002, // strh
        0xf800_1002, // strb
    ] {
        assert!(!is_udf(&decode_32(opcode)), "{opcode:#010x}");
        for bit in 6..12 {
            // bit 11 set makes it an offset form, which is another instruction
            if bit == 11 {
                continue;
            }
            assert!(
                is_udf(&decode_32(opcode | 1 << bit)),
                "{opcode:#010x} with bit {bit}"
            );
        }
    }
    assert!(is_udf(&decode_32(0xf810_1040)));
}

#[test]
fn test_ldrd_literal_is_ldrd_with_pc_as_the_base() {
    use crate::core::instruction::Reg2DoubleParams;
    let ldrd = |imm32, add| Instruction::LDRD_imm {
        params: Reg2DoubleParams {
            rn: Reg::PC,
            rt: Reg::R1,
            rt2: Reg::R2,
            imm32,
            index: true,
            add,
            wback: false,
        },
    };
    assert_eq!(decode_32(0xe9df_1202), ldrd(8, true)); // ldrd r1, r2, [pc, #8]
    assert_eq!(decode_32(0xe95f_1202), ldrd(8, false)); // ldrd r1, r2, [pc, #-8]
    // write back and the post-indexed form are not there, and neither are SP, PC or one register twice
    assert!(is_udf(&decode_32(0xe9ff_1202)));
    assert!(is_udf(&decode_32(0xe8df_1202)));
    assert!(is_udf(&decode_32(0xe9df_d202)));
    assert!(is_udf(&decode_32(0xe9df_1f02)));
    assert!(is_udf(&decode_32(0xe9df_1102)));
}

// Patterns that overlap with as many dots as each other are resolved by name in build.rs
// (RESOLVED_OVERLAPS), and do not depend on the order of a hash map.
#[test]
fn test_resolved_overlaps_decode_as_resolved() {
    // VSTM with P = 0, U = 0, W = 0 is not VMOV (bit 4 clear), so it is MCRR, which is undefined
    assert!(is_udf(&decode_32(0xec40_0a02)));
    // add sp, sp, sp is the T1 encoding
    assert!(!is_udf(&decode_16(0x44ed)));
}

// VLDM and VSTM with P = U (and no write back): the space of other instructions or undefined.
#[cfg(feature = "has-fp")]
#[test]
fn test_multiple_register_transfers_with_p_equal_to_u_are_undefined() {
    for opcode in [
        0xec00_0a01, // VSTM_T2, decrement before without write back
        0xec10_0a01, // VLDM_T2
        0xec00_0b02, // VSTM_T1
        0xec10_0b02, // VLDM_T1
        0xec51_0a01, // in the MRRC space
    ] {
        assert!(is_udf(&decode_32(opcode)), "{opcode:#010x}");
    }
    // vstmia r0, {s0, s1} (P = 0, U = 1) and vldmdb r0!, {s0, s1} (P = 1, U = 0, W = 1) are fine
    assert!(!is_udf(&decode_32(0xec80_0a02)));
    assert!(!is_udf(&decode_32(0xed30_0a02)));
}

// What the manual makes UNPREDICTABLE or UNDEFINED about the base register and the exclusives.
#[test]
fn test_base_register_rules_decode_as_udf() {
    for opcode in [
        0xe841_0000, // strex r0, r0, [r1]: Rd = Rt
        0xe841_0100, // strex r1, r0, [r1]: Rd = Rn
        0xe8c1_0f40, // strexb r0, r0, [r1]
        0xe8c1_0f41, // strexb r1, r0, [r1]
        0xe85f_0f00, // ldrex r0, [pc]
        0xe84f_1000, // strex r0, r1, [pc]
        0xf8cf_0004, // str.w r0, [pc, #4]
        0xf88f_0004, // strb.w
        0xf8af_0004, // strh.w
        0xf84f_0c04, // str r0, [pc, #-4]
        0xf84f_0002, // str.w r0, [pc, r2]
        0xe9cf_0102, // strd r0, r1, [pc]
        0xe8f0_0102, // ldrd r0, r1, [r0], #8: write back to a destination
        0xe9e0_0102, // strd r0, r1, [r0, #8]!
        0xe8f0_1002, // ldrd r1, r0, [r0], #8
        // write back to Rt, every size, load and store (the imm8 pre-indexed form)
        0xf850_0f04,
        0xf810_0f04,
        0xf830_0f04,
        0xf910_0f04,
        0xf930_0f04,
        0xf840_0f04,
        0xf800_0f04,
        0xf820_0f04,
    ] {
        assert!(is_udf(&decode_32(opcode)), "{opcode:#010x}");
    }
    for opcode in [
        0xe840_1200, // strex r2, r1, [r0]
        0xe8c0_1f52, // strexh r2, r1, [r0]
        0xf8c0_1004, // str.w r1, [r0, #4]
        0xe8f0_2301, // ldrd r2, r3, [r0], #4
        0xf850_1f04, // ldr r1, [r0, #4]!
        0xf810_1f04,
        0xf830_1f04,
        0xf910_1f04,
        0xf930_1f04,
        0xf840_1f04,
        0xf800_1f04,
        0xf820_1f04,
    ] {
        assert!(!is_udf(&decode_32(opcode)), "{opcode:#010x}");
    }
}

// SP or PC as the offset register Rm of a register form (T2) is UNPREDICTABLE for every load and
// store (A7.7.45 LDR, .48 LDRB, .57 LDRH, .61 LDRSB, .65 LDRSH, .162 STR, .164 STRB, .171 STRH).
#[test]
fn test_offset_register_of_sp_or_pc_decodes_as_udf() {
    for opcode in [
        0xf850_1002, // ldr.w r1, [r0, r2]
        0xf810_1002, // ldrb.w
        0xf830_1002, // ldrh.w
        0xf910_1002, // ldrsb.w
        0xf930_1002, // ldrsh.w
        0xf840_1002, // str.w
        0xf800_1002, // strb.w
        0xf820_1002, // strh.w
        0xf850_1012, // ldr.w r1, [r0, r2, lsl #1]
        0xf840_1032, // str.w r1, [r0, r2, lsl #3]
    ] {
        assert!(!is_udf(&decode_32(opcode)), "{opcode:#010x} should decode");
        for register in [13, 15] {
            let patched = opcode & !0xf | register;
            assert!(
                is_udf(&decode_32(patched)),
                "{patched:#010x} (Rm = {register})"
            );
        }
    }
}

// PUSH T3 (STR Rt, [SP, #-4]!, A7.7.101) and POP T3 (LDR Rt, [SP], #4, A7.7.99) are UNPREDICTABLE
// for Rt = SP, and PUSH also for Rt = PC; POP {PC} is a branch and is allowed.
#[test]
fn test_push_and_pop_of_one_register_forbid_sp() {
    assert!(!is_udf(&decode_32(0xf84d_1d04))); // push {r1}
    assert!(is_udf(&decode_32(0xf84d_dd04))); // push {sp}
    assert!(is_udf(&decode_32(0xf84d_fd04))); // push {pc}
    assert!(!is_udf(&decode_32(0xf85d_1b04))); // pop {r1}
    assert!(is_udf(&decode_32(0xf85d_db04))); // pop {sp}
    assert!(!is_udf(&decode_32(0xf85d_fb04))); // pop {pc}
}

// TBB and TBH: Rn = SP, and Rm = SP or PC, are UNPREDICTABLE (A7.7.185). Rn = PC is the usual form.
#[test]
fn test_table_branch_forbids_sp_as_base_and_sp_or_pc_as_index() {
    for opcode in [0xe8df_f001, 0xe8d0_f001, 0xe8df_f011, 0xe8d0_f011] {
        assert!(!is_udf(&decode_32(opcode)), "{opcode:#010x}");
    }
    for opcode in [
        0xe8dd_f001, // tbb [sp, r1]
        0xe8dd_f011, // tbh [sp, r1, lsl #1]
        0xe8d0_f00d, // tbb [r0, sp]
        0xe8d0_f00f, // tbb [r0, pc]
        0xe8d0_f01d, // tbh [r0, sp, lsl #1]
        0xe8d0_f01f, // tbh [r0, pc, lsl #1]
    ] {
        assert!(is_udf(&decode_32(opcode)), "{opcode:#010x}");
    }
}

// The multiple-register forms (A7.7.41 LDM, .42 LDMDB, .99 POP, .101 PUSH, .159 STM, .160 STMDB):
// the 32-bit forms need at least two registers (one register is the single-register LDR/STR),
// Rn = PC is UNPREDICTABLE, LDM and POP may not list PC and LR together, and the 32-bit LDM, STM
// and STMDB may not write back to a base that is in the list. The 16-bit forms need one register.
#[test]
fn test_register_lists_follow_the_manual() {
    for opcode in [
        0xe890_0006, // ldm.w r0, {r1, r2}
        0xe8b0_0006, // ldm.w r0!, {r1, r2}
        0xe890_8002, // ldm.w r0, {r1, pc}
        0xe890_4002, // ldm.w r0, {r1, lr}
        0xe880_0006, // stm.w r0, {r1, r2}
        0xe8a0_0006, // stm.w r0!, {r1, r2}
        0xe900_0006, // stmdb r0, {r1, r2}
        0xe8bd_8006, // pop.w {r1, r2, pc}
        0xe92d_4006, // push.w {r1, r2, lr}
        0xe8bd_0006, // pop.w {r1, r2}
    ] {
        assert!(!is_udf(&decode_32(opcode)), "{opcode:#010x} should decode");
    }
    for opcode in [
        0xe89f_0006, // ldm.w pc, {r1, r2}
        0xe88f_0006, // stm.w pc, {r1, r2}
        0xe8af_0006, // stm.w pc!, {r1, r2}
        0xe90f_0006, // stmdb pc, {r1, r2}
        0xe890_c002, // ldm.w r0, {r1, lr, pc}
        0xe8bd_c002, // pop.w {r1, lr, pc}
        0xe880_0002, // stm.w r0, {r1}
        0xe900_0002, // stmdb r0, {r1}
        0xe92d_0002, // push.w {r1} (T2)
        0xe8bd_0002, // pop.w {r1} (T2)
        0xe8a0_0003, // stm.w r0!, {r0, r1}
        0xe920_0003, // stmdb r0!, {r0, r1}
    ] {
        assert!(is_udf(&decode_32(opcode)), "{opcode:#010x} should be UDF");
    }
    // the 16-bit forms with an empty list
    for opcode in [0xb400, 0xbc00, 0xc000, 0xc800] {
        assert!(is_udf_16(&decode_16(opcode)), "{opcode:#06x}");
    }
    for opcode in [0xb402, 0xbc02, 0xc002, 0xc802, 0xb500, 0xbd00] {
        assert!(!is_udf_16(&decode_16(opcode)), "{opcode:#06x}");
    }
    // LDM.W (A7.7.41 T2): one register, and write back with the base in the list
    assert!(is_udf(&decode_32(0xe890_0002))); // ldm.w r0, {r1}
    assert!(is_udf(&decode_32(0xe8b0_0003))); // ldm.w r0!, {r0, r1}
    assert!(!is_udf(&decode_32(0xe890_0003))); // ldm.w r0, {r0, r1}
    assert!(!is_udf(&decode_32(0xe8b0_0006))); // ldm.w r0!, {r1, r2}
    // r7 is in the 8-bit register_list of the 16-bit forms (A7.7.159 T1, A7.7.41 T1, A7.7.99 T1,
    // A7.7.101 T1)
    for (opcode, rn) in [(0xc080_u16, Reg::R0), (0xc1f0, Reg::R1)] {
        let Instruction::STM { params, .. } = decode_16(opcode) else {
            panic!("{opcode:#06x} should be STM");
        };
        assert_eq!(params.rn, rn);
        assert!(params.registers.contains(&Reg::R7), "{opcode:#06x}");
    }
    assert!(
        matches!(decode_16(0xc8f0), Instruction::LDM { params, .. } if params.registers.contains(&Reg::R7))
    );
    assert!(
        matches!(decode_16(0xb480), Instruction::PUSH { registers, .. } if registers.contains(&Reg::R7))
    );
    assert!(
        matches!(decode_16(0xbc80), Instruction::POP { registers, .. } if registers.contains(&Reg::R7))
    );
    assert!(
        matches!(decode_16(0xb5ff), Instruction::PUSH { registers, .. } if registers.len() == 9)
    );
    assert!(
        matches!(decode_16(0xbdff), Instruction::POP { registers, .. } if registers.len() == 9)
    );
    // SP and PC cannot be in the list of the 32-bit STM and STMDB (A7.7.159, .160)
    for opcode in [
        0xe880_2002, // stm.w r0, {r1, sp}
        0xe880_8002, // stm.w r0, {r1, pc}
        0xe8a0_2002,
        0xe8a0_8002,
        0xe900_2002, // stmdb r0, {r1, sp}
        0xe900_8002,
        0xe920_2002,
        0xe920_8002,
    ] {
        assert!(is_udf(&decode_32(opcode)), "{opcode:#010x}");
    }
    // a 16-bit STM with its base in the list is allowed
    assert!(!is_udf_16(&decode_16(0xc001)));
}

fn is_udf_16(instruction: &Instruction) -> bool {
    matches!(instruction, Instruction::UDF { thumb32: false, .. })
}
