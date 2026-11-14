use super::*;
use crate::core::instruction::Reg2FullParams;

// Each row is an instruction with a normal destination register, and the bit position of the
// destination field in the opcode. Putting PC (0b1111) in that field is UNPREDICTABLE, and so
// it decodes as undefined. The decoder must not hand the executor a plain register write to PC.
const WRITES_A_REGISTER: &[(&str, u32, usize)] = &[
    ("adc.w r1, r2, r3", 0xeb420103, 8),
    ("adc r1, r2, #1", 0xf1420101, 8),
    ("add.w r1, r2, #1", 0xf1020101, 8),
    ("addw r1, r2, #1", 0xf2020101, 8),
    ("and.w r1, r2, r3", 0xea020103, 8),
    ("and r1, r2, #1", 0xf0020101, 8),
    ("asr.w r1, r2, #3", 0xea4f01e2, 8),
    ("asr.w r1, r2, r3", 0xfa42f103, 8),
    ("bfc r1, #4, #4", 0xf36f1107, 8),
    ("bfi r1, r2, #4, #4", 0xf3621107, 8),
    ("bic.w r1, r2, r3", 0xea220103, 8),
    ("bic r1, r2, #1", 0xf0220101, 8),
    ("clz r1, r2", 0xfab2f182, 8),
    ("eor.w r1, r2, r3", 0xea820103, 8),
    ("eor r1, r2, #1", 0xf0820101, 8),
    ("ldrb r1, [r2, #4]!", 0xf8121f04, 12),
    ("ldrh r1, [r2, #4]!", 0xf8321f04, 12),
    ("ldrsb r1, [r2, #4]!", 0xf9121f04, 12),
    ("ldrsh r1, [r2, #4]!", 0xf9321f04, 12),
    ("ldrd r1, r4, [r2, #8]", 0xe9d21402, 12),
    ("ldrd r4, r1, [r2, #8]", 0xe9d24102, 8),
    ("ldrex r1, [r2]", 0xe8521f00, 12),
    ("ldrexb r1, [r2]", 0xe8d21f4f, 12),
    ("ldrexh r1, [r2]", 0xe8d21f5f, 12),
    ("lsl.w r1, r2, #3", 0xea4f01c2, 8),
    ("lsl.w r1, r2, r3", 0xfa02f103, 8),
    ("lsr.w r1, r2, #3", 0xea4f01d2, 8),
    ("lsr.w r1, r2, r3", 0xfa22f103, 8),
    ("ror.w r1, r2, #3", 0xea4f01f2, 8),
    ("ror.w r1, r2, r3", 0xfa62f103, 8),
    ("mla r1, r2, r3, r4", 0xfb024103, 8),
    ("mls r1, r2, r3, r4", 0xfb024113, 8),
    ("movt r1, #1234", 0xf2c041d2, 8),
    ("mov.w r1, #0x12000", 0xf44f3190, 8),
    ("movw r1, #0x1234", 0xf2412134, 8),
    ("mrs r1, apsr", 0xf3ef8100, 8),
    ("mvn r1, #1", 0xf06f0101, 8),
    ("mvn.w r1, r2", 0xea6f0102, 8),
    ("mul r1, r2, r3", 0xfb02f103, 8),
    ("orn r1, r2, r3", 0xea620103, 8),
    ("orn r1, r2, #1", 0xf0620101, 8),
    ("orr.w r1, r2, r3", 0xea420103, 8),
    ("orr r1, r2, #1", 0xf0420101, 8),
    ("rbit r1, r2", 0xfa92f1a2, 8),
    ("rev.w r1, r2", 0xfa92f182, 8),
    ("rev16.w r1, r2", 0xfa92f192, 8),
    ("revsh.w r1, r2", 0xfa92f1b2, 8),
    ("rrx r1, r2", 0xea4f0132, 8),
    ("rsb r1, r2, #1", 0xf1c20101, 8),
    ("rsb r1, r2, r3", 0xebc20103, 8),
    ("sbc r1, r2, #1", 0xf1620101, 8),
    ("sbc.w r1, r2, r3", 0xeb620103, 8),
    ("sbfx r1, r2, #4, #4", 0xf3421103, 8),
    ("sdiv r1, r2, r3", 0xfb92f1f3, 8),
    ("smlal r4, r1, r2, r3", 0xfbc24103, 8),
    ("smlal r1, r4, r2, r3", 0xfbc21403, 12),
    ("smull r4, r1, r2, r3", 0xfb824103, 8),
    ("smull r1, r4, r2, r3", 0xfb821403, 12),
    ("ssat r1, #8, r2", 0xf3020107, 8),
    ("strex r1, r2, [r3]", 0xe8432100, 8),
    ("strexb r1, r2, [r3]", 0xe8c32f41, 0),
    ("strexh r1, r2, [r3]", 0xe8c32f51, 0),
    ("sub.w r1, r2, #1", 0xf1a20101, 8),
    ("subw r1, r2, #1", 0xf2a20101, 8),
    ("sub.w r1, r2, r3", 0xeba20103, 8),
    ("sxtb.w r1, r2", 0xfa4ff182, 8),
    ("sxth.w r1, r2", 0xfa0ff182, 8),
    ("ubfx r1, r2, #4, #4", 0xf3c21103, 8),
    ("udiv r1, r2, r3", 0xfbb2f1f3, 8),
    ("umlal r4, r1, r2, r3", 0xfbe24103, 8),
    ("umlal r1, r4, r2, r3", 0xfbe21403, 12),
    ("umull r4, r1, r2, r3", 0xfba24103, 8),
    ("umull r1, r4, r2, r3", 0xfba21403, 12),
    ("usat r1, #8, r2", 0xf3820108, 8),
    ("uxtb.w r1, r2", 0xfa5ff182, 8),
    ("uxth.w r1, r2", 0xfa1ff182, 8),
];

// These are there only with the DSP extension, and are undefined without it.
#[cfg(feature = "has-dsp-ext")]
const DSP_WRITES_A_REGISTER: &[(&str, u32, usize)] = &[
    ("pkhbt r1, r2, r3", 0xeac20103, 8),
    ("smlabb r1, r2, r3, r4", 0xfb124103, 8),
    ("smulbb r1, r2, r3", 0xfb12f103, 8),
    ("sxtab r1, r2, r3", 0xfa42f183, 8),
    ("sxtah r1, r2, r3", 0xfa02f183, 8),
    ("uadd8 r1, r2, r3", 0xfa82f143, 8),
    ("sel r1, r2, r3", 0xfaa2f183, 8),
    ("uxtab r1, r2, r3", 0xfa52f183, 8),
    ("uxtah r1, r2, r3", 0xfa12f183, 8),
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
