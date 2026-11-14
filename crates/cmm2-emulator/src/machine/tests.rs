use super::*;

use crate::args::RunOptions;
use crate::test_support::scratch;

#[test]
fn the_ds3231_is_fitted_and_shows_the_hosts_time() {
    let dir = scratch("ds3231-fitted");
    let firmware = dir.join("firmware.bin");
    let mut image = vec![0; 64];
    image[..4].copy_from_slice(&0x2000_8000u32.to_le_bytes());
    image[4..8].copy_from_slice(&0x0800_0021u32.to_le_bytes());
    std::fs::write(&firmware, image).unwrap();

    let machine = build_machine(firmware.to_str().unwrap(), &RunOptions::default()).unwrap();

    // The chip's own default clock stands at the year 2000, until the host's is attached.
    let shown = machine.cpu.memory().ds3231().time();
    assert_ne!(shown.year, 2000);
    std::fs::remove_dir_all(&dir).unwrap();
}

// A program that turns the RNG's clocks on, enables it, waits for DRDY and reads the first word
// into r4, then loops. The code is at 0x0800_0040, behind the vector table.
const RNG_PROGRAM: [u16; 24] = [
    0x4806, // ldr r0, =RCC_CR
    0x4907, // ldr r1, =1 << 12 (RC48ON)
    0x6001, // str r1, [r0]
    0x4A07, // ldr r2, =RCC_AHB2ENR
    0x4907, // ldr r1, =1 << 6 (RNGEN)
    0x6011, // str r1, [r2]
    0x4B07, // ldr r3, =RNG_CR
    0x2104, // movs r1, #4 (RNGEN)
    0x6019, // str r1, [r3]
    0x685D, // loop: ldr r5, [r3, #4] (RNG_SR)
    0x2D00, // cmp r5, #0
    0xD0FC, // beq loop
    0x689C, // ldr r4, [r3, #8] (RNG_DR)
    0xE7FE, // b .
    0x4400, 0x5802, // RCC_CR
    0x1000, 0x0000, 0x44DC, 0x5802, // RCC_AHB2ENR
    0x0040, 0x0000, 0x1800, 0x4802, // RNG_CR
];

// The first word the RNG gives the program, with the seed the options ask for.
fn first_rng_word(name: &str, seed: Option<u64>) -> u32 {
    let dir = scratch(name);
    let firmware = dir.join("firmware.bin");
    let mut image = vec![0; 64];
    image[..4].copy_from_slice(&0x2000_8000u32.to_le_bytes());
    image[4..8].copy_from_slice(&0x0800_0041u32.to_le_bytes());
    for half in RNG_PROGRAM {
        image.extend_from_slice(&half.to_le_bytes());
    }
    std::fs::write(&firmware, image).unwrap();
    let options = RunOptions {
        rng_seed: seed,
        ..RunOptions::default()
    };
    let mut machine = build_machine(firmware.to_str().unwrap(), &options).unwrap();
    machine.cpu.run(20_000);
    let word = machine.cpu.reg(4);
    std::fs::remove_dir_all(&dir).unwrap();
    word
}

#[test]
fn the_same_rng_seed_gives_the_same_values_and_another_seed_others() {
    let word = first_rng_word("rng-seed-a", Some(7));
    assert_ne!(word, 0, "the program got a word");
    assert_eq!(first_rng_word("rng-seed-b", Some(7)), word);
    assert_ne!(first_rng_word("rng-seed-c", Some(8)), word);
}

#[test]
fn without_a_seed_the_rng_values_differ_from_run_to_run() {
    let (a, b) = (
        first_rng_word("rng-host-a", None),
        first_rng_word("rng-host-b", None),
    );
    assert_ne!(a, 0);
    assert_ne!(a, b);
}
