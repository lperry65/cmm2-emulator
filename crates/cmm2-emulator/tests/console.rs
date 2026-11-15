//! --console: the guest's output, --on rules, piped keys and the quit key.

mod common;

use common::*;
use std::io::{Read, Write};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const LIMIT_LINE: &str = "cmm2-emulator: instruction limit reached\n";

// Runs the program as `--console` with `args` after it; stdin is closed, so only the rules type.
fn run_console(name: &str, args: &[&str]) -> Output {
    let mut all = vec!["--console"];
    all.extend_from_slice(args);
    run_program(name, &ECHO, &all)
}

#[test]
fn console_prints_the_guest_output_as_it_is() {
    let output = run_console("console-plain", &["--run", "200000"]);

    assert!(output.status.success());
    assert_eq!(stdout(&output), "ready\r\n");
    assert_eq!(stderr(&output), LIMIT_LINE);
}

#[test]
fn a_rule_types_keys_when_its_text_appears() {
    let output = run_console("console-on", &["--on", "ready", "ab", "--run", "2000000"]);

    assert!(output.status.success());
    assert_eq!(stdout(&output), "ready\r\nbc");
}

#[test]
fn rules_understand_escapes() {
    let output = run_console(
        "console-escapes",
        &["--on", "ready", "\\x41\\x30", "--run", "2000000"],
    );

    assert_eq!(stdout(&output), "ready\r\nB1");
}

#[test]
fn a_rule_whose_text_never_appears_sends_nothing() {
    let output = run_console(
        "console-never",
        &["--on", "never printed", "ab", "--run", "2000000"],
    );

    assert!(output.status.success());
    assert_eq!(stdout(&output), "ready\r\n");
}

#[test]
fn console_stops_at_an_address_without_a_summary() {
    let output = run_program(
        "console-stop-at",
        &THREE,
        &["--console", "--stop-at", "0x08000104"],
    );

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        stderr(&output),
        "cmm2-emulator: reached address 0x08000104\n"
    );
}

#[test]
fn a_fault_in_the_console_goes_to_stderr_and_exits_with_one() {
    let output = run_program("console-fault", &UNDEFINED, &["--console"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("cmm2-emulator: fault trap: "),
        "{stderr}"
    );
    assert!(stderr.contains("UndefInstr"), "{stderr}");
}

#[test]
fn piped_keys_reach_the_guest_and_the_quit_key_ends_the_run() {
    let path = temp_program("console-piped", &ECHO);
    let mut child = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .args(["--firmware", path.to_str().unwrap()])
        .args(["--console", "--run", "500000000"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to run cmm2-emulator");
    let mut stdin = child.stdin.take().unwrap();
    let mut out = child.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut byte = [0];
        while out.read(&mut byte).is_ok_and(|n| n == 1) {
            if sender.send(byte[0]).is_err() {
                break;
            }
        }
    });

    stdin.write_all(b"a").unwrap();
    stdin.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut seen = Vec::new();
    while !seen.ends_with(b"b") {
        let left = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(left) {
            Ok(byte) => seen.push(byte),
            Err(_) => {
                child.kill().unwrap();
                panic!("no echo, got {:?}", String::from_utf8_lossy(&seen));
            }
        }
    }
    stdin.write_all(&[0x1D]).unwrap();
    stdin.flush().unwrap();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            panic!("the quit key did not end the run");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    std::fs::remove_file(&path).unwrap();

    assert!(status.success());
    assert_eq!(String::from_utf8_lossy(&seen), "ready\r\nb");
    // Nothing more arrives: the quit key itself is not sent to the guest.
    assert_eq!(receiver.recv_timeout(Duration::from_secs(5)).ok(), None);
}

#[test]
fn a_system_reset_in_the_console_restarts_the_guest_and_is_reported_on_stderr() {
    let output = run_program("console-reboot", &REBOOT, &["--console", "--run", "200000"]);

    assert!(output.status.success());
    assert_eq!(stdout(&output), "ABC");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        RESET_LINE.repeat(2) + LIMIT_LINE
    );
}
