//! LDMDB, also written LDMEA (A7.7.42).
//!
//! The opcodes are written from the diagram (and checked against llvm-mc). The expected values come
//! from `model`, which is the manual's pseudocode on a plain array of registers.

use super::dsp_test_support::Rng;
use super::{ExecuteSuccess, Executor, ExecutorHelper};
use crate::Processor;
use crate::bus::Bus;
use crate::core::fault::Fault;
use crate::core::instruction::{Instruction, LoadAndStoreMultipleParams};
use crate::core::register::{Apsr, BaseReg, Epsr, Reg};
use crate::decoder::{decode_16, decode_32};
use enum_set::EnumSet;

const RAM: u32 = 0x2000_0000;

/// What the RAM holds at `address`, for any address: a hash, the same in the model
fn word_at(address: u32) -> u32 {
    let x = (address >> 2).wrapping_mul(0x9e37_79b1);
    (x ^ (x >> 15)).wrapping_mul(0x85eb_ca6b)
}

fn core_with_memory() -> Processor {
    let mut core = Processor::new();
    for address in (RAM..RAM + 0x1000).step_by(4) {
        core.write32(address, word_at(address)).unwrap();
    }
    for number in 0..13u8 {
        core.set_r(
            Reg::from(number),
            0x1000_0000 + u32::from(number) * 0x0101_0101,
        );
    }
    core.set_r(Reg::LR, 0x0dd0_0dd0);
    core
}

/// 1110 1001 0 0 W 1 Rn / P M 0 `register_list`: the list is `registers` with bit 13 never set
fn encode(rn: u32, wback: bool, registers: u32) -> u32 {
    (0xe910 | u32::from(wback) << 5 | rn) << 16 | registers
}

fn list(registers: u32) -> EnumSet<Reg> {
    (0..16u8)
        .filter(|number| registers >> number & 1 == 1)
        .map(Reg::from)
        .collect()
}

/// The pseudocode on an array of registers (0 to 12, SP, LR, and the PC as the branch target).
/// Returns the new registers and the branch target, if there is one.
fn model(mut regs: [u32; 16], rn: usize, wback: bool, registers: u32) -> ([u32; 16], Option<u32>) {
    let count = registers.count_ones();
    let base = regs[rn];
    let mut address = base.wrapping_sub(4 * count);
    let mut target = None;
    for (number, slot) in regs.iter_mut().enumerate().take(15) {
        if registers >> number & 1 == 1 {
            *slot = word_at(address);
            address += 4;
        }
    }
    if registers >> 15 & 1 == 1 {
        target = Some(word_at(address));
    }
    if wback && registers >> rn & 1 == 0 {
        regs[rn] = base.wrapping_sub(4 * count);
    }
    (regs, target)
}

fn registers_of(core: &Processor) -> [u32; 16] {
    let mut regs = [0; 16];
    for (number, slot) in regs.iter_mut().enumerate().take(15) {
        *slot = core.get_r(Reg::from(number as u8));
    }
    regs
}

fn check(rn: usize, wback: bool, registers: u32, base: u32) {
    let mut core = core_with_memory();
    core.set_r(Reg::from(rn as u8), base);
    let before = registers_of(&core);
    let outcome = core.execute_internal(&decode_32(encode(rn as u32, wback, registers)));
    let (expected, target) = model(before, rn, wback, registers);
    let context = format!("rn {rn} wback {wback} list {registers:#06x} base {base:#x}");
    let after = registers_of(&core);
    assert_eq!(after[..15], expected[..15], "registers of {context}");
    match (outcome, target) {
        (Ok(ExecuteSuccess::Branched { .. }), Some(target)) => {
            assert_eq!(core.pc, target & !1, "PC of {context}");
            assert_eq!(core.psr.get_t(), target & 1 == 1, "T of {context}");
        }
        (Ok(ExecuteSuccess::Taken { .. }), None) => {}
        (outcome, target) => panic!("{outcome:?} with target {target:?}: {context}"),
    }
}

#[test]
fn test_the_opcodes_are_the_manuals_and_llvms_and_decode_to_the_right_registers() {
    // from llvm-mc; ldmea is the same encoding as ldmdb
    for (opcode, rn, wback, registers) in [
        (0xe912_0003, 2, false, 0x0003),
        (0xe932_4013, 2, true, 0x4013),
        (0xe912_8028, 2, false, 0x8028),
        (0xe93d_8010, 13, true, 0x8010),
        (0xe932_0003, 2, true, 0x0003),
    ] {
        assert_eq!(encode(rn, wback, registers), opcode);
        assert_eq!(
            decode_32(opcode),
            Instruction::LDMDB {
                params: LoadAndStoreMultipleParams {
                    registers: list(registers),
                    rn: Reg::from(rn as u8),
                    wback,
                }
            }
        );
    }
}

