//! The real firmware and the RNG (docs/rng.md). At boot the firmware's init code calls
//! HAL_RNG_Init, which enables the generator and polls for a first value. With no generator the
//! init fails and the firmware queues "Error:1", which the first command through the error path
//! prints, so every user saw it.

mod common;

use std::cell::Cell;
use std::rc::Rc;

use cmm2_core::{Cpu, Irqs, cmm2_memory, cmm2_memory_with_rng_seed};
use common::*;

const RNG_DR: u32 = 0x4802_1808;

fn boot() -> (Cpu, cmm2_core::Console) {
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let mut cpu = Cpu::new(&firmware(), memory, irqs).unwrap();
    boot_to_second_prompt(&mut cpu, &console);
    (cpu, console)
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_firmware_prints_no_error_1_after_boot() {
    let (mut cpu, console) = boot();
    let (output, _) = type_keys(&mut cpu, &console, "NEW\r");
    assert!(!output.contains("Error:1"), "{output:?}");
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_firmware_gives_different_values_for_rnd() {
    let (mut cpu, console) = boot();
    let first = answer_to(&mut cpu, &console, "PRINT RND");
    let second = answer_to(&mut cpu, &console, "PRINT RND");
    assert_ne!(first, second);
}

/// Boots with the RNG seeded with `seed`, and returns what `PRINT RND` printed and how many
/// times the firmware read RNG_DR to make it.
fn first_rnd_with_seed(seed: u64) -> (String, u32) {
    let irqs = Irqs::new();
    let mut memory = cmm2_memory_with_rng_seed(&irqs, seed);
    let reads = Rc::new(Cell::new(0));
    let counter = reads.clone();
    memory.set_peripheral_trace(Some(Box::new(move |access| {
        if access.addr == RNG_DR && !access.write {
            counter.set(counter.get() + 1);
        }
    })));
    let console = memory.console();
    let mut cpu = Cpu::new(&firmware(), memory, irqs).unwrap();
    boot_to_second_prompt(&mut cpu, &console);
    let before = reads.get();
    let value = answer_to(&mut cpu, &console, "PRINT RND");
    (value, reads.get() - before)
}

// MMBasic's RND on this firmware reads the hardware RNG: RNG_DR is read once for each RND (found
// by tracing the reads), and the value follows the RNG's seed.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_firmware_rnd_comes_from_the_hardware_rng() {
    let (a, reads_a) = first_rnd_with_seed(1);
    let (b, reads_b) = first_rnd_with_seed(2);
    assert_eq!((reads_a, reads_b), (1, 1));
    assert_ne!(a, b);
}
