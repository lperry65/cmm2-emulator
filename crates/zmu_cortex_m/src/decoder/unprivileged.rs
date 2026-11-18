//! LDRT, LDRBT, LDRHT, LDRSBT, LDRSHT, STRT, STRBT and STRHT (A7.7.67, .49, .58, .62, .66, .173,
//! .165, .172): `1111 1000 0 size 0 L Rn / Rt 1110 imm8` (the signed loads have hw1<8> set). Rn is
//! the base and the offset is imm8, added, with no write back.
//!
//! Rn = PC is the literal form of the load of that width (the manual says SEE it) and is UNDEFINED
//! for a store. Rt = SP or PC is UNPREDICTABLE, which `unpredictable.rs` turns into UDF.

use super::{ldrb, ldrh, ldrsb, ldrsh};
use crate::core::bits::Bits;
use crate::core::instruction::{Instruction, Reg2FullParams, UnprivilegedAccess};
use crate::core::register::Reg;

fn decode(access: UnprivilegedAccess, opcode: u32, literal: fn(u32) -> Instruction) -> Instruction {
    let rn = Reg::from(opcode.get_bits(16..20) as u8);
    if rn == Reg::PC {
        return if access.is_store() {
            super::decode_UDF_t2(opcode)
        } else {
            literal(opcode)
        };
    }
    Instruction::UNPRIVILEGED {
        access,
        params: Reg2FullParams {
            rt: Reg::from(opcode.get_bits(12..16) as u8),
            rn,
            imm32: opcode.get_bits(0..8),
            index: true,
            add: true,
            wback: false,
        },
    }
}

/// The literal form of a store, which there is not
fn no_literal(opcode: u32) -> Instruction {
    super::decode_UDF_t2(opcode)
}

#[allow(non_snake_case)]
pub fn decode_LDRT_t1(opcode: u32) -> Instruction {
    decode(
        UnprivilegedAccess::LDRT,
        opcode,
        super::ldr::decode_LDR_lit_t2,
    )
}

#[allow(non_snake_case)]
pub fn decode_LDRBT_t1(opcode: u32) -> Instruction {
    decode(UnprivilegedAccess::LDRBT, opcode, ldrb::decode_LDRB_lit_t1)
}

#[allow(non_snake_case)]
pub fn decode_LDRHT_t1(opcode: u32) -> Instruction {
    decode(UnprivilegedAccess::LDRHT, opcode, ldrh::decode_LDRH_lit_t1)
}

#[allow(non_snake_case)]
pub fn decode_LDRSBT_t1(opcode: u32) -> Instruction {
    decode(
        UnprivilegedAccess::LDRSBT,
        opcode,
        ldrsb::decode_LDRSB_lit_t1,
    )
}

#[allow(non_snake_case)]
pub fn decode_LDRSHT_t1(opcode: u32) -> Instruction {
    decode(
        UnprivilegedAccess::LDRSHT,
        opcode,
        ldrsh::decode_LDRSH_lit_t1,
    )
}

#[allow(non_snake_case)]
pub fn decode_STRT_t1(opcode: u32) -> Instruction {
    decode(UnprivilegedAccess::STRT, opcode, no_literal)
}

#[allow(non_snake_case)]
pub fn decode_STRBT_t1(opcode: u32) -> Instruction {
    decode(UnprivilegedAccess::STRBT, opcode, no_literal)
}

#[allow(non_snake_case)]
pub fn decode_STRHT_t1(opcode: u32) -> Instruction {
    decode(UnprivilegedAccess::STRHT, opcode, no_literal)
}
