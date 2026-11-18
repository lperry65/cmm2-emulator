//! LDRT, LDRBT, LDRHT, LDRSBT, LDRSHT, STRT, STRBT and STRHT (A7.7.67, .49, .58, .62, .66, .173,
//! .165, .172).
//!
//! The opcodes are written from the diagrams (`1111 1000 0 size 0 L Rn / Rt 1110 imm8`, with the
//! sign bit at hw1<8> for the loads) and checked against llvm-mc. The expected values come from
//! `model`, the pseudocode on an array of bytes: address = R[n] + imm8, no write back, a store
//! writes the low byte, halfword or word of R[t] and a load extends what it reads. The privilege
//! only matters to the MPU, which this core does not have (`peripheral::mpu` accepts every
//! address), so the access is the ordinary one; the tests run in Thread mode and in Handler mode
//! (privileged) and want the same.

use super::dsp_test_support::{Rng, is_udf};
use super::{Executor, ExecutorHelper};
use crate::Processor;
use crate::ProcessorMode;
use crate::bus::Bus;
use crate::core::register::{Apsr, BaseReg, Reg};
use crate::decoder::{decode_16, decode_32};

const RAM: u32 = 0x2000_0000;
const SIZE: usize = 0x400;

#[derive(Clone, Copy, Debug)]
struct Kind {
    name: &'static str,
    /// hw1 with Rn = 0
    hw1: u32,
    store: bool,
    bytes: u32,
    signed: bool,
}

const KINDS: [Kind; 8] = [
    Kind {
        name: "ldrt",
        hw1: 0xf850,
        store: false,
        bytes: 4,
        signed: false,
    },
    Kind {
        name: "ldrbt",
        hw1: 0xf810,
        store: false,
        bytes: 1,
        signed: false,
    },
    Kind {
        name: "ldrht",
        hw1: 0xf830,
        store: false,
        bytes: 2,
        signed: false,
    },
    Kind {
        name: "ldrsbt",
        hw1: 0xf910,
        store: false,
        bytes: 1,
        signed: true,
    },
    Kind {
        name: "ldrsht",
        hw1: 0xf930,
        store: false,
        bytes: 2,
        signed: true,
    },
    Kind {
        name: "strt",
        hw1: 0xf840,
        store: true,
        bytes: 4,
        signed: false,
    },
    Kind {
        name: "strbt",
        hw1: 0xf800,
        store: true,
        bytes: 1,
        signed: false,
    },
    Kind {
        name: "strht",
        hw1: 0xf820,
        store: true,
        bytes: 2,
        signed: false,
    },
];

fn encode(kind: Kind, rt: u32, rn: u32, imm8: u32) -> u32 {
    (kind.hw1 | rn) << 16 | rt << 12 | 0xe00 | imm8
}

#[test]
fn test_the_opcodes_are_llvms() {
    // llvm-mc: <name> r1, [r2, #4] for each, then ldrt r1, [sp, #255] and strt r12, [sp]
    let want = [
        0xf852_1e04_u32,
        0xf812_1e04,
        0xf832_1e04,
        0xf912_1e04,
        0xf932_1e04,
        0xf842_1e04,
        0xf802_1e04,
        0xf822_1e04,
    ];
    for (kind, want) in KINDS.iter().zip(want) {
        assert_eq!(encode(*kind, 1, 2, 4), want, "{}", kind.name);
    }
    assert_eq!(encode(KINDS[0], 1, 13, 255), 0xf85d_1eff);
    assert_eq!(encode(KINDS[5], 12, 13, 0), 0xf84d_ce00);
}

#[test]
fn test_each_decodes_to_its_own_name_and_not_to_an_ordinary_store_or_load() {
    for kind in KINDS {
        let text = decode_32(encode(kind, 1, 2, 4)).to_string();
        assert!(text.starts_with(kind.name), "{}: {text}", kind.name);
    }
}

fn fresh(rng: &mut Rng) -> (Processor, [u8; SIZE]) {
    let mut core = Processor::new();
    let mut bytes = [0u8; SIZE];
    for (offset, byte) in bytes.iter_mut().enumerate() {
        *byte = rng.next() as u8;
        core.write8(RAM + offset as u32, *byte).unwrap();
    }
    for number in 0..13u8 {
        core.set_r(Reg::from(number), rng.next());
    }
    (core, bytes)
}

