//! The local exclusive monitor: LDREX, LDREXB, LDREXH, STREX, STREXB, STREXH and CLREX
//! (A3.4.1, Table A3-2, A7.7.23, B2.3.7), and what clears it on exception entry and return
//! (A3.4.4, B1.5.6, B1.5.8) and on reset (B1.5.5).
//!
//! The opcodes are the manual's, checked against llvm-mc. The random sequences are checked against
//! `Monitor`, the state machine of Figure A3-6 written out on its own: the monitor tags the word
//! of the last load-exclusive (the smallest granule, a = 2), a store-exclusive passes only to a
//! tagged word and always leaves the monitor open, and CLREX opens it.

use super::dsp_test_support::Rng;
use super::{Executor, ExecutorHelper};
use crate::Processor;
use crate::ProcessorMode;
use crate::bus::Bus;
use crate::core::register::{Apsr, BaseReg, Epsr, Reg};
use crate::core::reset::Reset;
use crate::decoder::{decode_16, decode_32};

const RAM: u32 = 0x2000_0000;
const EXC_RETURN: u32 = 0xffff_fff9;

// llvm-mc: ldrex r1, [r2] / strex r3, r1, [r2] / ldrexb r1, [r2] / strexb r3, r1, [r2] /
// ldrexh r1, [r2] / strexh r3, r1, [r2] / clrex / str r1, [r2]
const LDREX: u32 = 0xe852_1f00;
const STREX: u32 = 0xe842_1300;
const LDREXB: u32 = 0xe8d2_1f4f;
const STREXB: u32 = 0xe8c2_1f43;
const LDREXH: u32 = 0xe8d2_1f5f;
const STREXH: u32 = 0xe8c2_1f53;
const CLREX: u32 = 0xf3bf_8f2f;
const STR: u32 = 0xf8c2_1000;

fn core() -> Processor {
    let mut core = Processor::new();
    core.set_msp(RAM + 0x800);
    core.set_r(Reg::R1, 0xdead_beef);
    core.set_r(Reg::R2, RAM + 0x100);
    core.write32(RAM + 0x100, 0x1111_1111).unwrap();
    core
}

fn run(core: &mut Processor, opcode: u32) {
    core.execute_internal(&decode_32(opcode))
        .unwrap_or_else(|fault| panic!("{opcode:#010x} faulted with {fault:?}"));
}

/// Runs a store-exclusive of r1 to [r2] and says whether it stored (status 0) or not (status 1)
fn stored(core: &mut Processor, opcode: u32) -> bool {
    core.set_r(Reg::R3, 0x5555_5555);
    run(core, opcode);
    match core.get_r(Reg::R3) {
        0 => true,
        1 => false,
        other => panic!("status {other:#x}"),
    }
}

#[test]
fn test_the_opcodes_are_the_manuals_and_decode_to_the_right_names() {
    for (opcode, name) in [
        (LDREX, "ldrex"),
        (STREX, "strex"),
        (LDREXB, "ldrexb"),
        (STREXB, "strexb"),
        (LDREXH, "ldrexh"),
        (STREXH, "strexh"),
        (CLREX, "clrex"),
    ] {
        let text = decode_32(opcode).to_string();
        assert_eq!(text.split(' ').next(), Some(name), "{opcode:#010x}: {text}");
    }
}

#[test]
fn test_a_store_exclusive_with_no_load_exclusive_fails() {
    for store in [STREX, STREXB, STREXH] {
        let mut core = core();
        assert!(!stored(&mut core, store), "{store:#010x}");
        assert_eq!(
            core.read32(RAM + 0x100).unwrap(),
            0x1111_1111,
            "memory of {store:#010x}"
        );
    }
}

#[test]
fn test_a_load_exclusive_then_a_store_exclusive_to_the_same_address_succeeds_and_clears() {
    for (load, store, mask) in [
        (LDREX, STREX, 0xffff_ffff_u32),
        (LDREXB, STREXB, 0xff),
        (LDREXH, STREXH, 0xffff),
    ] {
        let mut core = core();
        run(&mut core, load);
        assert_eq!(
            core.get_r(Reg::R1),
            0x1111_1111 & mask,
            "loaded by {load:#010x}"
        );
        core.set_r(Reg::R1, 0xdead_beef);
        assert!(stored(&mut core, store), "{store:#010x}");
        let expected = (0x1111_1111 & !mask) | (0xdead_beef & mask);
        assert_eq!(
            core.read32(RAM + 0x100).unwrap(),
            expected,
            "memory of {store:#010x}"
        );
        // the success cleared the monitor: a second store-exclusive fails and writes nothing
        core.set_r(Reg::R1, 0x2222_2222);
        assert!(!stored(&mut core, store), "second {store:#010x}");
        assert_eq!(
            core.read32(RAM + 0x100).unwrap(),
            expected,
            "memory after the second"
        );
    }
}

