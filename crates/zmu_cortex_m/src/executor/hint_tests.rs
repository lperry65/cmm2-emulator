//! The hints do nothing to the core: NOP, YIELD, WFE, WFI, SEV, DBG #0 to #15, CSDB and the
//! unallocated encodings of the hint space (A7.7.31, A7.7.32, A5.3.4). Every hw2 of `0xf3af` that
//! decodes to something that runs is executed with every register and the flags set to
//! distinctive values, and nothing may change but the program counter, which the executor moves.

use super::dsp_test_support::{Rng, is_udf};
use super::{Executor, ExecutorHelper};
use crate::Processor;
use crate::core::register::{Apsr, BaseReg, Epsr, Reg};
use crate::decoder::{decode_16, decode_32};

fn state(core: &Processor) -> Vec<u32> {
    let mut all: Vec<u32> = (0..15).map(|n| core.get_r(Reg::from(n as u8))).collect();
    all.push(core.psr.value);
    all.push(core.get_psp());
    all
}

#[test]
fn test_every_hint_leaves_every_register_and_flag_alone() {
    let mut rng = Rng(0x41a7_5eed_0000_0003);
    let mut run = 0;
    for hw2 in 0x8000..=0x80ff_u32 {
        let instruction = decode_32(0xf3af_0000 | hw2);
        assert!(!is_udf(&instruction), "{hw2:#06x}");
        for _ in 0..8 {
            let mut core = Processor::new();
            for number in 0..13 {
                core.set_r(Reg::from(number as u8), rng.next());
            }
            core.psr.value = rng.next() & 0xf80f_0000;
            core.psr.set_t(true);
            let before = state(&core);
            core.execute_internal(&instruction)
                .unwrap_or_else(|fault| panic!("{hw2:#06x} faulted with {fault:?}"));
            assert_eq!(state(&core), before, "{hw2:#06x}");
            run += 1;
        }
    }
    assert_eq!(run, 256 * 8);
}

#[test]
fn test_a_hint_in_an_it_block_that_does_not_pass_is_skipped_like_a_nop() {
    // `it eq` with Z clear, then dbg #3 and csdb: they take their cycle and change nothing
    for opcode in [0xf3af_80f3_u32, 0xf3af_8014] {
        let mut core = Processor::new();
        core.psr.set_t(true);
        core.psr.set_z_bit(false);
        core.set_pc(0x100);
        core.execute(&decode_16(0xbf08), 2);
        let before = state(&core);
        core.execute(&decode_32(opcode), 4);
        let mut after = state(&core);
        // the program counter is the one thing that moves (the IT state moves on too)
        after[15] = before[15];
        assert_eq!(after[..15], before[..15], "{opcode:#010x}");
        assert_eq!(core.get_pc(), 0x106, "{opcode:#010x} is four bytes");
    }
}