#[test]
fn test_every_shape_of_list_and_base_matches_the_model() {
    // r0 to r12 and LR, in the lists of two, and every register on its own with LR, and all of them
    let mut lists: Vec<u32> = vec![
        0x0003,
        0x1fff,
        0x5fff,
        0x4001,
        0x8001,
        0x9fff,
        0x1000 | 0x4000,
    ];
    for number in 0..13 {
        lists.push(1 << number | 0x4000);
        lists.push(1 << number | 0x8000);
        lists.push(1 << number | 1 << ((number + 1) % 13));
    }
    for registers in lists {
        for rn in [0, 5, 12, 13, 14] {
            for wback in [false, true] {
                // write back with the base in the list is UNPREDICTABLE, and decodes as UDF
                if wback && registers >> rn & 1 == 1 {
                    continue;
                }
                // a PC with an odd word, so that the target is a Thumb address
                check(rn, wback, registers, RAM + 0x400);
            }
        }
    }
}

#[test]
fn test_random_lists_and_bases_match_the_model() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut next = move || rng.next();
    for _ in 0..3000 {
        let rn = [0, 1, 2, 7, 8, 12, 13, 14][next() as usize % 8];
        let wback = next() & 1 == 1;
        let mut registers = next() & 0x1fff;
        match next() % 3 {
            0 => registers |= 0x4000,
            1 => registers |= 0x8000,
            _ => {}
        }
        if wback {
            registers &= !(1 << rn);
        }
        if registers.count_ones() < 2 {
            continue;
        }
        // 4 * 15 below and 4 above stay inside the filled 4 KiB
        let base = RAM + 0x40 + (next() % 0x3a0) * 4;
        check(rn, wback, registers, base);
    }
}

#[test]
fn test_the_word_for_pc_is_the_branch_and_bit_0_clear_clears_t() {
    // ldmdb r2, {r3, r5, pc}: the PC word is the highest one, just under r2
    let mut core = core_with_memory();
    core.set_r(Reg::R2, RAM + 0x100);
    core.write32(RAM + 0x100 - 4, 0x0800_1235).unwrap();
    let outcome = core.execute_internal(&decode_32(0xe912_8028)).unwrap();
    assert!(matches!(outcome, ExecuteSuccess::Branched { .. }));
    assert_eq!(core.pc, 0x0800_1234);
    assert!(core.psr.get_t());
    assert_eq!(core.get_r(Reg::R3), word_at(RAM + 0x100 - 12));
    assert_eq!(core.get_r(Reg::R5), word_at(RAM + 0x100 - 8));
    // an even PC word clears T: the INVSTATE fault comes when the next instruction is fetched
    core.write32(RAM + 0x100 - 4, 0x0800_1234).unwrap();
    core.execute_internal(&decode_32(0xe912_8028)).unwrap();
    assert!(!core.psr.get_t());
}

#[test]
fn test_write_back_comes_before_an_exception_return_that_uses_sp() {
    // an SVC makes a frame and enters Handler mode; ldmdb sp!, {r4, pc} then loads r4 and the
    // EXC_RETURN from the two words under SP, and SP must be the frame when the return unstacks it
    let mut core = Processor::new();
    core.set_msp(RAM + 0x200);
    core.set_pc(0x100);
    core.psr.set_t(true);
    core.execute(&decode_16(0xdf00), 2); // svc #0
    let frame = core.get_msp();
    core.set_r(Reg::SP, frame + 8);
    core.write32(frame, 0x1111_1111).unwrap();
    core.write32(frame + 4, 0xffff_fff9).unwrap();
    let outcome = core.execute_internal(&decode_32(0xe93d_8010)).unwrap();
    assert!(matches!(outcome, ExecuteSuccess::Branched { .. }));
    assert_eq!(core.get_r(Reg::R4), 0x1111_1111);
    // the frame was unstacked, and SP is above it; the word that was in r1's slot came back in r1
    assert_eq!(core.get_msp(), frame + 0x20);
    assert_eq!(core.get_r(Reg::R1), 0xffff_fff9);
}

fn cycles(outcome: ExecuteSuccess) -> u32 {
    match outcome {
        ExecuteSuccess::Taken { cycles } | ExecuteSuccess::Branched { cycles } => cycles,
        ExecuteSuccess::NotTaken => panic!("not taken"),
    }
}