#[test]
fn test_clrex_makes_the_store_exclusive_fail() {
    for (load, store) in [(LDREX, STREX), (LDREXB, STREXB), (LDREXH, STREXH)] {
        let mut core = core();
        run(&mut core, load);
        run(&mut core, CLREX);
        assert!(!stored(&mut core, store), "{store:#010x}");
        assert_eq!(core.read32(RAM + 0x100).unwrap(), 0x1111_1111);
    }
}

#[test]
fn test_clrex_in_an_it_block_that_does_not_pass_leaves_the_monitor_alone() {
    let mut core = core();
    run(&mut core, LDREX);
    // `it eq` with Z clear: the CLREX is skipped
    core.psr.set_z_bit(false);
    core.execute(&decode_16(0xbf08), 2);
    core.execute(&decode_32(CLREX), 4);
    assert!(stored(&mut core, STREX));
}

#[test]
fn test_a_store_exclusive_to_another_address_fails_and_closes_the_monitor() {
    // A3.4.1: it is IMPLEMENTATION DEFINED whether such a store takes place. Here it does not, and
    // in all cases the monitor ends open, so the store to the tagged address after it fails too.
    let mut core = core();
    run(&mut core, LDREX);
    core.set_r(Reg::R2, RAM + 0x200);
    let elsewhere = core.read32(RAM + 0x200).unwrap();
    assert!(!stored(&mut core, STREX));
    assert_eq!(core.read32(RAM + 0x200).unwrap(), elsewhere);
    core.set_r(Reg::R2, RAM + 0x100);
    assert!(
        !stored(&mut core, STREX),
        "the monitor was closed by the first"
    );
}

#[test]
fn test_a_load_exclusive_moves_the_tag_to_the_new_address() {
    let mut core = core();
    run(&mut core, LDREX);
    core.set_r(Reg::R2, RAM + 0x200);
    run(&mut core, LDREX);
    core.set_r(Reg::R1, 0xdead_beef);
    assert!(stored(&mut core, STREX));
    assert_eq!(core.read32(RAM + 0x200).unwrap(), 0xdead_beef);
}

#[test]
fn test_the_tag_is_the_word_so_another_byte_of_the_word_passes() {
    // The granule is IMPLEMENTATION DEFINED, a word at least (A3.4.3): here it is one word, so a
    // STREXB to another byte of the word that LDREXB tagged is in the tagged block and passes.
    let mut core = core();
    run(&mut core, LDREXB);
    core.set_r(Reg::R2, RAM + 0x101);
    assert!(stored(&mut core, STREXB));
    // and the next word is another block
    run(&mut core, LDREXB);
    core.set_r(Reg::R2, RAM + 0x104);
    assert!(!stored(&mut core, STREXB));
}

#[test]
fn test_an_ordinary_store_to_the_tagged_address_leaves_the_monitor_as_it_was() {
    // Table A3-2: Store(!t) has no effect on the monitor, and for Store(t) it is IMPLEMENTATION
    // DEFINED whether the monitor closes. Here it does not.
    let mut core = core();
    run(&mut core, LDREX);
    core.set_r(Reg::R1, 0xdead_beef);
    run(&mut core, STR);
    assert_eq!(core.read32(RAM + 0x100).unwrap(), 0xdead_beef);
    core.set_r(Reg::R1, 0x3333_3333);
    assert!(stored(&mut core, STREX));
}

#[test]
fn test_a_store_exclusive_that_does_not_pass_its_condition_leaves_the_monitor_alone() {
    let mut core = core();
    run(&mut core, LDREX);
    // `it ne` with Z set: the STREX is skipped, and the monitor is still set for the next one
    core.psr.set_z_bit(true);
    core.execute(&decode_16(0xbf18), 2);
    core.set_r(Reg::R3, 0x5555_5555);
    core.execute(&decode_32(STREX), 4);
    assert_eq!(
        core.get_r(Reg::R3),
        0x5555_5555,
        "the skipped STREX wrote its status"
    );
    assert!(stored(&mut core, STREX));
}

/// An SVC taken from Thread mode, which leaves the core in the handler, with LR = `EXC_RETURN`
fn take_svc(core: &mut Processor) {
    core.set_pc(0x100);
    core.psr.set_t(true);
    core.execute(&decode_16(0xdf00), 2);
    assert_eq!(core.mode, ProcessorMode::HandlerMode);
}

fn return_from_handler(core: &mut Processor) {
    core.set_r(Reg::LR, EXC_RETURN);
    core.execute(&decode_16(0x4770), 2); // bx lr
    assert_eq!(core.mode, ProcessorMode::ThreadMode);
}

