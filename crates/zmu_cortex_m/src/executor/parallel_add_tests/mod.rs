//! The 36 parallel adds and subtracts (the A7.7 pages from QADD16, A7.7.103, to USUB8, A7.7.217)
//! and SEL (A7.7.128).
//!
//! The opcodes are written out from the encoding diagrams in the manual and checked against what
//! llvm-mc assembles. The expected results come from `model`, which follows the manual's
//! pseudocode for each lane, one arm per instruction shape, and shares nothing with the executor.
//! `model.rs` has the encoder and the model, `examples.rs` the opcodes and the worked examples,
//! `random.rs` the edge and random cases, `decode.rs` the encodings that must not decode, and
//! `sel.rs` SEL.

use super::ExecutorHelper;
use crate::Processor;
use crate::core::bits::Bits;
use crate::core::instruction::Instruction;
use crate::core::register::{Apsr, BaseReg, Reg};
use crate::decoder::decode_32;

mod decode;
mod examples;
mod model;
mod random;
mod sel;

/// Every instruction with the opcode llvm-mc gives for `name r1, r2, r3`.
const OPCODES: [(&str, u32); 36] = [
    ("sadd8", 0xfa82_f103),
    ("sadd16", 0xfa92_f103),
    ("sasx", 0xfaa2_f103),
    ("ssax", 0xfae2_f103),
    ("ssub8", 0xfac2_f103),
    ("ssub16", 0xfad2_f103),
    ("qadd8", 0xfa82_f113),
    ("qadd16", 0xfa92_f113),
    ("qasx", 0xfaa2_f113),
    ("qsax", 0xfae2_f113),
    ("qsub8", 0xfac2_f113),
    ("qsub16", 0xfad2_f113),
    ("shadd8", 0xfa82_f123),
    ("shadd16", 0xfa92_f123),
    ("shasx", 0xfaa2_f123),
    ("shsax", 0xfae2_f123),
    ("shsub8", 0xfac2_f123),
    ("shsub16", 0xfad2_f123),
    ("uadd8", 0xfa82_f143),
    ("uadd16", 0xfa92_f143),
    ("uasx", 0xfaa2_f143),
    ("usax", 0xfae2_f143),
    ("usub8", 0xfac2_f143),
    ("usub16", 0xfad2_f143),
    ("uqadd8", 0xfa82_f153),
    ("uqadd16", 0xfa92_f153),
    ("uqasx", 0xfaa2_f153),
    ("uqsax", 0xfae2_f153),
    ("uqsub8", 0xfac2_f153),
    ("uqsub16", 0xfad2_f153),
    ("uhadd8", 0xfa82_f163),
    ("uhadd16", 0xfa92_f163),
    ("uhasx", 0xfaa2_f163),
    ("uhsax", 0xfae2_f163),
    ("uhsub8", 0xfac2_f163),
    ("uhsub16", 0xfad2_f163),
];

/// Runs the instruction on a core with GE and Q preset, and returns the core.
fn run(opcode: u32, regs: &[(Reg, u32)], ge: u32, q: bool) -> Processor {
    let mut core = Processor::new();
    for &(register, value) in regs {
        core.set_r(register, value);
    }
    core.psr.value.set_bits(16..20, ge);
    core.psr.set_q(q);
    core.execute_internal(&decode_32(opcode))
        .unwrap_or_else(|fault| panic!("{opcode:#010x} faulted with {fault:?}"));
    core
}

fn ge_of(core: &Processor) -> u32 {
    core.psr.value.get_bits(16..20)
}
/// N and C set, Z and V clear, so a flag that moved shows
const FLAGS: u32 = 0xa100_0000;

fn is_udf(instruction: &Instruction) -> bool {
    matches!(instruction, Instruction::UDF { .. })
}