// ARM publishes no Cortex-M7 timings. LDMDB counts as POP does for the same list: 1 + N, and
// 4 + N when it loads PC (the M4 TRM gives 1 + N for LDM and P, a refill of 1 to 3 cycles, for a
// PC load). LDM.W counts 1 + N for the PC load too, which differs; that is not changed here.
#[test]
fn test_the_cycles_are_one_plus_n_and_four_plus_n_with_pc_like_pop() {
    let mut core = core_with_memory();
    core.set_r(Reg::R2, RAM + 0x100);
    // ldmdb r2, {r0, r1, r4}
    assert_eq!(
        cycles(core.execute_internal(&decode_32(0xe912_0013)).unwrap()),
        4
    );
    // ldmdb r2, {r3, r5, pc}
    assert_eq!(
        cycles(core.execute_internal(&decode_32(0xe912_8028)).unwrap()),
        7
    );
    // the same lists as POP: pop {r4, r5} and pop {r4, pc}, against ldmdb sp!, {r4, r5} and {r4, pc}
    for (pop, ldmdb) in [(0xbc30, 0xe93d_0030), (0xbd10, 0xe93d_8010)] {
        let mut by_pop = core_with_memory();
        by_pop.set_r(Reg::SP, RAM + 0x100);
        let mut by_ldmdb = core_with_memory();
        by_ldmdb.set_r(Reg::SP, RAM + 0x100);
        assert_eq!(
            cycles(by_ldmdb.execute_internal(&decode_32(ldmdb)).unwrap()),
            cycles(by_pop.execute_internal(&decode_16(pop)).unwrap()),
            "{ldmdb:#010x}"
        );
    }
}

#[test]
fn test_a_failed_it_condition_skips_the_instruction() {
    let mut core = core_with_memory();
    core.set_r(Reg::R2, RAM + 0x100);
    core.psr.set_z_bit(true);
    core.set_itstate(((crate::core::condition::Condition::NE.value() as u8) << 4) | 0b1000);
    let outcome = core.execute_internal(&decode_32(0xe932_0003)).unwrap();
    assert!(matches!(outcome, ExecuteSuccess::NotTaken));
    assert_eq!(core.get_r(Reg::R2), RAM + 0x100);
    assert_eq!(core.get_r(Reg::R0), 0x1000_0000);
}

#[test]
fn test_an_unaligned_base_is_a_usage_fault_and_changes_nothing() {
    for base in [RAM + 0x102, RAM + 0x101, RAM + 0x103] {
        let mut core = core_with_memory();
        core.set_r(Reg::R2, base);
        let before = registers_of(&core);
        // ldmdb r2!, {r0, r1, r4, lr}
        let outcome = core.execute_internal(&decode_32(0xe932_4013));
        assert_eq!(outcome, Err(Fault::Unaligned), "base {base:#x}");
        assert_eq!(registers_of(&core)[..15], before[..15]);
    }
}

#[test]
fn test_a_load_that_faults_leaves_every_register_and_the_base_alone() {
    // 128 KiB of RAM end at 0x20020000: with the base at 0x20020008 and four registers the first
    // two words are there and the third is not, so a fault in the middle must not show
    for (registers, wback) in [(0x4013, true), (0x4013, false), (0x8013, false)] {
        let mut core = core_with_memory();
        core.set_r(Reg::R2, 0x2002_0008);
        let before = registers_of(&core);
        let outcome = core.execute_internal(&decode_32(encode(2, wback, registers)));
        assert!(outcome.is_err(), "{registers:#x}: {outcome:?}");
        assert_eq!(
            registers_of(&core)[..15],
            before[..15],
            "{registers:#x} wback {wback}"
        );
    }
}

fn is_udf(instruction: &Instruction) -> bool {
    matches!(instruction, Instruction::UDF { .. })
}

#[test]
fn test_the_unpredictable_forms_are_undefined() {
    // fewer than two registers
    assert!(is_udf(&decode_32(encode(2, false, 0x0001))));
    assert!(is_udf(&decode_32(encode(2, true, 0x0000))));
    // PC and LR together (P = M = 1)
    assert!(is_udf(&decode_32(encode(2, false, 0xc001))));
    // n = 15
    assert!(is_udf(&decode_32(encode(15, false, 0x0003))));
    // write back with the base in the list
    assert!(is_udf(&decode_32(encode(2, true, 0x0007))));
    assert!(!is_udf(&decode_32(encode(2, false, 0x0007))));
    // the allowed ones
    assert!(!is_udf(&decode_32(encode(13, true, 0x8010))));
    assert!(!is_udf(&decode_32(encode(2, true, 0x4003))));
}
