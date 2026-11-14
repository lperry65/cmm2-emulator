//!
//! Encodings that put PC where the executor writes a plain register
//!
//! ARMv7-M calls it UNPREDICTABLE for most instructions to use PC as the destination,
//! or as the base of a write back. I decode those as UDF, like BFI with `msbit < lsbit`,
//! so `set_r` never gets a write to PC from the guest. The instructions that may have PC
//! as their destination (MOV, ADD, LDR, LDM and POP) move it through a branch instead,
//! so they are left out here.
//!

use crate::core::instruction::{Instruction, Reg2FullParams};
use crate::core::register::Reg;

fn is_pc(register: Reg) -> bool {
    register == Reg::PC
}

/// The 8-bit offset forms of the loads and stores with P = 0 and W = 0 (no index and no write
/// back) are not loads or stores, that space is other instructions: they are UNDEFINED. The
/// valid forms always index or write back, so it is the one test for all of them.
pub(super) fn is_unallocated_addressing(instruction: &Instruction) -> bool {
    match instruction {
        Instruction::LDR_imm { params, .. }
        | Instruction::LDRB_imm { params, .. }
        | Instruction::LDRH_imm { params, .. }
        | Instruction::LDRSB_imm { params, .. }
        | Instruction::LDRSH_imm { params, .. }
        | Instruction::STR_imm { params, .. }
        | Instruction::STRB_imm { params, .. }
        | Instruction::STRH_imm { params, .. } => !params.index && !params.wback,
        Instruction::LDRD_imm { params } | Instruction::STRD_imm { params } => {
            !params.index && !params.wback
        }
        _ => false,
    }
}

/// A load of a halfword that has PC as its destination is not a load: with an offset and no write
/// back, or with a register offset, it is the unallocated memory hint of Table A5-21, and does
/// nothing. (LDRB is PLD and LDRSB is PLI by their own encodings, and the literal forms are
/// decoded as hints where they are decoded.)
pub(super) fn is_memory_hint(instruction: &Instruction) -> bool {
    match instruction {
        Instruction::LDRH_reg { params, .. } | Instruction::LDRSH_reg { params, .. } => {
            is_pc(params.rt)
        }
        Instruction::LDRH_imm { params, .. } | Instruction::LDRSH_imm { params, .. } => {
            is_pc(params.rt) && params.index && !params.wback
        }
        _ => false,
    }
}

fn is_bad(register: Reg) -> bool {
    register == Reg::SP || register == Reg::PC
}

/// A register the manual makes UNPREDICTABLE for the instruction: SP for the byte and halfword
/// loads (PC is a hint, or caught above), SP and PC for STRB, STRH, LDRD, STRD, the exclusive
/// stores and SP for the exclusive loads; PC for STR. LDR and STR may use SP, and LDR may load PC.
pub(super) fn uses_a_register_the_manual_forbids(instruction: &Instruction) -> bool {
    // Write back into a register that is also loaded or stored is UNPREDICTABLE, and the stores
    // with PC as the base are UNDEFINED.
    let clashes = |params: &Reg2FullParams| params.wback && params.rn == params.rt;
    match instruction {
        Instruction::LDRB_imm { params, .. }
        | Instruction::LDRH_imm { params, .. }
        | Instruction::LDRSB_imm { params, .. }
        | Instruction::LDRSH_imm { params, .. } => params.rt == Reg::SP || clashes(params),
        Instruction::LDR_imm { params, .. } => clashes(params),
        Instruction::LDRB_reg { params, .. }
        | Instruction::LDRH_reg { params, .. }
        | Instruction::LDRSB_reg { params, .. }
        | Instruction::LDRSH_reg { params, .. } => params.rt == Reg::SP,
        Instruction::STRB_imm { params, .. } | Instruction::STRH_imm { params, .. } => {
            is_bad(params.rt) || is_pc(params.rn) || clashes(params)
        }
        Instruction::STRB_reg { params, .. } | Instruction::STRH_reg { params, .. } => {
            is_bad(params.rt) || is_pc(params.rn)
        }
        Instruction::STR_imm { params, .. } => {
            is_pc(params.rt) || is_pc(params.rn) || clashes(params)
        }
        Instruction::STR_reg { params, .. } => is_pc(params.rt) || is_pc(params.rn),
        Instruction::LDRD_imm { params } => {
            is_bad(params.rt)
                || is_bad(params.rt2)
                || params.rt == params.rt2
                || (params.wback && (params.rn == params.rt || params.rn == params.rt2))
        }
        Instruction::STRD_imm { params } => {
            is_bad(params.rt)
                || is_bad(params.rt2)
                || is_pc(params.rn)
                || (params.wback && (params.rn == params.rt || params.rn == params.rt2))
        }
        Instruction::LDREX { params } => params.rt == Reg::SP || is_pc(params.rn),
        Instruction::LDREXB { params } | Instruction::LDREXH { params } => {
            params.rt == Reg::SP || is_pc(params.rn)
        }
        Instruction::STREX { params } => {
            is_bad(params.rd)
                || is_bad(params.rt)
                || is_pc(params.rn)
                || params.rd == params.rt
                || params.rd == params.rn
        }
        Instruction::STREXB { params } | Instruction::STREXH { params } => {
            is_bad(params.rd)
                || is_bad(params.rt)
                || is_pc(params.rn)
                || params.rd == params.rt
                || params.rd == params.rn
        }
        _ => false,
    }
}

