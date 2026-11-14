//! Batch runs: the limit, --stop-at, faults, the unmapped accesses and resets in the summary.

mod common;

use common::*;

// movs r0,#1; str r0,[0x6000_0000] (the str is at +6)
const UNMAPPED: [u16; 5] = [0xF04F, 0x41C0, 0x2001, 0x6008, 0xE7FE];

// Sets UE and TE in USART1_CR1 (the str, at +4), then sends "Hi!" through USART1_TDR; the
// address is the literal at +20.
const CONSOLE: [u16; 12] = [
    0x4804, 0x2109, 0x6001, 0x2248, 0x6282, 0x2269, 0x6282, 0x2221, 0x6282, 0xE7FE, 0x1000, 0x4001,
];

#[test]
fn run_stops_at_the_instruction_limit() {
    let output = run_program("limit", &[0xE7FE], &["--run", "10"]);

    assert!(output.status.success());
    assert_eq!(
        stdout(&output),
        "stopped: instruction limit\ninstructions 10\npc 0x08000100\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn console_output_follows_the_summary() {
    let output = run_program("console", &CONSOLE, &["--run", "10"]);

    assert!(output.status.success());
    assert_eq!(
        stdout(&output),
        "stopped: instruction limit\ninstructions 10\npc 0x08000112\nconsole output (3 bytes):\nHi!\n"
    );
}

#[test]
fn run_accepts_a_hex_count() {
    let output = run_program("hex-count", &[0xE7FE], &["--run", "0x10"]);

    assert!(stdout(&output).contains("\ninstructions 16\n"));
}

#[test]
fn stop_at_stops_before_the_instruction() {
    let output = run_program("stop-at", &THREE, &["--stop-at", "0x08000104"]);

    assert!(output.status.success());
    assert_eq!(
        stdout(&output),
        "stopped: address 0x08000104\ninstructions 2\npc 0x08000104\n"
    );
}

#[test]
fn run_and_stop_at_can_be_given_in_either_order() {
    let output = run_program("both", &THREE, &["--stop-at", "0x08000106", "--run", "100"]);

    assert!(stdout(&output).starts_with("stopped: address 0x08000106\n"));
}

#[test]
fn a_fault_is_reported_and_exits_with_one() {
    let output = run_program("fault", &UNDEFINED, &["--run", "10"]);

    assert_eq!(output.status.code(), Some(1));
    let out = stdout(&output);
    assert!(out.starts_with("stopped: fault trap: "), "{out}");
    assert!(out.contains("UndefInstr"), "{out}");
    assert!(out.contains("pc=0x08000100"), "{out}");
    assert!(out.contains("\ninstructions 1\n"), "{out}");
}

#[test]
fn unmapped_accesses_are_listed_in_the_summary() {
    let output = run_program("unmapped", &UNMAPPED, &["--run", "10"]);

    assert!(output.status.success());
    let out = stdout(&output);
    assert!(out.contains("\nunmapped accesses 1\n"), "{out}");
    assert!(
        out.ends_with("  0x08000106 write 0x60000000 size 4\n"),
        "{out}"
    );
}

#[test]
fn a_clean_run_has_no_unmapped_line() {
    let output = run_program("clean", &THREE, &["--run", "4"]);

    assert!(!stdout(&output).contains("unmapped"));
}

#[test]
fn a_system_reset_in_a_batch_run_is_reported_on_stderr() {
    let output = run_program("batch-reboot", &REBOOT, &["--run", "200000"]);

    assert!(output.status.success());
    assert!(stdout(&output).contains("ABC"));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        RESET_LINE.repeat(2)
    );
}

#[test]
fn a_run_without_a_reset_says_nothing_on_stderr() {
    let output = run_program("batch-no-reset", &THREE, &["--run", "10"]);

    assert!(output.stderr.is_empty());
}
