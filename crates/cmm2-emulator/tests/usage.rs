//! Usage, arguments and the commands that only look: --version, --lookup, --firmware with no run.

mod common;

use common::*;
use std::path::PathBuf;

const USAGE: &str = "usage: cmm2-emulator [--firmware <file>] [--run <count>] [--stop-at <address>] [--trace <file>] [--trace-mmio] [--trace-from <count>] [--console] [--on <text> <keys>] [--screenshot <file>] [--window] [--console-keys] [--unpaced] [--flash <file>] [--sd <file> | --no-sd] [--burn <file>] [--rng-seed <number>] | --version | --lookup <address>";

// Writes a file with the given vector table words to a unique temp path.
fn temp_firmware(name: &str, sp: u32, reset: u32) -> PathBuf {
    let path = temp_path(name, "bin");
    let mut bytes = vec![0; 64];
    bytes[..4].copy_from_slice(&sp.to_le_bytes());
    bytes[4..8].copy_from_slice(&reset.to_le_bytes());
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn version_flag_prints_name_and_version() {
    let output = emulator()
        .arg("--version")
        .output()
        .expect("failed to run cmm2-emulator");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        concat!("cmm2-emulator ", env!("CARGO_PKG_VERSION"), "\n")
    );
}

#[test]
fn unknown_argument_is_rejected() {
    let output = emulator()
        .arg("--bogus")
        .output()
        .expect("failed to run cmm2-emulator");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!("cmm2-emulator: unknown argument '--bogus'\n{USAGE}\n")
    );
}

#[test]
fn firmware_prints_vector_table() {
    let path = temp_firmware("valid", 0x2001_FFF0, 0x0800_0021);
    let output = run(&["--firmware", path.to_str().unwrap()]);
    std::fs::remove_file(&path).unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "initial SP 0x2001FFF0\nreset handler 0x08000021\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn firmware_missing_file_fails() {
    let output = run(&["--firmware", "/nonexistent/firmware.bin"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("cmm2-emulator: /nonexistent/firmware.bin: "),
        "{stderr}"
    );
}

// The firmware's name reaches the refusal through the core's own error, which the run hands over
// whole. A name with an escape and a line feed in it is still one line, broken nowhere and nothing
// a terminal acts on, and the name is still there to read.
#[test]
fn a_firmware_named_with_control_characters_is_refused_in_one_unbroken_line() {
    let name = "/nonexistent/evil\n\x1b[31mRED\x1b[0m.bin";

    let output = run(&["--firmware", name, "--run", "10"]);
    let err = stderr(&output);

    assert_eq!(output.status.code(), Some(1), "{err}");
    assert!(output.stdout.is_empty());
    assert_eq!(err.matches('\n').count(), 1, "{err:?}");
    assert_eq!(err.lines().count(), 1, "{err:?}");
    assert!(
        !err.contains('\x1b'),
        "the name recoloured the line: {err:?}"
    );
    assert!(err.starts_with("cmm2-emulator: "), "{err:?}");
    assert!(
        err.contains(r"/nonexistent/evil\n\x1B[31mRED\x1B[0m.bin"),
        "the name is not said: {err}"
    );
    assert!(
        err.contains("No such file"),
        "the reason is not said: {err}"
    );
}

#[test]
fn firmware_implausible_file_fails() {
    let path = temp_firmware("bad-sp", 0x1234_5678, 0x0800_0021);
    let output = run(&["--firmware", path.to_str().unwrap()]);
    std::fs::remove_file(&path).unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "cmm2-emulator: initial stack pointer 0x12345678 is not in RAM\n"
    );
}

#[test]
fn firmware_without_path_is_a_usage_error() {
    let output = run(&["--firmware"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!("{USAGE}\n")
    );
}

#[test]
fn lookup_prints_register_and_fields() {
    let output = run(&["--lookup", "0x58024400"]);

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    assert_eq!(lines.next(), Some("RCC_CR 0x58024400"));
    assert!(stdout.contains("\n  HSION bit 0\n"), "{stdout}");
    assert!(stdout.contains("\n  HSIDIV bits 3-4\n"), "{stdout}");
    assert!(output.stderr.is_empty());
}

#[test]
fn lookup_finds_a_register_in_another_peripheral() {
    let output = run(&["--lookup", "0x5200414C"]);

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("FMC_SDTR2 0x5200414C\n"), "{stdout}");
    assert!(stdout.contains("\n  TMRD bits 0-3\n"), "{stdout}");
}

#[test]
fn lookup_inside_a_register_shows_the_byte_offset() {
    let output = run(&["--lookup", "0x58024401"]);

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("RCC_CR 0x58024400 +1\n"), "{stdout}");
}

#[test]
fn lookup_of_an_unknown_address_is_not_an_error() {
    let output = run(&["--lookup", "0x12345678"]);

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "unknown address 0x12345678\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn lookup_with_a_malformed_address_is_a_usage_error() {
    for bad in ["nope", "58024400", "0x", "0x1ffffffff"] {
        let output = run(&["--lookup", bad]);

        assert_eq!(output.status.code(), Some(2), "{bad}");
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            format!("cmm2-emulator: bad address '{bad}'\n{USAGE}\n")
        );
    }
}