/// The instruction would write PC through `set_r`, or write the base register back to PC.
pub(super) fn writes_pc_as_plain_register(instruction: &Instruction) -> bool {
    match instruction {
        // rd of the data processing group
        Instruction::ADC_reg { params, .. }
        | Instruction::AND_reg { params, .. }
        | Instruction::BIC_reg { params, .. }
        | Instruction::EOR_reg { params, .. }
        | Instruction::ORN_reg { params, .. }
        | Instruction::ORR_reg { params, .. }
        | Instruction::RSB_reg { params, .. }
        | Instruction::SBC_reg { params, .. }
        | Instruction::SUB_reg { params, .. }
        | Instruction::PKH { params, .. } => is_pc(params.rd),
        Instruction::ADC_imm { params, .. }
        | Instruction::ADD_imm { params, .. }
        | Instruction::RSB_imm { params, .. }
        | Instruction::SBC_imm { params, .. }
        | Instruction::SUB_imm { params, .. } => is_pc(params.rd),
        Instruction::AND_imm { params, .. }
        | Instruction::BIC_imm { params, .. }
        | Instruction::EOR_imm { params, .. }
        | Instruction::ORN_imm { params, .. }
        | Instruction::ORR_imm { params, .. } => is_pc(params.rd),
        Instruction::MOV_imm { params, .. } | Instruction::MVN_imm { params, .. } => {
            is_pc(params.rd)
        }
        Instruction::MVN_reg { params, .. } => is_pc(params.rd),
        Instruction::RRX { params, .. } => is_pc(params.rd),
        Instruction::ADR { params, .. } => is_pc(params.r),
        Instruction::ASR_imm { params, .. }
        | Instruction::LSL_imm { params, .. }
        | Instruction::LSR_imm { params, .. }
        | Instruction::ROR_imm { params, .. } => is_pc(params.rd),
        Instruction::ASR_reg { params, .. }
        | Instruction::LSL_reg { params, .. }
        | Instruction::LSR_reg { params, .. }
        | Instruction::ROR_reg { params, .. }
        | Instruction::MUL { params, .. } => is_pc(params.rd),

        // multiply and divide
        Instruction::MLA { params } | Instruction::MLS { params } => is_pc(params.rd),
        Instruction::SDIV { params }
        | Instruction::UDIV { params }
        | Instruction::UADD8 { params }
        | Instruction::SEL { params } => is_pc(params.rd),
        Instruction::SMUL { params } => is_pc(params.rd),
        Instruction::SMLA { params } => is_pc(params.rd),
        Instruction::SMLAL { params }
        | Instruction::SMULL { params }
        | Instruction::UMLAL { params }
        | Instruction::UMULL { params } => is_pc(params.rdlo) || is_pc(params.rdhi),

        // extend, bit field and miscellaneous
        Instruction::SXTB { params, .. }
        | Instruction::SXTH { params, .. }
        | Instruction::UXTB { params, .. }
        | Instruction::UXTH { params, .. } => is_pc(params.rd),
        Instruction::SXTAB { params }
        | Instruction::SXTAH { params }
        | Instruction::UXTAB { params }
        | Instruction::UXTAH { params } => is_pc(params.rd),
        Instruction::BFC { params } => is_pc(params.rd),
        Instruction::BFI { params } => is_pc(params.rd),
        Instruction::SBFX { params } | Instruction::UBFX { params } => is_pc(params.rd),
        Instruction::CLZ { params }
        | Instruction::RBIT { params }
        | Instruction::REV { params, .. }
        | Instruction::REV16 { params, .. }
        | Instruction::REVSH { params, .. } => is_pc(params.rd),
        Instruction::MOVT { params } => is_pc(params.rd),
        Instruction::SSAT { params } | Instruction::USAT { params } => is_pc(params.rd),
        Instruction::MRS { params } => is_pc(params.rd),

        // loads: LDR may load PC, the others may not. Rt = PC with a plain offset is PLD,
        // decoded as such before it gets here.
        Instruction::LDRB_reg { params, .. }
        | Instruction::LDRH_reg { params, .. }
        | Instruction::LDRSB_reg { params, .. }
        | Instruction::LDRSH_reg { params, .. } => is_pc(params.rt),
        Instruction::LDRB_imm { params, .. }
        | Instruction::LDRH_imm { params, .. }
        | Instruction::LDRSB_imm { params, .. }
        | Instruction::LDRSH_imm { params, .. } => {
            is_pc(params.rt) || (params.wback && is_pc(params.rn))
        }
        Instruction::LDR_imm { params, .. } => params.wback && is_pc(params.rn),
        Instruction::LDRD_imm { params } => {
            is_pc(params.rt) || is_pc(params.rt2) || (params.wback && is_pc(params.rn))
        }
        Instruction::LDREX { params } => is_pc(params.rt),
        Instruction::LDREXB { params } | Instruction::LDREXH { params } => is_pc(params.rt),

        // stores have no destination, but they write the base back
        Instruction::STR_imm { params, .. }
        | Instruction::STRB_imm { params, .. }
        | Instruction::STRH_imm { params, .. } => params.wback && is_pc(params.rn),
        Instruction::STRD_imm { params } => params.wback && is_pc(params.rn),
        Instruction::STREX { params } => is_pc(params.rd),
        Instruction::STREXB { params } | Instruction::STREXH { params } => is_pc(params.rd),
        Instruction::LDM { params, .. }
        | Instruction::STM { params, .. }
        | Instruction::STMDB { params } => params.wback && is_pc(params.rn),

        _ => false,
    }
}
