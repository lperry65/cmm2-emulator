//! --trace, --trace-mmio and --trace-from.

mod common;

use common::*;
use std::process::Output;

// Writes 42 to RCC_CR (the str, at +4), then reads it back (the ldr, at +6).
const RCC: [u16; 8] = [
    0x4902, 0x202A, 0x6008, 0x680A, 0xE7FE, 0x0000, 0x4400, 0x5802,
];

// Runs the program with a trace file and returns the output and the file's lines.
fn run_traced_program(name: &str, code: &[u16], args: &[&str]) -> (Output, Vec<String>) {
    let trace = temp_path(name, "txt");
    let mut all = vec!["--trace", trace.to_str().unwrap()];
    all.extend_from_slice(args);
    let output = run_program(name, code, &all);
    let text = std::fs::read_to_string(&trace).unwrap();
    std::fs::remove_file(&trace).unwrap();
    (output, text.lines().map(str::to_string).collect())
}

#[test]
fn trace_writes_one_line_per_instruction() {
    let trace = temp_path("trace", "txt");
    let output = run_program(
        "trace",
        &THREE,
        &["--run", "5", "--trace", trace.to_str().unwrap()],
    );
    let text = std::fs::read_to_string(&trace).unwrap();
    std::fs::remove_file(&trace).unwrap();

    assert!(output.status.success());
    assert!(stdout(&output).contains("\ninstructions 5\n"));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 5, "{text}");
    assert_eq!(lines[0], "0x08000100  mov r0, #1");
    assert_eq!(lines[1], "0x08000102  mov r1, #2");
    for line in &lines {
        let (addr, rest) = line.split_once("  ").unwrap();
        assert!(addr.starts_with("0x") && addr.len() == 10, "{line}");
        assert!(!rest.is_empty(), "{line}");
        assert!(!line.starts_with(' '), "{line}");
    }
}

#[test]
fn trace_mmio_prints_peripheral_accesses() {
    let output = run_program("mmio", &RCC, &["--trace-mmio", "--run", "6"]);

    assert!(output.status.success());
    let out = stdout(&output);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "    W RCC_CR 0x58024400 size 4 = 0x0000002A");
    assert_eq!(lines[1], "    R RCC_CR 0x58024400 size 4 = 0x0000C02A");
    assert!(lines[2].starts_with("stopped: "), "{out}");
}

#[test]
fn trace_mmio_names_an_address_inside_a_register() {
    // ldr r1,[pc,#4]; ldrb r0,[r1]; b .; pool = 0x5802_4401
    let code = [0x4901, 0x7808, 0xE7FE, 0x0000, 0x4401, 0x5802];
    let output = run_program("mmio-offset", &code, &["--trace-mmio", "--run", "3"]);

    assert!(
        stdout(&output).starts_with("    R RCC_CR +1 0x58024401 size 1 = 0x000000C1\n"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn trace_mmio_leaves_unknown_registers_unnamed() {
    // ldr r1,[pc,#4]; ldr r0,[r1]; b .; pool = 0x5FFF_0000
    let code = [0x4901, 0x6808, 0xE7FE, 0x0000, 0x0000, 0x5FFF];
    let output = run_program("mmio-unknown", &code, &["--trace-mmio", "--run", "3"]);

    assert!(
        stdout(&output).starts_with("    R ? 0x5FFF0000 size 4 = 0x00000000\n"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn trace_file_holds_the_accesses_after_their_instruction() {
    let trace = temp_path("trace-mmio", "txt");
    let output = run_program(
        "trace-mmio",
        &RCC,
        &[
            "--run",
            "6",
            "--trace",
            trace.to_str().unwrap(),
            "--trace-mmio",
        ],
    );
    let text = std::fs::read_to_string(&trace).unwrap();
    std::fs::remove_file(&trace).unwrap();

    assert!(output.status.success());
    assert!(!stdout(&output).contains("RCC_CR"));
    let lines: Vec<&str> = text.lines().collect();
    let store = lines
        .iter()
        .position(|line| line.starts_with("0x08000104  "))
        .unwrap();
    assert_eq!(
        lines[store + 1],
        "    W RCC_CR 0x58024400 size 4 = 0x0000002A"
    );
    assert!(lines[store + 2].starts_with("0x08000106  "), "{text}");
    assert_eq!(
        lines[store + 3],
        "    R RCC_CR 0x58024400 size 4 = 0x0000C02A"
    );
}

#[test]
fn trace_file_that_cannot_be_created_fails() {
    let output = run_program(
        "bad-trace",
        &THREE,
        &["--run", "5", "--trace", "/nonexistent/trace.txt"],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("cmm2-emulator: /nonexistent/trace.txt: "),
        "{stderr}"
    );
}

#[test]
fn trace_from_starts_the_trace_at_that_instruction() {
    let (output, lines) =
        run_traced_program("from-loop", &THREE, &["--trace-from", "5", "--run", "10"]);

    assert!(output.status.success());
    assert!(stdout(&output).contains("\ninstructions 10\n"));
    // Instructions 0 to 2 are the movs, the rest is the loop at +6.
    assert_eq!(lines.len(), 5, "{lines:?}");
    assert!(lines[0].starts_with("0x08000106  "), "{lines:?}");
    assert!(lines.iter().all(|line| line == &lines[0]), "{lines:?}");
}

#[test]
fn trace_from_can_start_in_the_straight_line_code() {
    let (_, lines) = run_traced_program("from-two", &THREE, &["--trace-from", "0x2", "--run", "4"]);

    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0], "0x08000104  mov r2, #3");
    assert!(lines[1].starts_with("0x08000106  "), "{lines:?}");
}

#[test]
fn trace_from_leaves_out_the_accesses_before_it() {
    // The store (instruction 2) writes RCC_CR, the load (instruction 3) reads it back.
    let (_, lines) = run_traced_program(
        "from-mmio",
        &RCC,
        &["--trace-mmio", "--trace-from", "3", "--run", "6"],
    );

    assert!(lines[0].starts_with("0x08000106  "), "{lines:?}");
    assert_eq!(lines[1], "    R RCC_CR 0x58024400 size 4 = 0x0000C02A");
    assert!(!lines.iter().any(|line| line.contains(" W ")), "{lines:?}");
}

#[test]
fn trace_from_after_the_end_traces_nothing() {
    let (output, lines) = run_traced_program(
        "from-late",
        &RCC,
        &["--trace-mmio", "--trace-from", "100", "--run", "6"],
    );

    assert!(output.status.success());
    assert!(lines.is_empty(), "{lines:?}");
}

#[test]
fn trace_from_with_the_console_keeps_the_screen_clean() {
    // Instruction 3 is in the first slice, 60000 in the second.
    for from in [3, 60_000] {
        let (output, lines) = run_traced_program(
            "from-console",
            &ECHO,
            &[
                "--console",
                "--on",
                "ready",
                "ab",
                "--trace-mmio",
                "--trace-from",
                &from.to_string(),
                "--run",
                "70000",
            ],
        );

        assert!(output.status.success(), "{from}");
        assert_eq!(stdout(&output), "ready\r\nbc", "{from}");
        let instructions = lines.iter().filter(|line| !line.starts_with(' ')).count();
        assert_eq!(instructions, 70_000 - from, "{from}");
        if from == 3 {
            assert!(lines[0].starts_with("0x08000106  "), "{from}");
        }
    }
}