#[test]
fn lookup_without_an_address_is_a_usage_error() {
    let output = run(&["--lookup"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!("{USAGE}\n")
    );
}

#[test]
fn console_keys_without_the_window_is_a_usage_error() {
    let output = run_program("console-keys", &SCREEN, &["--console-keys", "--run", "40"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .starts_with("cmm2-emulator: --console-keys needs --window\n")
    );
}

#[test]
fn bad_run_options_are_usage_errors() {
    let cases: [(&[&str], &str); 17] = [
        (
            &["--trace-from", "5"],
            "--trace-from needs --trace or --trace-mmio",
        ),
        (
            &["--burn", "a", "--firmware", "b", "--run", "9"],
            "--burn puts the firmware on the board and --firmware runs one that is not on it, \
             not both",
        ),
        (
            &["--firmware", "f", "--console", "--no-sd"],
            "--no-sd is for the board, which --firmware does not use",
        ),
        (
            &["--burn", "a", "--burn", "b"],
            "--burn is given twice, which one is meant?",
        ),
        (
            &["--firmware", "a", "--firmware", "b", "--run", "9"],
            "unknown argument '--firmware'",
        ),
        (
            &["--sd", "c", "--no-sd"],
            "--sd puts a card in the socket and --no-sd takes it out, not both",
        ),
        (&["--firmware", "f", "--trace-from"], ""),
        (
            &["--firmware", "f", "--trace-from", "abc"],
            "bad count 'abc'",
        ),
        (
            &["--firmware", "f", "--run", "9", "--trace-from", "5"],
            "--trace-from needs --trace or --trace-mmio",
        ),
        (
            &["--firmware", "f", "--trace-from", "5"],
            "--trace-from needs --trace or --trace-mmio",
        ),
        (&["--firmware", "f", "--run"], ""),
        (&["--firmware", "f", "--run", "abc"], "bad count 'abc'"),
        (&["--firmware", "f", "--run", "0x"], "bad count '0x'"),
        (&["--firmware", "f", "--stop-at"], ""),
        (
            &["--firmware", "f", "--stop-at", "1234"],
            "bad address '1234'",
        ),
        (&["--firmware", "f", "--trace"], ""),
        (
            &["--firmware", "f", "--bogus"],
            "unknown argument '--bogus'",
        ),
    ];
    for (args, message) in cases {
        let output = run(args);

        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        let expected = if message.is_empty() {
            format!("{USAGE}\n")
        } else {
            format!("cmm2-emulator: {message}\n{USAGE}\n")
        };
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            expected,
            "{args:?}"
        );
    }
}

#[test]
fn trace_without_anything_to_run_is_a_usage_error() {
    for option in [&["--trace", "out.txt"][..], &["--trace-mmio"][..]] {
        let mut args = vec!["--firmware", "f"];
        args.extend_from_slice(option);
        let output = run(&args);

        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            format!("cmm2-emulator: nothing to run, give --run or --stop-at\n{USAGE}\n"),
            "{args:?}"
        );
    }
}

#[test]
fn bad_console_options_are_usage_errors() {
    let cases: [(&[&str], &str); 5] = [
        (&["--on", "a", "b"], "--on needs --console"),
        (
            &["--firmware", "f", "--run", "1", "--on", "a", "b"],
            "--on needs --console",
        ),
        (
            &["--firmware", "f", "--console", "--trace-mmio"],
            "--console with --trace-mmio needs --trace, the screen belongs to the guest",
        ),
        (
            &["--firmware", "f", "--console", "--on", "a", "\\q"],
            "bad keys '\\q'",
        ),
        (&["--firmware", "f", "--console", "--on", "a"], ""),
    ];
    for (args, message) in cases {
        let output = run(args);

        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        let expected = if message.is_empty() {
            format!("{USAGE}\n")
        } else {
            format!("cmm2-emulator: {message}\n{USAGE}\n")
        };
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            expected,
            "{args:?}"
        );
    }
}

#[test]
fn an_empty_file_name_is_a_usage_error_for_every_file_option() {
    // An unset shell variable gives one, and it would otherwise be found out at the end.
    for option in ["--flash", "--sd", "--trace", "--screenshot"] {
        let output = run_program("empty-name", &THREE, &["--run", "10", option, ""]);
        assert_eq!(output.status.code(), Some(2), "{option}");
        assert!(output.stdout.is_empty());
        assert_eq!(
            stderr(&output),
            format!("cmm2-emulator: {option} needs a file name, not an empty one\n{USAGE}\n"),
            "{option}"
        );
    }
}

#[test]
fn an_empty_firmware_name_is_loaded_and_fails_there_as_it_did() {
    let output = run(&["--firmware", ""]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).starts_with("cmm2-emulator: : "),
        "{}",
        stderr(&output)
    );
}
