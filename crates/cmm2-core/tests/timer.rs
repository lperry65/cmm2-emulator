//! The real firmware's TIMER and PAUSE, which depend on TIM16's update interrupt (docs/timers.md):
//! TIMER is (N * 50000 + TIM16_CNT) / 1000, with N counted by the interrupt handler.

mod common;

use cmm2_core::{Console, Cpu, Irqs, cmm2_memory};
use common::*;

fn booted() -> (Cpu, Console) {
    let firmware = firmware();
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();
    boot_to_second_prompt(&mut cpu, &console);
    (cpu, console)
}

fn numbers(answer: &str) -> Vec<f64> {
    answer
        .split_whitespace()
        .map(|word| word.parse().unwrap_or_else(|_| panic!("{answer:?}")))
        .collect()
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn pause_100_takes_about_100_milliseconds_by_timer() {
    let (mut cpu, console) = booted();
    // Measured on the board: 100.013. Without the interrupt N stays 0, TIMER wraps every 50 ms
    // and this printed 0.01.
    let answer = answer_to(&mut cpu, &console, "T=TIMER:PAUSE 100:PRINT TIMER-T");
    let elapsed = numbers(&answer)[0];
    println!("PAUSE 100 -> {elapsed}");
    assert!((95.0..=110.0).contains(&elapsed), "{answer:?}");
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn timer_increases_across_a_pause_longer_than_the_50_ms_wrap() {
    let (mut cpu, console) = booted();
    let answer = answer_to(&mut cpu, &console, "A=TIMER:PAUSE 60:B=TIMER:PRINT B-A");
    let elapsed = numbers(&answer)[0];
    println!("PAUSE 60 -> {elapsed}");
    assert!((55.0..=75.0).contains(&elapsed), "{answer:?}");
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn three_timer_reads_in_one_statement_increase() {
    let (mut cpu, console) = booted();
    // Measured on the board: 4 to 7 microseconds apart. Each triple spans about 13 us of
    // machine time and the run is deterministic, so none of these straddles a wrap of TIM16;
    // this checks the reads increase, and the wrap itself is checked by the PAUSE tests.
    for _ in 0..5 {
        let answer = answer_to(&mut cpu, &console, "PRINT TIMER;\" \";TIMER;\" \";TIMER");
        let t = numbers(&answer);
        assert_eq!(t.len(), 3, "{answer:?}");
        assert!(t[0] < t[1] && t[1] < t[2], "{answer:?}");
    }
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn timer_counts_wraps_that_nothing_looked_at() {
    let (mut cpu, console) = booted();
    // 400,000 empty iterations run for several wraps of TIM16 (the board takes about 180 ms,
    // the emulator longer) with no TIMER read in between. This does not isolate the scheduled
    // wake: the machine ticks every device whenever any wake is due, and the display and USB
    // wakes are frequent, so it passes without it too. Only the unit tests in timer/tests.rs
    // guard the wake.
    let answer = answer_to(
        &mut cpu,
        &console,
        "T=TIMER:FOR I=1 TO 400000:NEXT:PRINT TIMER-T",
    );
    let elapsed = numbers(&answer)[0];
    println!("400000 iterations -> {elapsed}");
    assert!((150.0..=400.0).contains(&elapsed), "{answer:?}");
}
