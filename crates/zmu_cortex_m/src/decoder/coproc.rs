//! The coprocessor instructions (A5.3.18, Table A5-30): CDP, MCR, MRC, MCRR, MRRC, LDC and STC,
//! and the `2` form of each (the same encoding with hw1<28> set).
//!
//! The Cortex-M7 has two coprocessors, CP10 and CP11, the floating-point unit, and no other
//! (CPACR has no field for any other that reads back, B3.2.20). An instruction for another
//! coprocessor takes a `UsageFault` with UFSR.NOCP (`Instruction::COPROC`). The FP instructions of
//! CP10 and CP11 are matched by the table before these; one of those two numbers that is not an
//! instruction of the floating-point unit is UNDEFINED when the FPU is enabled and NOCP when it is
//! not, which only the executor can tell (CPACR), so those decode as `COPROC` too (when there is no
//! FPU the two are coprocessors that do not exist, like the rest).
//!
//! The encodings the pages call UNDEFINED or UNPREDICTABLE decode as UDF, as the other
//! instructions do; for a coprocessor that is absent the architecture raises the fault whatever
//! the operands, but UNPREDICTABLE allows UDF here.

use crate::core::bits::Bits;
use crate::core::instruction::{CoprocKind, Instruction};

const SP: u32 = 13;
const PC: u32 = 15;

fn is_sp_or_pc(register: u32) -> bool {
    register == SP || register == PC
}

/// Hw2<15:12> is Rt of MCR, MRC (and the first register of the two-register moves, Rt), and
/// hw1<3:0> is Rn, or Rt2 of the two-register moves
fn rt(opcode: u32) -> u32 {
    opcode.get_bits(12..16)
}

fn rt2_or_rn(opcode: u32) -> u32 {
    opcode.get_bits(16..20)
}

/// LDC and STC with P = 0, U = 0 and W = 0 are not these instructions: with D = 1 they are
/// MCRR and MRRC, which the table matches first, and with D = 0 they are UNDEFINED.
fn is_unallocated_transfer(opcode: u32) -> bool {
    !opcode.get_bit(24) && !opcode.get_bit(23) && !opcode.get_bit(21)
}

fn instruction(kind: CoprocKind, opcode: u32, refused: bool) -> Instruction {
    let coproc = opcode.get_bits(8..12) as u8;
    if refused {
        return super::decode_UDF_t2(opcode);
    }
    Instruction::COPROC { kind, coproc }
}

macro_rules! coprocessor {
    ($name:ident, $kind:ident, $refused:expr) => {
        #[allow(non_snake_case)]
        pub fn $name(opcode: u32) -> Instruction {
            let refused: fn(u32) -> bool = $refused;
            instruction(CoprocKind::$kind, opcode, refused(opcode))
        }
    };
}

// CDP has no register and no rule
fn never(_opcode: u32) -> bool {
    false
}

// MCR: Rt = SP or PC is UNPREDICTABLE. MRC: only SP is (PC is the form that sets the flags).
fn mcr_registers(opcode: u32) -> bool {
    is_sp_or_pc(rt(opcode))
}

fn mrc_registers(opcode: u32) -> bool {
    rt(opcode) == SP
}

// The two-register moves: SP or PC in either register is UNPREDICTABLE, and so is MRRC with the
// two registers the same
fn mcrr_registers(opcode: u32) -> bool {
    is_sp_or_pc(rt(opcode)) || is_sp_or_pc(rt2_or_rn(opcode))
}

fn mrrc_registers(opcode: u32) -> bool {
    mcrr_registers(opcode) || rt(opcode) == rt2_or_rn(opcode)
}

// STC: the unallocated P, U, W of `is_unallocated_transfer`, and Rn = PC, which is UNPREDICTABLE
fn stc_shape(opcode: u32) -> bool {
    is_unallocated_transfer(opcode) || rt2_or_rn(opcode) == PC
}

// LDC (literal) is the form with Rn = PC: P = 0 or W = 1 are UNPREDICTABLE (it is always the
// offset form in Thumb), and P = 0 with W = 0 is the unallocated space again
fn ldc_literal_shape(opcode: u32) -> bool {
    !opcode.get_bit(24) || opcode.get_bit(21)
}

coprocessor!(decode_CDP_t1, CDP, never);
coprocessor!(decode_CDP2_t2, CDP2, never);
coprocessor!(decode_MCR_t1, MCR, mcr_registers);
coprocessor!(decode_MCR2_t2, MCR2, mcr_registers);
coprocessor!(decode_MRC_t1, MRC, mrc_registers);
coprocessor!(decode_MRC2_t2, MRC2, mrc_registers);
coprocessor!(decode_MCRR_t1, MCRR, mcrr_registers);
coprocessor!(decode_MCRR2_t2, MCRR2, mcrr_registers);
coprocessor!(decode_MRRC_t1, MRRC, mrrc_registers);
coprocessor!(decode_MRRC2_t2, MRRC2, mrrc_registers);
coprocessor!(decode_LDC_imm_t1, LDC, is_unallocated_transfer);
coprocessor!(decode_LDC2_imm_t2, LDC2, is_unallocated_transfer);
coprocessor!(decode_STC_t1, STC, stc_shape);
coprocessor!(decode_STC2_t2, STC2, stc_shape);
coprocessor!(decode_LDC_lit_t1, LDC, ldc_literal_shape);
coprocessor!(decode_LDC2_lit_t2, LDC2, ldc_literal_shape);
