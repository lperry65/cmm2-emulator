//! The real firmware and the board's DS3231.

mod common;

use cmm2_core::{Console, Cpu, Irqs, cmm2_memory};
use common::*;

// The board's DS3231 on PD4 and PD5 (docs/ds3231.md). With `OPTION DS3231 ON` the firmware reads
// the time from it at boot and for every TIME$ and DATE$. These runs inject a host clock that
// stands still, so what the firmware prints is the same every time.

/// Thursday 2026-12-24, with December for the month's tens bit.
fn host_clock() -> cmm2_core::FixedClock {
    cmm2_core::FixedClock::new(cmm2_core::HostTime::new(2026, 12, 24, 13, 45, 56))
}

/// The saved state of a board that has been through its first boot and then
/// `OPTION DS3231 ON`. It takes a billion instructions to make, so the runs share one.
fn state_with_the_ds3231_on() -> &'static [u8] {
    static STATE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    STATE.get_or_init(|| {
        let irqs = Irqs::new();
        let memory = cmm2_memory(&irqs);
        memory.ds3231().set_host_clock(Box::new(host_clock()));
        let console = memory.console();
        let mut cpu = Cpu::new(&firmware(), memory, irqs).unwrap();
        boot_to_second_prompt(&mut cpu, &console);
        let (_, spent) = type_keys(&mut cpu, &console, "OPTION DS3231 ON\r");
        println!("OPTION DS3231 ON and the reboot to the prompt took {spent} instructions");
        assert_eq!(cpu.resets(), 2);
        cpu.persistent().unwrap()
    })
}

/// A machine started from `state`, with the host clock `clock`, run to its prompt. Returns the
/// instructions that took.
fn boot_from(state: &[u8], clock: &cmm2_core::FixedClock) -> (Cpu, Console, u64) {
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    memory.ds3231().set_host_clock(Box::new(clock.clone()));
    let console = memory.console();
    let mut cpu = Cpu::new(&firmware(), memory, irqs).unwrap();
    cpu.load_persistent(state).unwrap();
    run_to_text(&mut cpu, &console, "\r\n> ", 1_000_000_000);
    let spent = cpu.instruction_count();
    (cpu, console, spent)
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_firmware_boots_to_the_prompt_in_the_normal_time_with_the_ds3231_on() {
    let (cpu, _, spent) = boot_from(state_with_the_ds3231_on(), &host_clock());
    println!("prompt at {spent} instructions");
    // Measured: about 412 million with the option off. With nothing answering on the pins each
    // byte the firmware sends waits for an acknowledge, and the prompt came after 1.3 billion.
    assert_eq!(cpu.resets(), 0);
    assert!(spent < 450_000_000, "{spent}");
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_firmware_shows_the_host_time_and_date_from_the_ds3231() {
    let (mut cpu, console, _) = boot_from(state_with_the_ds3231_on(), &host_clock());
    assert_eq!(answer_to(&mut cpu, &console, "PRINT TIME$"), "13:45:56");
    assert_eq!(answer_to(&mut cpu, &console, "PRINT DATE$"), "24-12-2026");
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_firmware_sets_the_ds3231_and_the_time_counts_on_from_there() {
    let clock = host_clock();
    let (mut cpu, console, _) = boot_from(state_with_the_ds3231_on(), &clock);

    say_all(&mut cpu, &console, &["TIME$ = \"10:20:30\""]);
    assert_eq!(answer_to(&mut cpu, &console, "PRINT TIME$"), "10:20:30");
    assert_eq!(answer_to(&mut cpu, &console, "PRINT DATE$"), "24-12-2026");
    clock.advance(90);
    assert_eq!(answer_to(&mut cpu, &console, "PRINT TIME$"), "10:22:00");

    say_all(&mut cpu, &console, &["DATE$ = \"05/11/27\""]);
    assert_eq!(answer_to(&mut cpu, &console, "PRINT DATE$"), "05-11-2027");
    assert_eq!(answer_to(&mut cpu, &console, "PRINT TIME$"), "10:22:00");
    // The offset is in whole seconds from the host's clock, and the date went with it.
    assert!(cpu.memory().ds3231().offset_millis().is_some());
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_firmware_keeps_the_ds3231_time_across_a_save_and_load() {
    let clock = host_clock();
    let (mut cpu, console, _) = boot_from(state_with_the_ds3231_on(), &clock);
    say_all(
        &mut cpu,
        &console,
        &["TIME$ = \"10:20:30\"", "DATE$ = \"25/12/26\""],
    );
    let saved = cpu.persistent().unwrap();
    let offset = cpu.memory().ds3231().offset_millis();

    // The emulator is closed for an hour, and the host's clock goes on.
    clock.advance(3600);
    let (mut cpu, console, _) = boot_from(&saved, &clock);
    assert_eq!(cpu.memory().ds3231().offset_millis(), offset);
    assert_eq!(answer_to(&mut cpu, &console, "PRINT TIME$"), "11:20:30");
    assert_eq!(answer_to(&mut cpu, &console, "PRINT DATE$"), "25-12-2026");
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_firmware_counts_evenly_through_a_clock_change_once_the_ds3231_is_set() {
    let clock = host_clock();
    let (mut cpu, console, _) = boot_from(state_with_the_ds3231_on(), &clock);
    // Never set: it follows the host's wall clock, which jumps an hour.
    clock.set_zone(3600);
    assert_eq!(answer_to(&mut cpu, &console, "PRINT TIME$"), "14:45:56");
    say_all(&mut cpu, &console, &["TIME$ = \"10:00:00\""]);
    clock.advance(2);
    clock.set_zone(7200);
    assert_eq!(answer_to(&mut cpu, &console, "PRINT TIME$"), "10:00:02");
}

// The firmware writes a weekday of its own with every DATE$ (1 to 7, Sunday as 7), and the chip
// keeps it as it is given. It must be the day the date says, as it is on the real chip, so the
// weekday count that the saved state holds stays at 0.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_firmware_writes_the_ds3231_the_weekday_the_date_says() {
    let (mut cpu, console, _) = boot_from(state_with_the_ds3231_on(), &host_clock());
    // A Friday, and a Sunday, which the firmware numbers 5 and 7.
    for date in ["05/11/27", "07/11/27"] {
        say_all(&mut cpu, &console, &[&format!("DATE$ = \"{date}\"")]);
        let saved = cpu.memory().ds3231().saved().unwrap();
        assert_eq!(
            saved[8], 1,
            "the weekday count and the 12-hour form are 0: {saved:?}"
        );
    }
    assert_eq!(answer_to(&mut cpu, &console, "PRINT DATE$"), "07-11-2027");
}