#[test]
fn test_a_store_exclusive_after_an_exception_entry_fails() {
    // B1.5.6: ExceptionTaken() clears the monitor, so the thread's LDREX is lost to the handler
    let mut core = core();
    run(&mut core, LDREX);
    take_svc(&mut core);
    return_from_handler(&mut core);
    assert!(!stored(&mut core, STREX));
    assert_eq!(core.read32(RAM + 0x100).unwrap(), 0x1111_1111);
}

#[test]
fn test_a_store_exclusive_in_the_handler_after_the_entry_fails() {
    // the entry alone, before any return can clear it
    let mut core = core();
    run(&mut core, LDREX);
    take_svc(&mut core);
    assert!(!stored(&mut core, STREX));
}

#[test]
fn test_a_store_exclusive_after_an_exception_return_fails() {
    // B1.5.8: ExceptionReturn() clears it too: a load-exclusive made inside the handler is gone
    // when the thread is back
    let mut core = core();
    take_svc(&mut core);
    run(&mut core, LDREX);
    return_from_handler(&mut core);
    assert!(!stored(&mut core, STREX));
}

#[test]
fn test_a_load_exclusive_after_the_return_works_again() {
    let mut core = core();
    take_svc(&mut core);
    return_from_handler(&mut core);
    run(&mut core, LDREX);
    assert!(stored(&mut core, STREX));
}

#[test]
fn test_a_fault_entry_clears_the_monitor_too() {
    // a UsageFault from an undefined instruction goes through the same ExceptionTaken()
    let mut core = core();
    run(&mut core, LDREX);
    core.set_pc(0x100);
    core.psr.set_t(true);
    core.execute(&decode_32(0xf7f0_a000), 4); // udf.w
    assert_eq!(core.mode, ProcessorMode::HandlerMode);
    return_from_handler(&mut core);
    assert!(!stored(&mut core, STREX));
}

#[test]
fn test_reset_clears_the_monitor() {
    // B1.5.5: TakeReset() calls ClearExclusiveLocal()
    let mut core = core();
    run(&mut core, LDREX);
    let _ = core.reset();
    core.set_r(Reg::R2, RAM + 0x100);
    core.set_r(Reg::R1, 0xdead_beef);
    assert!(!stored(&mut core, STREX));
}

/// Figure A3-6 and Table A3-2 as a state machine over the tagged word
struct Monitor(Option<u32>);

impl Monitor {
    fn tag(address: u32) -> u32 {
        address >> 2
    }

    /// whether a store-exclusive to `address` stores; it always leaves the monitor open
    fn store_exclusive(&mut self, address: u32) -> bool {
        let passes = self.0 == Some(Self::tag(address));
        self.0 = None;
        passes
    }
}

#[test]
fn test_random_sequences_follow_the_state_machine() {
    // load-exclusives, store-exclusives, CLREX and plain stores, on addresses from a small set,
    // with the width of the access random too: 5,000 sequences of 12 operations
    let mut rng = Rng(0x0ecc_1e55_f00d_0001);
    let addresses = [
        RAM + 0x100,
        RAM + 0x101,
        RAM + 0x104,
        RAM + 0x108,
        RAM + 0x10a,
    ];
    let mut checked = 0;
    for _ in 0..5000 {
        let mut core = core();
        let mut monitor = Monitor(None);
        for step in 0..12 {
            let address = addresses[rng.next() as usize % addresses.len()];
            core.set_r(Reg::R2, address);
            // the width: word accesses need a word-aligned address and halfword ones an even one
            let (load, store, size) = match rng.next() % 3 {
                0 if address.is_multiple_of(4) => (LDREX, STREX, 4),
                1 if address.is_multiple_of(2) => (LDREXH, STREXH, 2),
                _ => (LDREXB, STREXB, 1),
            };
            match rng.next() % 5 {
                0 | 1 => {
                    run(&mut core, load);
                    monitor.0 = Some(Monitor::tag(address));
                }
                2 | 3 => {
                    let expected = monitor.store_exclusive(address);
                    let before = core.read32(address & !3).unwrap();
                    core.set_r(Reg::R1, 0xa5a5_a5a5);
                    let actual = stored(&mut core, store);
                    assert_eq!(actual, expected, "step {step}, size {size} at {address:#x}");
                    if !actual {
                        assert_eq!(core.read32(address & !3).unwrap(), before, "memory");
                    }
                    checked += 1;
                }
                _ => {
                    if rng.next() & 1 == 0 {
                        run(&mut core, CLREX);
                        monitor.0 = None;
                    } else {
                        core.set_r(Reg::R2, RAM + 0x180);
                        run(&mut core, STR);
                    }
                }
            }
        }
    }
    assert!(checked > 10_000, "{checked}");
}