fn memory(core: &Processor) -> [u8; SIZE] {
    let mut bytes = [0u8; SIZE];
    for (offset, byte) in bytes.iter_mut().enumerate() {
        *byte = core.read8(RAM + offset as u32).unwrap();
    }
    bytes
}

/// The pseudocode, on the registers and the bytes: returns the new value of R[rt] for a load
fn model(
    kind: Kind,
    regs: &[u32; 13],
    bytes: &mut [u8; SIZE],
    rt: usize,
    address: u32,
) -> Option<u32> {
    let at = (address - RAM) as usize;
    if kind.store {
        let value = regs[rt];
        for index in 0..kind.bytes as usize {
            bytes[at + index] = (value >> (8 * index)) as u8;
        }
        return None;
    }
    let mut value = 0u32;
    for index in 0..kind.bytes as usize {
        value |= u32::from(bytes[at + index]) << (8 * index);
    }
    if kind.signed {
        let shift = 32 - 8 * kind.bytes;
        value = (((value << shift) as i32) >> shift) as u32;
    }
    Some(value)
}

fn check_all(handler_mode: bool, seed: u64) -> usize {
    let mut rng = Rng(seed);
    let mut cases = 0;
    for kind in KINDS {
        for _ in 0..1500 {
            let (mut core, mut bytes) = fresh(&mut rng);
            if handler_mode {
                core.mode = ProcessorMode::HandlerMode;
            }
            // registers 0 to 12 for Rt and Rn; the stack pointer for Rn some of the time
            let rt = rng.next() as usize % 13;
            let use_sp = rng.next().is_multiple_of(8);
            let rn = if use_sp { 13 } else { rng.next() as usize % 13 };
            // the offsets: 0 and 255 often, the rest random; the base so that every address of
            // the access, odd ones included, stays inside the bytes held
            let imm8 = match rng.next() % 4 {
                0 => 0,
                1 => 255,
                _ => rng.next() & 0xff,
            };
            let base = RAM + (rng.next() % (SIZE as u32 - 256 - 4));
            if use_sp {
                core.set_msp(base & !3);
            } else {
                core.set_r(Reg::from(rn as u8), base);
            }
            let before: [u32; 13] = std::array::from_fn(|n| core.get_r(Reg::from(n as u8)));
            let base_value = core.get_r(Reg::from(rn as u8));
            let address = base_value.wrapping_add(imm8);
            let want_load = model(kind, &before, &mut bytes, rt, address);
            core.execute_internal(&decode_32(encode(kind, rt as u32, rn as u32, imm8)))
                .unwrap_or_else(|fault| panic!("{} faulted with {fault:?}", kind.name));
            let context = format!("{} r{rt}, [r{rn}, #{imm8}] base {base_value:#x}", kind.name);
            // R[t] gets the loaded value and nothing else changes, the base included (no write
            // back)
            for (number, &prior) in before.iter().enumerate() {
                let expected = match want_load {
                    Some(value) if number == rt => value,
                    _ => prior,
                };
                assert_eq!(
                    core.get_r(Reg::from(number as u8)),
                    expected,
                    "r{number} of {context}"
                );
            }
            if use_sp {
                assert_eq!(core.get_r(Reg::SP), base_value, "sp of {context}");
            }
            assert_eq!(memory(&core), bytes, "memory of {context}");
            cases += 1;
        }
    }
    cases
}

#[test]
fn test_random_accesses_in_thread_mode_match_the_model() {
    assert_eq!(check_all(false, 0x7e57_ed01_0000_0001), 8 * 1500);
}

#[test]
fn test_random_accesses_in_handler_mode_match_the_model() {
    // Handler mode is privileged: the access is made as if unprivileged, which only the MPU can see
    assert_eq!(check_all(true, 0x7e57_ed01_0000_0002), 8 * 1500);
}

#[test]
fn test_the_offset_is_eight_bits_added_and_the_base_register_is_not_written_back() {
    let mut core = Processor::new();
    core.write32(RAM + 0x1ff, 0x8899_aabb).unwrap();
    core.write32(RAM + 0x100, 0x1122_3344).unwrap();
    core.set_r(Reg::R2, RAM + 0x100);
    // ldrt r1, [r2, #255] reads RAM + 0x1ff (unaligned, allowed unless CCR.UNALIGN_TRP)
    core.execute_internal(&decode_32(encode(KINDS[0], 1, 2, 255)))
        .unwrap();
    assert_eq!(core.get_r(Reg::R1), 0x8899_aabb);
    assert_eq!(core.get_r(Reg::R2), RAM + 0x100);
}

#[test]
fn test_rt_equal_to_rn_loads_into_the_base_and_stores_the_base() {
    let mut core = Processor::new();
    core.write32(RAM + 0x104, 0x600d_f00d).unwrap();
    core.set_r(Reg::R2, RAM + 0x100);
    core.execute_internal(&decode_32(encode(KINDS[0], 2, 2, 4)))
        .unwrap();
    assert_eq!(core.get_r(Reg::R2), 0x600d_f00d);
    core.set_r(Reg::R2, RAM + 0x100);
    core.execute_internal(&decode_32(encode(KINDS[5], 2, 2, 8)))
        .unwrap();
    assert_eq!(core.read32(RAM + 0x108).unwrap(), RAM + 0x100);
}

#[test]
fn test_rt_sp_or_pc_is_unpredictable_and_decodes_as_udf() {
    for kind in KINDS {
        for rt in [13, 15] {
            assert!(
                is_udf(&decode_32(encode(kind, rt, 2, 4))),
                "{} rt {rt}",
                kind.name
            );
        }
        assert!(
            !is_udf(&decode_32(encode(kind, 14, 2, 4))),
            "{} rt lr",
            kind.name
        );
    }
}

#[test]
fn test_the_stores_with_rn_pc_are_undefined_and_the_loads_are_the_literal_forms() {
    for kind in KINDS {
        let instruction = decode_32(encode(kind, 1, 15, 4));
        if kind.store {
            assert!(is_udf(&instruction), "{}", kind.name);
        } else {
            // the literal form of the ordinary load of that width, never UDF (A7.7.67 and the
            // others say SEE the literal form)
            assert!(!is_udf(&instruction), "{}: {instruction:?}", kind.name);
            let text = instruction.to_string();
            assert!(!text.starts_with(kind.name), "{}: {text}", kind.name);
        }
    }
}

#[test]
fn test_an_access_that_faults_faults_as_the_ordinary_one_does() {
    // an address that no memory answers: the same fault as `ldr` and `str` give
    let outcome = |opcode: u32| {
        let mut core = Processor::new();
        core.set_r(Reg::R2, 0xa000_0000);
        core.execute_internal(&decode_32(opcode))
    };
    // ldr.w r1, [r2, #4] / ldrt r1, [r2, #4] and str.w / strt
    let ordinary_load = outcome(0xf8d2_1004);
    assert!(ordinary_load.is_err(), "{ordinary_load:?}");
    assert_eq!(outcome(encode(KINDS[0], 1, 2, 4)), ordinary_load);
    let ordinary_store = outcome(0xf8c2_1004);
    assert!(ordinary_store.is_err(), "{ordinary_store:?}");
    assert_eq!(outcome(encode(KINDS[5], 1, 2, 4)), ordinary_store);
}

#[test]
fn test_unaligned_word_and_halfword_follow_ccr_unalign_trp() {
    for (kind, mask) in [(KINDS[0], 3), (KINDS[2], 1), (KINDS[5], 3), (KINDS[7], 1)] {
        let mut core = Processor::new();
        core.set_r(Reg::R2, RAM + 0x101);
        // CCR.UNALIGN_TRP clear: the unaligned access is made
        core.execute_internal(&decode_32(encode(kind, 1, 2, 0)))
            .unwrap();
        // set: the alignment fault of A3.2.1
        core.ccr |= 1 << 3;
        let outcome = core.execute_internal(&decode_32(encode(kind, 1, 2, 0)));
        assert_eq!(outcome.is_err(), (RAM + 0x101) & mask != 0, "{}", kind.name);
    }
}

#[test]
fn test_in_an_it_block_that_does_not_pass_they_do_nothing() {
    let mut core = Processor::new();
    core.psr.set_z_bit(false);
    core.set_r(Reg::R2, RAM + 0x100);
    core.set_r(Reg::R1, 0x1234_5678);
    core.write32(RAM + 0x104, 0xaaaa_5555).unwrap();
    // `it eq`, with Z clear
    core.execute(&decode_16(0xbf08), 2);
    core.execute(&decode_32(encode(KINDS[5], 1, 2, 4)), 4);
    assert_eq!(core.read32(RAM + 0x104).unwrap(), 0xaaaa_5555);
    core.execute(&decode_16(0xbf08), 2);
    core.execute(&decode_32(encode(KINDS[0], 1, 2, 4)), 4);
    assert_eq!(core.get_r(Reg::R1), 0x1234_5678);
}
