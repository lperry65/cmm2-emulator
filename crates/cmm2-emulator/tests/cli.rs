use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[test]
fn version_flag_prints_name_and_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
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
    let output = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
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
fn no_arguments_prints_usage() {
    let output = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .output()
        .expect("failed to run cmm2-emulator");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!("{USAGE}\n")
    );
}

const USAGE: &str = "usage: cmm2-emulator --version | --lookup <address> | --firmware <file> [--run <count>] [--stop-at <address>] [--trace <file>] [--trace-mmio] [--trace-from <count>] [--console] [--on <text> <keys>] [--screenshot <file>] [--window] [--console-keys] [--unpaced] [--flash <file>] [--sd <file>]";

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .args(args)
        // The window tests have no display to open on.
        .env("SDL_VIDEODRIVER", "dummy")
        .output()
        .expect("failed to run cmm2-emulator")
}

fn temp_path(name: &str, extension: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "cmm2-cli-{}-{name}.{extension}",
        std::process::id()
    ))
}

// Writes a file with the given vector table words to a unique temp path.
fn temp_firmware(name: &str, sp: u32, reset: u32) -> PathBuf {
    let path = temp_path(name, "bin");
    let mut bytes = vec![0; 64];
    bytes[..4].copy_from_slice(&sp.to_le_bytes());
    bytes[4..8].copy_from_slice(&reset.to_le_bytes());
    std::fs::write(&path, bytes).unwrap();
    path
}

const CODE: u32 = 0x0800_0100;

// Writes a firmware image whose reset handler is `code` at CODE, as Thumb halfwords.
fn temp_program(name: &str, code: &[u16]) -> PathBuf {
    let path = temp_path(name, "bin");
    let mut bytes = vec![0; 0x100];
    bytes[..4].copy_from_slice(&0x2000_8000u32.to_le_bytes());
    bytes[4..8].copy_from_slice(&(CODE | 1).to_le_bytes());
    bytes.extend(code.iter().flat_map(|half| half.to_le_bytes()));
    std::fs::write(&path, bytes).unwrap();
    path
}

// movs r0,#1; movs r1,#2; movs r2,#3; b .
const THREE: [u16; 4] = [0x2001, 0x2102, 0x2203, 0xE7FE];
// Writes 42 to RCC_CR (the str, at +4), then reads it back (the ldr, at +6).
const RCC: [u16; 8] = [
    0x4902, 0x202A, 0x6008, 0x680A, 0xE7FE, 0x0000, 0x4400, 0x5802,
];
// movs r0,#1; str r0,[0x6000_0000] (the str is at +6)
const UNMAPPED: [u16; 5] = [0xF04F, 0x41C0, 0x2001, 0x6008, 0xE7FE];
// Sets UE and TE in USART1_CR1 (the str, at +4), then sends "Hi!" through USART1_TDR; the
// address is the literal at +20.
const CONSOLE: [u16; 12] = [
    0x4804, 0x2109, 0x6001, 0x2248, 0x6282, 0x2269, 0x6282, 0x2221, 0x6282, 0xE7FE, 0x1000, 0x4001,
];
// Sets UE, TE and RE in USART1_CR1, sends "ready\r\n", then echoes each byte it receives plus
// one. The address of USART1 is the literal at +52.
const ECHO: [u16; 28] = [
    0x480C, 0x210D, 0x6001, 0x2272, 0x6282, 0x2265, 0x6282, 0x2261, 0x6282, 0x2264, 0x6282, 0x2279,
    0x6282, 0x220D, 0x6282, 0x220A, 0x6282, 0x69C2, 0x2320, 0x421A, 0xD0FB, 0x6A42, 0x3201, 0x6282,
    0xE7F7, 0x0000, 0x1000, 0x4001,
];
const UNDEFINED: [u16; 1] = [0xDE00]; // udf #0
// Counts its boots in AXI SRAM, sends a letter for each ('A' for the first) and asks for a
// system reset until the third boot, which spins. The literals are at +32: the counter's
// address, USART1, AIRCR and the reset request.
const REBOOT: [u16; 24] = [
    0x4807, 0x6801, 0x3101, 0x6001, 0x4A06, 0x2309, 0x6013, 0x2440, 0x1864, 0x6294, 0x2903, 0xDA02,
    0x4A03, 0x4B04, 0x6013, 0xE7FE, 0x0100, 0x2400, 0x1000, 0x4001, 0xED0C, 0xE000, 0x0004, 0x05FA,
];
const RESET_LINE: &str = "cmm2-emulator: system reset\n";

// Runs the program with `args` after `--firmware <image>`, and removes the image.
fn run_program(name: &str, code: &[u16], args: &[&str]) -> Output {
    let path = temp_program(name, code);
    let mut all = vec!["--firmware", path.to_str().unwrap()];
    all.extend_from_slice(args);
    let output = run(&all);
    std::fs::remove_file(&path).unwrap();
    output
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
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

// Brings up a 1 x 1 screen: the LTDC enabled, active area 1 x 1, layer 1 on it in RGB565 at the
// start of AXI SRAM, fetching 1 line, reloaded, and one red pixel stored there. The addresses
// are literals at +56.
const SCREEN: [u16; 46] = [
    0x480D, 0x490E, 0x4A0E, 0x6182, 0x4A0E, 0x6102, 0x4A0C, 0x604A, 0x4A0C, 0x608A, 0x4A0B, 0x60CA,
    0x4A0B, 0x614A, 0x4A0B, 0x618A, 0x4A0B, 0x62CA, 0x4A0B, 0x630A, 0x4A05, 0x634A, 0x4A04, 0x6242,
    0x4A09, 0x4B07, 0x801A, 0xE7FE, 0x1000, 0x5000, 0x1080, 0x5000, 0x0001, 0x0000, 0x0001, 0x0001,
    0x0002, 0x0000, 0x00FF, 0x0000, 0x0000, 0x2400, 0x0009, 0x0002, 0xF800, 0x0000,
];

#[test]
fn screenshot_writes_the_screen_as_a_ppm_file() {
    let image = temp_path("screenshot", "ppm");
    let output = run_program(
        "screenshot",
        &SCREEN,
        &["--run", "40", "--screenshot", image.to_str().unwrap()],
    );

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        std::fs::read(&image).unwrap(),
        b"P6\n1 1\n255\n\xFF\x00\x00"
    );
    std::fs::remove_file(&image).unwrap();
}

#[test]
fn window_shows_the_screen_and_ends_with_the_run() {
    let output = run_program("window", &SCREEN, &["--window", "--run", "40"]);

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
}

#[test]
fn window_says_once_that_there_is_no_picture() {
    let output = run_program("window-blank", &THREE, &["--window", "--run", "400000"]);

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "cmm2-emulator: no picture: the LTDC is not enabled\n"
    );
}

#[test]
fn window_goes_with_the_console() {
    let output = run_program(
        "window-console",
        &SCREEN,
        &["--window", "--console", "--run", "40"],
    );

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
}

#[test]
fn window_keys_can_go_to_the_console() {
    let output = run_program(
        "window-console-keys",
        &SCREEN,
        &["--window", "--console-keys", "--run", "40"],
    );

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
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
fn unpaced_goes_with_the_live_modes() {
    let output = run_program(
        "window-unpaced",
        &SCREEN,
        &["--window", "--unpaced", "--run", "40"],
    );

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
}

#[test]
fn screenshot_of_a_machine_with_no_screen_fails() {
    let image = temp_path("no-screen", "ppm");
    let output = run_program(
        "no-screen",
        &THREE,
        &["--run", "4", "--screenshot", image.to_str().unwrap()],
    );

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "cmm2-emulator: no screenshot: the LTDC is not enabled\n"
    );
    assert!(!image.exists());
}

#[test]
fn screenshot_that_cannot_be_written_fails() {
    let output = run_program(
        "bad-screenshot",
        &SCREEN,
        &["--run", "40", "--screenshot", "/nonexistent/s.ppm"],
    );

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("cmm2-emulator: /nonexistent/s.ppm: "),
        "{stderr}"
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
fn bad_run_options_are_usage_errors() {
    let cases: [(&[&str], &str); 15] = [
        (&["--run", "10"], "--run needs --firmware"),
        (&["--trace-from", "5"], "--trace-from needs --firmware"),
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
        (&["--stop-at", "0x1000"], "--stop-at needs --firmware"),
        (&["--trace-mmio"], "--trace-mmio needs --firmware"),
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
    assert!(output.stderr.is_empty());
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
    assert!(output.stderr.is_empty());
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
fn bad_console_options_are_usage_errors() {
    let cases: [(&[&str], &str); 6] = [
        (&["--console"], "--console needs --firmware"),
        (&["--on", "a", "b"], "--on needs --firmware"),
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

#[test]
fn a_system_reset_in_the_console_restarts_the_guest_and_is_reported_on_stderr() {
    let output = run_program("console-reboot", &REBOOT, &["--console", "--run", "200000"]);

    assert!(output.status.success());
    assert_eq!(stdout(&output), "ABC");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        RESET_LINE.repeat(2)
    );
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

// The flash file is the last 0xE0000 bytes of the flash, the saved options in its last sector,
// then the clock's time and date, a word each.
const FLASH_FILE_SIZE: usize = 0xE_0008;
const OPTIONS_AT: usize = 0xC_0000;
// Adds one to the word at 0x081E_0000, the start of the saved options (the literal is at +14),
// then ends as `end`. An erased word is -1, so the first run leaves 0.
fn count_in_flash(end: u16) -> [u16; 8] {
    [0x4802, 0x6801, 0x3101, 0x6001, end, 0x0000, 0x0000, 0x081E]
}
const SPIN: u16 = 0xE7FE; // b .

fn options_word(flash: &[u8]) -> u32 {
    u32::from_le_bytes(flash[OPTIONS_AT..OPTIONS_AT + 4].try_into().unwrap())
}

fn flash_path(name: &str) -> PathBuf {
    temp_path(name, "flash")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn the_flash_file_keeps_the_flash_between_runs() {
    let flash = flash_path("flash-keeps");
    let _ = std::fs::remove_file(&flash);
    let path = flash.to_str().unwrap();
    let program = count_in_flash(SPIN);

    let first = run_program(
        "flash-keeps-1",
        &program,
        &["--run", "100", "--flash", path],
    );
    assert!(first.status.success(), "{}", stderr(&first));
    let saved = std::fs::read(&flash).unwrap();
    assert_eq!(saved.len(), FLASH_FILE_SIZE);
    assert_eq!(options_word(&saved), 0);

    let second = run_program(
        "flash-keeps-2",
        &program,
        &["--run", "100", "--flash", path],
    );
    assert!(second.status.success(), "{}", stderr(&second));
    assert_eq!(options_word(&std::fs::read(&flash).unwrap()), 1);
    std::fs::remove_file(&flash).unwrap();
}

#[test]
fn the_flash_file_is_written_by_a_console_run_too() {
    let flash = flash_path("flash-console");
    let _ = std::fs::remove_file(&flash);
    let path = flash.to_str().unwrap();

    let output = run_program(
        "flash-console",
        &count_in_flash(SPIN),
        &["--console", "--run", "100", "--flash", path],
    );

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(options_word(&std::fs::read(&flash).unwrap()), 0);
    std::fs::remove_file(&flash).unwrap();
}

#[test]
fn the_flash_file_is_written_when_the_run_stops_at_an_address() {
    let flash = flash_path("flash-stop-at");
    let _ = std::fs::remove_file(&flash);
    let path = flash.to_str().unwrap();

    let output = run_program(
        "flash-stop-at",
        &count_in_flash(SPIN),
        &["--stop-at", "0x08000108", "--flash", path],
    );

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(options_word(&std::fs::read(&flash).unwrap()), 0);
    std::fs::remove_file(&flash).unwrap();
}

#[test]
fn the_flash_file_is_written_when_the_guest_faults() {
    for (name, live) in [
        ("flash-fault-batch", &["--run", "100"][..]),
        ("flash-fault-live", &["--console"][..]),
    ] {
        let flash = flash_path(name);
        let _ = std::fs::remove_file(&flash);
        let mut args = vec!["--flash", flash.to_str().unwrap()];
        args.extend_from_slice(live);

        let output = run_program(name, &count_in_flash(0xDE00), &args);

        assert_eq!(output.status.code(), Some(1), "{name}");
        assert_eq!(options_word(&std::fs::read(&flash).unwrap()), 0, "{name}");
        std::fs::remove_file(&flash).unwrap();
    }
}

#[test]
fn the_flash_file_is_written_when_the_quit_key_ends_the_run() {
    let flash = flash_path("flash-quit");
    let _ = std::fs::remove_file(&flash);
    let path = temp_program("flash-quit", &count_in_flash(SPIN));
    let mut child = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .args(["--firmware", path.to_str().unwrap(), "--console", "--flash"])
        .arg(&flash)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("failed to run cmm2-emulator");
    child.stdin.as_mut().unwrap().write_all(b"\x1D").unwrap();
    let status = child.wait().unwrap();

    assert!(status.success());
    assert_eq!(options_word(&std::fs::read(&flash).unwrap()), 0);
    std::fs::remove_file(&flash).unwrap();
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn saving_the_flash_leaves_no_temporary_file() {
    // The temporary file is made beside the flash file, so that directory shows it.
    let dir = temp_path("flash-temp", "d");
    std::fs::create_dir(&dir).unwrap();
    let flash = dir.join("board.flash");

    run_program(
        "flash-temp",
        &THREE,
        &["--run", "10", "--flash", flash.to_str().unwrap()],
    );

    assert!(flash.exists());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_flash_file_of_the_wrong_size_is_an_error_and_is_left_alone() {
    let flash = flash_path("flash-size");
    std::fs::write(&flash, [1, 2, 3]).unwrap();

    let output = run_program(
        "flash-size",
        &THREE,
        &["--run", "10", "--flash", flash.to_str().unwrap()],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        stderr(&output),
        format!(
            "cmm2-emulator: {}: the saved flash and clock are {FLASH_FILE_SIZE} bytes, not 3\n",
            flash.display()
        )
    );
    assert_eq!(std::fs::read(&flash).unwrap(), [1, 2, 3]);
    std::fs::remove_file(&flash).unwrap();
}

#[test]
fn a_flash_file_that_cannot_be_read_is_an_error() {
    let output = run_program(
        "flash-unreadable",
        &THREE,
        &[
            "--run",
            "10",
            "--flash",
            std::env::temp_dir().to_str().unwrap(),
        ],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).starts_with("cmm2-emulator: "),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_flash_file_in_a_directory_that_is_not_there_is_an_error_at_the_start() {
    let missing = std::env::temp_dir().join("cmm2-cli-no-such-directory/f.flash");

    let output = run_program(
        "flash-unwritable",
        &THREE,
        &["--run", "10", "--flash", missing.to_str().unwrap()],
    );

    assert_eq!(output.status.code(), Some(1));
    // Found at the start, and not when the run is over and its session is lost.
    assert!(output.stdout.is_empty());
    assert_eq!(
        stderr(&output),
        format!(
            "cmm2-emulator: {}: its directory {} does not exist, so the run could not save its flash and clock\n",
            missing.display(),
            missing.parent().unwrap().display()
        )
    );
}

// Writes 0x5A to every byte of block 1 of the SD card, which it drives through the GPIO pins
// the way the firmware does (CMD0, CMD55, ACMD41 and CMD24 over port C and port D), then prints
// "done\n" on USART1 and ends as `end`. It was assembled with GNU as for a Cortex-M7 (the
// source is not kept: it is a bit-banged SPI byte routine and the command sequence).
const WRITE_CARD: [u16; 114] = [
    0xF640, 0x0000, 0xF6C5, 0x0002, 0xF640, 0x4100, 0xF6C5, 0x0102, 0xF64F, 0x72FF, 0xF6CF, 0x521F,
    0x6002, 0xF64F, 0x72DF, 0xF6CF, 0x72FF, 0x600A, 0x2201, 0x0492, 0x618A, 0x2640, 0x2700, 0xF000,
    0xF82C, 0x2677, 0x2700, 0xF000, 0xF828, 0x2669, 0x2740, 0x063F, 0xF000, 0xF823, 0x2658, 0x2701,
    0xF000, 0xF81F, 0x22FE, 0xF000, 0xF837, 0xF240, 0x2802, 0x225A, 0xF000, 0xF832, 0xF1B8, 0x0801,
    0xD1F9, 0x22FF, 0xF000, 0xF82C, 0xF241, 0x0000, 0xF2C4, 0x0001, 0x2109, 0x6001, 0x2164, 0x6281,
    0x216F, 0x6281, 0x216E, 0x6281, 0x2165, 0x6281, 0x210A, 0x6281, 0xE7FE, 0xB500, 0x4632, 0xF000,
    0xF817, 0x0E3A, 0xF000, 0xF814, 0x0C3A, 0xF000, 0xF811, 0x0A3A, 0xF000, 0xF80E, 0x463A, 0xF000,
    0xF80B, 0x2201, 0xF000, 0xF808, 0x22FF, 0xF000, 0xF805, 0xF000, 0xF803, 0xF000, 0xF801, 0xBD00,
    0x2308, 0x0614, 0xF241, 0x0500, 0xBF58, 0xF04F, 0x5580, 0x6185, 0xF44F, 0x6580, 0x6185, 0xF04F,
    0x6580, 0x6185, 0x0052, 0x3B01, 0xD1EF, 0x4770,
];
// Where in WRITE_CARD the end of the program is, and its address.
const WRITE_CARD_END: usize = 68;
const WRITE_CARD_END_ADDRESS: &str = "0x08000188";

fn write_card(end: u16) -> [u16; 114] {
    let mut code = WRITE_CARD;
    code[WRITE_CARD_END] = end;
    code
}

// The smallest card there is, 512 KB: its first four blocks are filled with their own number
// (1 to 4) and the rest with zeros.
fn card_image() -> Vec<u8> {
    let mut image: Vec<u8> = (1..=4u8).flat_map(|b| [b; 512]).collect();
    image.resize(512 * 1024, 0);
    image
}

// A card image as `card_image` makes it, and its inode and time of
// writing. A card the guest did not write to is not written back, so the file is the same one;
// one that it did is a new file, as it is written through a temporary file and a rename.
fn sd_card(name: &str) -> (PathBuf, Vec<u8>) {
    let path = temp_path(name, "img");
    let image = card_image();
    std::fs::write(&path, &image).unwrap();
    (path, image)
}

fn inode(path: &PathBuf) -> u64 {
    std::fs::metadata(path).unwrap().ino()
}

fn untouched(path: &PathBuf, image: &[u8], inode_before: u64) -> bool {
    let meta = std::fs::metadata(path).unwrap();
    meta.ino() == inode_before && std::fs::read(path).unwrap() == image
}

// The card image as the guest of WRITE_CARD leaves it.
fn written(image: &[u8]) -> Vec<u8> {
    let mut image = image.to_vec();
    image[512..1024].fill(0x5A);
    image
}

fn written_back(path: &PathBuf, image: &[u8], inode_before: u64) -> bool {
    inode(path) != inode_before && std::fs::read(path).unwrap() == written(image)
}

#[test]
fn the_sd_file_is_written_back_whichever_way_the_run_ends() {
    let cases: [(&str, &[&str]); 3] = [
        ("sd-limit", &["--run", "1000000"]),
        ("sd-console", &["--console", "--run", "1000000"]),
        ("sd-stop-at", &["--stop-at", WRITE_CARD_END_ADDRESS]),
    ];
    for (name, args) in cases {
        let (card, image) = sd_card(name);
        let before = inode(&card);
        let mut all = vec!["--sd", card.to_str().unwrap()];
        all.extend_from_slice(args);

        let output = run_program(name, &write_card(SPIN), &all);

        assert!(output.status.success(), "{name}: {}", stderr(&output));
        assert!(written_back(&card, &image, before), "{name}");
        std::fs::remove_file(&card).unwrap();
    }
}

#[test]
fn the_sd_file_is_written_back_when_the_guest_faults() {
    for (name, args) in [
        ("sd-fault-batch", &["--run", "1000000"][..]),
        ("sd-fault-live", &["--console"][..]),
    ] {
        let (card, image) = sd_card(name);
        let before = inode(&card);
        let mut all = vec!["--sd", card.to_str().unwrap()];
        all.extend_from_slice(args);

        let output = run_program(name, &write_card(0xDE00), &all);

        assert_eq!(output.status.code(), Some(1), "{name}");
        assert!(written_back(&card, &image, before), "{name}");
        std::fs::remove_file(&card).unwrap();
    }
}

#[test]
fn a_card_the_guest_did_not_write_to_leaves_the_sd_file_alone() {
    let cases: [(&str, &[u16], &[&str]); 5] = [
        ("sd-clean-limit", &THREE, &["--run", "10"]),
        ("sd-clean-console", &THREE, &["--console", "--run", "10"]),
        ("sd-clean-stop-at", &THREE, &["--stop-at", "0x08000106"]),
        ("sd-clean-fault", &UNDEFINED, &["--run", "100"]),
        ("sd-clean-fault-live", &UNDEFINED, &["--console"]),
    ];
    for (name, code, args) in cases {
        let (card, image) = sd_card(name);
        let before = inode(&card);
        let mut all = vec!["--sd", card.to_str().unwrap()];
        all.extend_from_slice(args);

        run_program(name, code, &all);

        assert!(untouched(&card, &image, before), "{name}");
        std::fs::remove_file(&card).unwrap();
    }
}

#[test]
fn changes_made_to_the_sd_file_during_a_clean_run_are_kept() {
    // The run is long enough to change the file under it; a card never written to is not
    // written back over it.
    let (card, _) = sd_card("sd-host-edit");
    let path = temp_program("sd-host-edit", &ECHO);
    let mut child = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .args(["--firmware", path.to_str().unwrap(), "--console", "--sd"])
        .arg(&card)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to run cmm2-emulator");
    // The guest says "ready" once the card is in and it is running.
    let mut out = child.stdout.take().unwrap();
    let mut seen = Vec::new();
    let mut byte = [0];
    while !seen.ends_with(b"ready\r\n") && out.read(&mut byte).is_ok_and(|n| n == 1) {
        seen.push(byte[0]);
    }
    std::fs::write(&card, vec![0xEE; 2 * 512]).unwrap();
    child.stdin.as_mut().unwrap().write_all(b"\x1D").unwrap();
    assert!(child.wait().unwrap().success());

    assert_eq!(std::fs::read(&card).unwrap(), vec![0xEE; 2 * 512]);
    std::fs::remove_file(&card).unwrap();
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn saving_the_sd_file_leaves_no_temporary_file() {
    let dir = temp_path("sd-temp", "d");
    std::fs::create_dir(&dir).unwrap();
    let card = dir.join("card.img");
    std::fs::write(&card, card_image()).unwrap();

    run_program(
        "sd-temp",
        &write_card(SPIN),
        &["--run", "1000000", "--sd", card.to_str().unwrap()],
    );

    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_missing_sd_file_is_an_error_and_is_not_made() {
    let card = temp_path("sd-missing", "img");
    let _ = std::fs::remove_file(&card);

    let output = run_program(
        "sd-missing",
        &THREE,
        &["--run", "10", "--sd", card.to_str().unwrap()],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        stderr(&output),
        format!(
            "cmm2-emulator: {}: No such file or directory (os error 2)\n",
            card.display()
        )
    );
    assert!(!card.exists());
}

#[test]
fn an_sd_file_that_is_not_whole_blocks_is_an_error_and_is_left_alone() {
    for (name, bytes) in [("sd-ragged", vec![7; 1000]), ("sd-empty", Vec::new())] {
        let card = temp_path(name, "img");
        std::fs::write(&card, &bytes).unwrap();

        let output = run_program(
            name,
            &THREE,
            &["--run", "10", "--sd", card.to_str().unwrap()],
        );

        assert_eq!(output.status.code(), Some(1), "{name}");
        assert!(output.stdout.is_empty(), "{name}");
        assert_eq!(
            stderr(&output),
            format!(
                "cmm2-emulator: {}: {} bytes is not a whole number of 512-byte blocks\n",
                card.display(),
                bytes.len()
            ),
            "{name}"
        );
        assert_eq!(std::fs::read(&card).unwrap(), bytes, "{name}");
        std::fs::remove_file(&card).unwrap();
    }
}

// The tests that read /proc look at what the kernel says about the emulator's own process and
// threads, and are for Linux.

// The signals the process has handlers for, from /proc, as a bit for each signal number - 1.
#[cfg(target_os = "linux")]
fn proc_mask(pid: u32, thread: u32, field: &str) -> u64 {
    let status = std::fs::read_to_string(format!("/proc/{pid}/task/{thread}/status")).unwrap();
    let line = status
        .lines()
        .find_map(|line| line.strip_prefix(field))
        .unwrap();
    u64::from_str_radix(line.trim(), 16).unwrap()
}

// Waits until the emulator has its handlers in, which is the first thing it does, so that a
// signal sent after this is the one it is waiting for and not one that ends it.
#[cfg(target_os = "linux")]
fn wait_until_catching(child: &std::process::Child) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while proc_mask(child.id(), child.id(), "SigCgt:") & 1 << (libc::SIGTERM - 1) == 0 {
        assert!(
            Instant::now() < deadline,
            "the emulator never took its handlers"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

// Waits until the emulator is blocked, which an unpaced run, always at work, is not: its state in
// /proc is sleeping for several looks in a row. That is it waiting in a write to a pipe that
// is full.
#[cfg(target_os = "linux")]
fn wait_until_blocked(child: &std::process::Child) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut sleeping = 0;
    while sleeping < 10 {
        let stat = std::fs::read_to_string(format!("/proc/{}/stat", child.id())).unwrap();
        // The state is the field after the command, which is in brackets and may hold anything.
        let state = stat.rsplit_once(") ").unwrap().1.chars().next();
        sleeping = if state == Some('S') { sleeping + 1 } else { 0 };
        assert!(Instant::now() < deadline, "the emulator never blocked");
        std::thread::sleep(Duration::from_millis(20));
    }
}

// Starts a live run of WRITE_CARD with the flash file and the card, and returns it once it has
// printed "done\n", so that the guest is running and has written the card.
fn start_writing_run(name: &str, flash: &PathBuf, card: &PathBuf) -> std::process::Child {
    let path = temp_program(name, &write_card(SPIN));
    let mut child = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .args(["--firmware", path.to_str().unwrap(), "--console", "--flash"])
        .arg(flash)
        .arg("--sd")
        .arg(card)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to run cmm2-emulator");
    let mut out = child.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let mut byte = [0];
        while out.read(&mut byte).is_ok_and(|n| n == 1) {
            buffer.push(byte[0]);
            if buffer.ends_with(b"done\n") {
                let _ = sender.send(());
                break;
            }
        }
        // Keep draining, so the emulator is never blocked on a full pipe.
        let _ = std::io::copy(&mut out, &mut std::io::sink());
    });
    if receiver.recv_timeout(Duration::from_secs(30)).is_err() {
        child.kill().unwrap();
        panic!("the guest never said it was done");
    }
    std::fs::remove_file(&path).unwrap();
    child
}

fn wait_for(child: &mut std::process::Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            panic!("the signal did not end the run");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_sd_file_is_written_back_when_the_quit_key_ends_the_run() {
    let flash = flash_path("sd-quit");
    let _ = std::fs::remove_file(&flash);
    let (card, image) = sd_card("sd-quit");
    let before = inode(&card);
    let mut child = start_writing_run("sd-quit", &flash, &card);

    child.stdin.as_mut().unwrap().write_all(b"\x1D").unwrap();
    let status = wait_for(&mut child);

    assert!(status.success());
    assert!(written_back(&card, &image, before));
    std::fs::remove_file(&flash).unwrap();
    std::fs::remove_file(&card).unwrap();
}

#[test]
fn a_signal_ends_a_live_run_and_the_files_are_written() {
    for (name, signal) in [
        ("signal-int", libc::SIGINT),
        ("signal-term", libc::SIGTERM),
        ("signal-hup", libc::SIGHUP),
    ] {
        let flash = flash_path(name);
        let _ = std::fs::remove_file(&flash);
        let (card, image) = sd_card(name);
        let before = inode(&card);
        let mut child = start_writing_run(name, &flash, &card);

        // SAFETY: kill only sends the signal to the child, which is still running.
        assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
        let status = wait_for(&mut child);

        // The run ends by the signal itself, once its files are written.
        assert_eq!(status.signal(), Some(signal), "{name}: {status:?}");
        assert_eq!(
            std::fs::read(&flash).unwrap().len(),
            FLASH_FILE_SIZE,
            "{name}"
        );
        assert!(written_back(&card, &image, before), "{name}");
        std::fs::remove_file(&flash).unwrap();
        std::fs::remove_file(&card).unwrap();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_signal_ends_a_batch_run_with_no_limit_and_the_flash_is_written() {
    for (name, signal) in [
        ("signal-batch-term", libc::SIGTERM),
        ("signal-batch-int", libc::SIGINT),
    ] {
        let flash = flash_path(name);
        let _ = std::fs::remove_file(&flash);
        let path = temp_program(name, &count_in_flash(SPIN));
        let trace = temp_path(name, "trace");
        // A stop address the guest never reaches, and no limit: only the signal ends it.
        let mut child = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
            .args([
                "--firmware",
                path.to_str().unwrap(),
                "--stop-at",
                "0x08000FF0",
            ])
            .arg("--flash")
            .arg(&flash)
            .arg("--trace")
            .arg(&trace)
            .stdout(Stdio::piped())
            .spawn()
            .expect("failed to run cmm2-emulator");
        // The trace file has something in it once the run is under way. A signal before that
        // would end the process without a save, as it has nothing to save.
        let deadline = Instant::now() + Duration::from_secs(30);
        while std::fs::metadata(&trace).map_or(0, |meta| meta.len()) == 0 {
            assert!(Instant::now() < deadline, "the run never began");
            std::thread::sleep(Duration::from_millis(10));
        }

        // SAFETY: kill only sends the signal to the child, which is still running.
        assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
        let status = wait_for(&mut child);
        let mut summary = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut summary)
            .unwrap();

        assert_eq!(status.signal(), Some(signal), "{name}: {status:?}");
        assert!(
            summary.starts_with("stopped: interrupted by a signal\n"),
            "{summary}"
        );
        assert_eq!(options_word(&std::fs::read(&flash).unwrap()), 0, "{name}");
        std::fs::remove_file(&flash).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(&trace).unwrap();
    }
}

#[test]
fn a_trace_that_cannot_be_written_does_not_hide_a_fault() {
    // /dev/full takes the open and refuses every write.
    let live = run_program(
        "trace-full-live",
        &UNDEFINED,
        &["--console", "--trace", "/dev/full"],
    );
    let lines: Vec<String> = stderr(&live).lines().map(str::to_string).collect();
    assert_eq!(live.status.code(), Some(1));
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(!lines[0].contains("writing the trace"), "{lines:?}");
    assert!(
        lines[1].starts_with("cmm2-emulator: writing the trace: "),
        "{lines:?}"
    );

    // A batch run says the fault in its summary, and the trace error after it.
    let batch = run_program(
        "trace-full-batch",
        &UNDEFINED,
        &["--run", "100", "--trace", "/dev/full"],
    );
    assert_eq!(batch.status.code(), Some(1));
    assert!(stdout(&batch).contains("stopped: "), "{}", stdout(&batch));
    assert!(
        stderr(&batch).starts_with("cmm2-emulator: writing the trace: "),
        "{}",
        stderr(&batch)
    );
}

#[test]
fn the_errors_of_the_trace_and_the_screenshot_are_both_said() {
    // Neither hides the other: the trace cannot be written, and there is no screen to take a
    // picture of.
    let output = run_program(
        "both-errors",
        &THREE,
        &[
            "--run",
            "1000",
            "--trace",
            "/dev/full",
            "--screenshot",
            "/nonexistent/s.ppm",
        ],
    );

    assert_eq!(output.status.code(), Some(1));
    let lines: Vec<String> = stderr(&output).lines().map(str::to_string).collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].starts_with("cmm2-emulator: writing the trace: "),
        "{lines:?}"
    );
    assert!(
        lines[1].starts_with("cmm2-emulator: no screenshot: "),
        "{lines:?}"
    );
}

// A save that fails only at the end, which the checks at the start cannot see: the directory
// of the flash file is made unwritable once the run is under way, and then a signal ends it.
// With the screenshot that cannot be taken, that is two errors, said in the order they came.
#[cfg(target_os = "linux")]
#[test]
fn a_save_that_fails_at_the_end_is_said_after_the_screenshot() {
    if root() {
        return;
    }
    let dir = temp_path("late-save", "d");
    std::fs::create_dir(&dir).unwrap();
    let flash = dir.join("board.flash");
    let trace = temp_path("late-save", "trace");
    let path = temp_program("late-save", &THREE);
    let mut child = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .args([
            "--firmware",
            path.to_str().unwrap(),
            "--stop-at",
            "0x08000FF0",
        ])
        .arg("--flash")
        .arg(&flash)
        .args(["--screenshot", "/nonexistent/s.ppm"])
        .arg("--trace")
        .arg(&trace)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to run cmm2-emulator");
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::fs::metadata(&trace).map_or(0, |meta| meta.len()) == 0 {
        assert!(Instant::now() < deadline, "the run never began");
        std::thread::sleep(Duration::from_millis(10));
    }
    mode(&dir, 0o555);

    // SAFETY: kill only sends the signal to the child, which is still running.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let status = wait_for(&mut child);
    mode(&dir, 0o755);
    let mut said = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut said)
        .unwrap();

    assert_eq!(status.signal(), Some(libc::SIGTERM));
    let lines: Vec<&str> = said.lines().collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].starts_with("cmm2-emulator: no screenshot: "),
        "{lines:?}"
    );
    assert_eq!(
        lines[1],
        format!(
            "cmm2-emulator: {}: Permission denied (os error 13)",
            flash.display()
        )
    );
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_file(&trace).unwrap();
    std::fs::remove_file(&path).unwrap();
}

// Starts the emulator on `code` with `args`, with SIGHUP ignored as nohup leaves it.
fn spawn_with_sighup_ignored(name: &str, code: &[u16], args: &[&str]) -> std::process::Child {
    let path = temp_program(name, code);
    let mut command = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"));
    command
        .args(["--firmware", path.to_str().unwrap()])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null());
    // SAFETY: signal with SIG_IGN is async-signal-safe and runs between fork and exec.
    unsafe {
        command.pre_exec(|| {
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
            Ok(())
        });
    }
    command.spawn().expect("failed to run cmm2-emulator")
}

#[cfg(target_os = "linux")]
#[test]
fn a_signal_that_was_ignored_when_the_emulator_started_is_still_ignored() {
    let mut child = spawn_with_sighup_ignored("signal-nohup", &THREE, &["--console"]);
    wait_until_catching(&child);
    std::thread::sleep(Duration::from_millis(500));

    // SAFETY: kill only sends the signal to the child, which is still running.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGHUP) }, 0);
    std::thread::sleep(Duration::from_millis(500));
    assert!(child.try_wait().unwrap().is_none(), "SIGHUP ended the run");

    // The others are not ignored, so they still end it.
    // SAFETY: as above.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    assert_eq!(wait_for(&mut child).signal(), Some(libc::SIGTERM));
    std::fs::remove_file(temp_path("signal-nohup", "bin")).unwrap();
}

// Sets UE and TE in USART1_CR1 and then sends 'x' for ever. The address of USART1 is the
// literal at +16.
const SPAM: [u16; 10] = [
    0x4803, 0x2109, 0x6001, 0x2278, 0x6282, 0xE7FD, 0x0000, 0x0000, 0x1000, 0x4001,
];

// A pipe filled to the last byte, so that the next write to it blocks with nothing written.
fn full_pipe() -> (std::io::PipeReader, std::io::PipeWriter) {
    use std::os::fd::AsRawFd;
    let (reader, mut writer) = std::io::pipe().unwrap();
    let fd = writer.as_raw_fd();
    // SAFETY: fcntl on a descriptor this function owns; the flag is shared with the other end
    // of the dup that the child gets, so it is put back before it goes.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        let mut chunk = 4096;
        while chunk > 0 {
            match writer.write(&vec![b'.'; chunk]) {
                Ok(_) => {}
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => chunk /= 2,
                Err(err) => panic!("{err}"),
            }
        }
        libc::fcntl(fd, libc::F_SETFL, flags);
    }
    (reader, writer)
}

#[cfg(target_os = "linux")]
#[test]
fn a_run_blocked_writing_to_a_pipe_nobody_reads_ends_on_a_signal() {
    let flash = flash_path("signal-blocked");
    let _ = std::fs::remove_file(&flash);
    let path = temp_program("signal-blocked", &SPAM);
    // Never read from: the pipe is full, and the emulator blocks in its first write.
    let (_reader, writer) = full_pipe();
    let mut child = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .args(["--firmware", path.to_str().unwrap()])
        .args(["--console", "--unpaced", "--flash"])
        .arg(&flash)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(writer))
        .spawn()
        .expect("failed to run cmm2-emulator");
    wait_until_blocked(&child);

    // SAFETY: kill only sends the signal to the child, which is still running.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let status = wait_for(&mut child);

    assert_eq!(status.signal(), Some(libc::SIGTERM));
    assert_eq!(std::fs::read(&flash).unwrap().len(), FLASH_FILE_SIZE);
    std::fs::remove_file(&flash).unwrap();
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn a_read_only_sd_file_is_a_write_protected_card_and_is_not_written_back() {
    if root() {
        return;
    }
    let (card, image) = sd_card("sd-readonly");
    let before = inode(&card);
    std::fs::set_permissions(&card, std::os::unix::fs::PermissionsExt::from_mode(0o444)).unwrap();

    // The guest writes anyway, as one that ignores the switch would.
    let output = run_program(
        "sd-readonly",
        &write_card(SPIN),
        &["--run", "1000000", "--sd", card.to_str().unwrap()],
    );

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output).ends_with("the card is write-protected because the file is read-only\n")
    );
    assert!(untouched(&card, &image, before));
    std::fs::set_permissions(&card, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    std::fs::remove_file(&card).unwrap();
}

#[test]
fn an_sd_file_under_512_kb_is_an_error_and_is_left_alone() {
    let card = temp_path("sd-tiny", "img");
    std::fs::write(&card, vec![7; 4 * 512]).unwrap();

    let output = run_program(
        "sd-tiny",
        &THREE,
        &["--run", "10", "--sd", card.to_str().unwrap()],
    );

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!(
            "cmm2-emulator: {}: 2048 bytes is smaller than the 524288 bytes (512 KB) of the smallest card\n",
            card.display()
        )
    );
    assert_eq!(std::fs::read(&card).unwrap(), vec![7; 4 * 512]);
    std::fs::remove_file(&card).unwrap();
}

#[test]
fn a_read_only_flash_file_in_a_writable_directory_is_saved_as_the_save_replaces_it() {
    let flash = flash_path("flash-readonly");
    let _ = std::fs::remove_file(&flash);
    run_program(
        "flash-readonly-1",
        &THREE,
        &["--run", "10", "--flash", flash.to_str().unwrap()],
    );
    mode(&flash, 0o444);
    let before = inode(&flash);

    let output = run_program(
        "flash-readonly-2",
        &THREE,
        &["--run", "10", "--flash", flash.to_str().unwrap()],
    );

    assert!(output.status.success(), "{}", stderr(&output));
    assert_ne!(inode(&flash), before, "the file was replaced");
    assert_eq!(std::fs::metadata(&flash).unwrap().mode() & 0o777, 0o444);
    mode(&flash, 0o644);
    std::fs::remove_file(&flash).unwrap();
}

fn mode(path: &PathBuf, mode: u32) {
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(mode)).unwrap();
}

// As root a mode bit stops nothing, so the tests that depend on one are not run.
fn root() -> bool {
    // SAFETY: geteuid has no arguments and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

#[test]
fn a_flash_file_in_a_directory_that_cannot_be_written_is_an_error_at_the_start() {
    if root() {
        return;
    }
    let dir = temp_path("flash-dir-ro", "d");
    std::fs::create_dir(&dir).unwrap();
    let flash = dir.join("board.flash");
    run_program(
        "flash-dir-ro-1",
        &THREE,
        &["--run", "10", "--flash", flash.to_str().unwrap()],
    );
    mode(&dir, 0o555);

    let output = run_program(
        "flash-dir-ro-2",
        &THREE,
        &["--run", "10", "--flash", flash.to_str().unwrap()],
    );

    mode(&dir, 0o755);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        stderr(&output),
        format!(
            "cmm2-emulator: {}: its directory is not writable, so the run could not save its flash and clock\n",
            flash.display()
        )
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

// Runs the emulator with `args` in a user and mount namespace in which `dir` is a read-only
// mount (the file system is read-only to it, whatever the modes). None, with a note, when the
// machine does not let a user make one.
fn run_with_read_only_mount(dir: &Path, firmware: &Path, args: &[&str]) -> Option<Output> {
    let script =
        "mount --bind \"$1\" \"$1\" && mount -o remount,bind,ro \"$1\" && shift && exec \"$@\"";
    let output = Command::new("unshare")
        .args(["-rm", "sh", "-c", script, "sh"])
        .arg(dir)
        .arg(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .args(["--firmware", firmware.to_str().unwrap()])
        .args(args)
        .env("SDL_VIDEODRIVER", "dummy")
        .output();
    match output {
        // unshare, or the mount in it, was refused: sh says which on stderr.
        Ok(output) if !output.status.success() && stderr(&output).contains("unshare:") => {
            eprintln!(
                "skipped: no user and mount namespaces here: {}",
                stderr(&output)
            );
            None
        }
        Ok(output) if output.status.code() == Some(32) => {
            eprintln!(
                "skipped: no mounts in a user namespace here: {}",
                stderr(&output)
            );
            None
        }
        Ok(output) => Some(output),
        Err(err) => {
            eprintln!("skipped: no unshare here: {err}");
            None
        }
    }
}

#[test]
fn a_flash_file_on_a_read_only_file_system_is_an_error_at_the_start() {
    let dir = temp_path("flash-ro-mount", "d");
    std::fs::create_dir(&dir).unwrap();
    let flash = dir.join("board.flash");
    run_program(
        "flash-ro-mount-1",
        &THREE,
        &["--run", "10", "--flash", flash.to_str().unwrap()],
    );
    mode(&flash, 0o664);
    let firmware = temp_program("flash-ro-mount-2", &THREE);

    let output = run_with_read_only_mount(
        &dir,
        &firmware,
        &["--run", "10", "--flash", flash.to_str().unwrap()],
    );

    if let Some(output) = output {
        assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
        assert_eq!(
            stderr(&output),
            format!(
                "cmm2-emulator: {}: its file system is read-only, so the run could not save its flash and clock\n",
                flash.display()
            )
        );
    }
    std::fs::remove_file(&firmware).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_sd_file_on_a_read_only_file_system_is_a_write_protected_card_that_is_not_written_back() {
    let dir = temp_path("sd-ro-mount", "d");
    std::fs::create_dir(&dir).unwrap();
    let card = dir.join("card.img");
    std::fs::write(&card, card_image()).unwrap();
    mode(&card, 0o664);
    let firmware = temp_program("sd-ro-mount", &write_card(SPIN));

    // The guest writes anyway, as one that ignores the switch would.
    let output = run_with_read_only_mount(
        &dir,
        &firmware,
        &["--run", "1000000", "--sd", card.to_str().unwrap()],
    );

    if let Some(output) = output {
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(
            stderr(&output),
            format!(
                "cmm2-emulator: {}: the card is write-protected because its file system is read-only\n",
                card.display()
            )
        );
        assert_eq!(std::fs::read(&card).unwrap(), card_image());
    }
    std::fs::remove_file(&firmware).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_sd_file_in_a_directory_that_cannot_be_written_is_a_write_protected_card() {
    if root() {
        return;
    }
    let dir = temp_path("sd-dir-ro", "d");
    std::fs::create_dir(&dir).unwrap();
    let card = dir.join("card.img");
    std::fs::write(&card, card_image()).unwrap();
    mode(&dir, 0o555);

    let output = run_program(
        "sd-dir-ro",
        &write_card(SPIN),
        &["--run", "1000000", "--sd", card.to_str().unwrap()],
    );

    mode(&dir, 0o755);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stderr(&output),
        format!(
            "cmm2-emulator: {}: the card is write-protected because its directory is not writable\n",
            card.display()
        )
    );
    assert_eq!(std::fs::read(&card).unwrap(), card_image());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_first_run_through_a_dangling_link_makes_the_file_it_points_to() {
    let dir = temp_path("flash-dangling", "d");
    std::fs::create_dir(&dir).unwrap();
    let link = dir.join("link.flash");
    std::os::unix::fs::symlink("real.flash", &link).unwrap();

    let output = run_program(
        "flash-dangling",
        &THREE,
        &["--run", "10", "--flash", link.to_str().unwrap()],
    );

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
    assert_eq!(
        std::fs::read(dir.join("real.flash")).unwrap().len(),
        FLASH_FILE_SIZE
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn a_batch_run_blocked_writing_its_trace_to_a_pipe_nobody_reads_ends_on_a_signal() {
    let flash = flash_path("signal-trace-blocked");
    let _ = std::fs::remove_file(&flash);
    let path = temp_program("signal-trace-blocked", &SPAM);
    // The trace of the peripherals goes to stdout, which is full and never read.
    let (_reader, writer) = full_pipe();
    let mut child = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .args(["--firmware", path.to_str().unwrap()])
        .args(["--trace-mmio", "--stop-at", "0x08000FF0", "--flash"])
        .arg(&flash)
        .stdout(Stdio::from(writer))
        .spawn()
        .expect("failed to run cmm2-emulator");
    wait_until_blocked(&child);

    // SAFETY: kill only sends the signal to the child, which is still running.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let status = wait_for(&mut child);

    assert_eq!(status.signal(), Some(libc::SIGTERM));
    assert_eq!(std::fs::read(&flash).unwrap().len(), FLASH_FILE_SIZE);
    std::fs::remove_file(&flash).unwrap();
    std::fs::remove_file(&path).unwrap();
}

// The local mode flags of the terminal on `fd`.
fn lflag(fd: libc::c_int) -> libc::tcflag_t {
    // SAFETY: tcgetattr fills a plain termios.
    unsafe {
        let mut termios: libc::termios = std::mem::zeroed();
        assert_eq!(libc::tcgetattr(fd, &mut termios), 0);
        termios.c_lflag
    }
}

#[test]
fn two_signals_in_a_row_leave_the_terminal_as_it_was() {
    use std::os::fd::{FromRawFd, OwnedFd};
    let (mut master, mut slave) = (0, 0);
    // SAFETY: openpty fills in two descriptors.
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(opened, 0);
    let cooked = lflag(slave);
    let path = temp_program("signal-pty", &ECHO);
    // SAFETY: dup of a descriptor this test owns; the new one is handed to the child.
    let stdin = unsafe { OwnedFd::from_raw_fd(libc::dup(slave)) };
    let mut child = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .args(["--firmware", path.to_str().unwrap(), "--console"])
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::null())
        .spawn()
        .expect("failed to run cmm2-emulator");
    let deadline = Instant::now() + Duration::from_secs(20);
    while lflag(slave) == cooked {
        assert!(Instant::now() < deadline, "the terminal never went raw");
        std::thread::sleep(Duration::from_millis(5));
    }

    for _ in 0..2 {
        // SAFETY: kill only sends the signal to the child, which is still running.
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    }
    let status = wait_for(&mut child);

    assert_eq!(status.signal(), Some(libc::SIGTERM));
    assert_eq!(lflag(slave), cooked, "the terminal is still raw");
    // SAFETY: closing the two descriptors made above.
    unsafe {
        libc::close(master);
        libc::close(slave);
    }
    std::fs::remove_file(&path).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn the_threads_the_run_makes_have_the_signals_blocked_and_the_main_one_takes_them() {
    let flash = flash_path("signal-threads");
    let _ = std::fs::remove_file(&flash);
    let path = temp_program("signal-threads", &ECHO);
    let mut child = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .args([
            "--firmware",
            path.to_str().unwrap(),
            "--console",
            "--window",
        ])
        .arg("--flash")
        .arg(&flash)
        .env("SDL_VIDEODRIVER", "dummy")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to run cmm2-emulator");
    // The guest says "ready" once everything is made and the run has started.
    let mut out = child.stdout.take().unwrap();
    let mut seen = Vec::new();
    let mut byte = [0];
    while !seen.ends_with(b"ready\r\n") && out.read(&mut byte).is_ok_and(|n| n == 1) {
        seen.push(byte[0]);
    }
    let pid = child.id();
    let ending: u64 = 1 << (libc::SIGINT - 1) | 1 << (libc::SIGTERM - 1) | 1 << (libc::SIGHUP - 1);
    let threads: Vec<u32> = std::fs::read_dir(format!("/proc/{pid}/task"))
        .unwrap()
        .map(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_str()
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect();
    assert!(threads.len() > 1, "{threads:?}: the stdin thread is one");
    for thread in threads {
        let blocked = proc_mask(pid, thread, "SigBlk:") & ending;
        let expected = if thread == pid { 0 } else { ending };
        assert_eq!(blocked, expected, "thread {thread}");
    }

    // SAFETY: kill only sends the signal to the child, which is still running.
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGTERM) }, 0);
    let status = wait_for(&mut child);

    assert_eq!(status.signal(), Some(libc::SIGTERM));
    assert_eq!(std::fs::read(&flash).unwrap().len(), FLASH_FILE_SIZE);
    std::fs::remove_file(&flash).unwrap();
    std::fs::remove_file(&path).unwrap();
}

fn make_fifo(path: &Path) {
    let name = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    // SAFETY: mkfifo with a NUL-terminated name that lives through the call.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o644) }, 0);
}

#[cfg(target_os = "linux")]
#[test]
fn a_signal_ends_a_load_that_is_stuck_at_once() {
    // The firmware is a FIFO with no writer, so the load waits in its open for ever. Nothing
    // has been run, so there is nothing to save: the signal ends the process, by itself.
    for (name, signal) in [("stuck-int", libc::SIGINT), ("stuck-term", libc::SIGTERM)] {
        let fifo = temp_path(name, "fifo");
        make_fifo(&fifo);
        let mut child = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
            .args(["--firmware", fifo.to_str().unwrap(), "--run", "10"])
            .spawn()
            .expect("failed to run cmm2-emulator");
        wait_until_catching(&child);
        std::thread::sleep(Duration::from_millis(200));
        assert!(child.try_wait().unwrap().is_none(), "{name}: not stuck");

        // SAFETY: kill only sends the signal to the child, which is still running.
        assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
        let status = wait_for(&mut child);

        assert_eq!(status.signal(), Some(signal), "{name}");
        std::fs::remove_file(&fifo).unwrap();
    }
}

#[test]
fn what_is_not_a_regular_file_is_refused_for_both_files_and_is_never_opened() {
    let dir = temp_path("not-regular", "d");
    std::fs::create_dir(&dir).unwrap();
    let fifo = dir.join("pipe");
    make_fifo(&fifo);

    // The FIFO would hang the read for ever, so a refusal that returns is the test.
    for path in [fifo.as_path(), dir.as_path(), Path::new("/dev/null")] {
        for option in ["--sd", "--flash"] {
            let output = run_program(
                "not-regular",
                &THREE,
                &["--run", "10", option, path.to_str().unwrap()],
            );
            assert_eq!(output.status.code(), Some(1), "{option} {}", path.display());
            assert!(output.stdout.is_empty());
            assert_eq!(
                stderr(&output),
                format!("cmm2-emulator: {}: not a regular file\n", path.display()),
                "{option}"
            );
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_name_that_ends_in_a_slash_or_dots_is_refused_at_the_start_for_both_files() {
    let dir = temp_path("slash", "d");
    std::fs::create_dir(&dir).unwrap();
    for tail in ["flash/", "flash/.", "flash/..", "."] {
        for option in ["--flash", "--sd"] {
            let path = format!("{}/{tail}", dir.display());
            let output = run_program("slash", &THREE, &["--run", "10", option, &path]);
            assert_eq!(output.status.code(), Some(1), "{option} {path}");
            assert_eq!(
                stderr(&output),
                format!(
                    "cmm2-emulator: {path}: not a file name (it is empty, or ends in a slash, or in . or ..)\n"
                ),
                "{option}"
            );
        }
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_file_name_as_long_as_there_can_be_is_saved_and_a_longer_one_is_refused_at_the_start() {
    let dir = temp_path("long-name", "d");
    std::fs::create_dir(&dir).unwrap();
    let longest = dir.join("f".repeat(255));
    let output = run_program(
        "long-name-1",
        &THREE,
        &["--run", "10", "--flash", longest.to_str().unwrap()],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    // The card too: its name plus a suffix would not have fitted.
    let card = dir.join("c".repeat(255));
    std::fs::write(&card, card_image()).unwrap();
    let output = run_program(
        "long-name-2",
        &write_card(SPIN),
        &["--run", "1000000", "--sd", card.to_str().unwrap()],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(std::fs::read(&card).unwrap()[512..1024], [0x5A; 512]);
    // Nothing else is left beside them.
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);

    let too_long = dir.join("f".repeat(256));
    let output = run_program(
        "long-name-3",
        &THREE,
        &["--run", "10", "--flash", too_long.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!(
            "cmm2-emulator: {}: a name on its path is longer than the file system allows, so the run could not save its flash and clock\n",
            too_long.display()
        )
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

// What a run does with a trace that goes to a pipe or a FIFO that nobody reads from: the first
// signal ends it, and it saves.
#[cfg(target_os = "linux")]
#[test]
fn a_run_blocked_writing_its_trace_file_to_a_stalled_reader_ends_on_a_signal() {
    use std::os::unix::fs::OpenOptionsExt;
    // /dev/stdout is the full pipe the run is given as its stdout, and a FIFO is one with a
    // reader that holds it open and never reads. Each is a file the trace is written to.
    for (name, fifo) in [("trace-stdout", false), ("trace-fifo", true)] {
        let flash = flash_path(name);
        let _ = std::fs::remove_file(&flash);
        let path = temp_program(name, &THREE);
        let (_reader, writer) = full_pipe();
        let trace_fifo = temp_path(name, "fifo");
        let mut _held = None;
        let trace = if fifo {
            make_fifo(&trace_fifo);
            // A reader that never reads, opened first so that the run's open does not wait.
            _held = Some(
                std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&trace_fifo)
                    .unwrap(),
            );
            trace_fifo.to_str().unwrap().to_string()
        } else {
            "/dev/stdout".to_string()
        };
        let mut child = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
            .args(["--firmware", path.to_str().unwrap(), "--run", "100000000"])
            .args(["--trace", &trace, "--flash"])
            .arg(&flash)
            .stdout(Stdio::from(writer))
            .spawn()
            .expect("failed to run cmm2-emulator");
        wait_until_blocked(&child);

        // SAFETY: kill only sends the signal to the child, which is still running.
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
        let status = wait_for(&mut child);

        assert_eq!(status.signal(), Some(libc::SIGTERM), "{name}");
        assert_eq!(
            std::fs::read(&flash).unwrap().len(),
            FLASH_FILE_SIZE,
            "{name}"
        );
        std::fs::remove_file(&flash).unwrap();
        std::fs::remove_file(&path).unwrap();
        if fifo {
            std::fs::remove_file(&trace_fifo).unwrap();
        }
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
fn a_link_to_a_name_that_is_not_a_file_name_is_refused_at_the_start() {
    let dir = temp_path("link-slash", "d");
    std::fs::create_dir(&dir).unwrap();
    let link = dir.join("lnk");
    std::os::unix::fs::symlink("newfile/", &link).unwrap();

    let flash = run_program(
        "link-slash-1",
        &THREE,
        &["--run", "10", "--flash", link.to_str().unwrap()],
    );
    assert_eq!(flash.status.code(), Some(1));
    assert_eq!(
        stderr(&flash),
        format!(
            "cmm2-emulator: {}: it leads to {}/newfile/, which cannot be the name of a file, so the run could not save its flash and clock\n",
            link.display(),
            dir.display()
        )
    );
    let card = run_program(
        "link-slash-2",
        &THREE,
        &["--run", "10", "--sd", link.to_str().unwrap()],
    );
    assert_eq!(card.status.code(), Some(1));
    assert!(stderr(&card).starts_with(&format!("cmm2-emulator: {}: ", link.display())));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

// Writes to AIRCR (the literals at +8 and +12) to reset the system, over and over: each reset is
// a line on standard error.
const RESET_FOR_EVER: [u16; 8] = [
    0x4801, 0x4902, 0x6001, 0xE7FD, 0xED0C, 0xE000, 0x0004, 0x05FA,
];

#[cfg(target_os = "linux")]
#[test]
fn a_run_blocked_writing_to_a_standard_error_nobody_reads_ends_on_a_signal() {
    let flash = flash_path("signal-stderr");
    let _ = std::fs::remove_file(&flash);
    let path = temp_program("signal-stderr", &RESET_FOR_EVER);
    // The pipe is full and never read, and the guest is a line of it for each reset.
    let (_reader, writer) = full_pipe();
    let mut child = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .args(["--firmware", path.to_str().unwrap()])
        .args(["--console", "--unpaced", "--flash"])
        .arg(&flash)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(writer))
        .spawn()
        .expect("failed to run cmm2-emulator");
    wait_until_blocked(&child);

    // SAFETY: kill only sends the signal to the child, which is still running.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let status = wait_for(&mut child);

    assert_eq!(status.signal(), Some(libc::SIGTERM));
    assert_eq!(std::fs::read(&flash).unwrap().len(), FLASH_FILE_SIZE);
    std::fs::remove_file(&flash).unwrap();
    std::fs::remove_file(&path).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn a_run_blocked_writing_to_a_terminal_nobody_reads_ends_on_a_signal() {
    use std::os::fd::{FromRawFd, OwnedFd};
    let (mut master, mut slave) = (0, 0);
    // SAFETY: openpty fills in two descriptors.
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(opened, 0);
    let flash = flash_path("signal-tty");
    let _ = std::fs::remove_file(&flash);
    let path = temp_program("signal-tty", &SPAM);
    // SAFETY: dup of a descriptor this test owns; the new one is handed to the child.
    let tty = unsafe { OwnedFd::from_raw_fd(libc::dup(slave)) };
    // Nothing reads the other end, so the terminal fills and the run waits in a write.
    let mut child = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .args(["--firmware", path.to_str().unwrap()])
        .args(["--console", "--unpaced", "--flash"])
        .arg(&flash)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(tty))
        .spawn()
        .expect("failed to run cmm2-emulator");
    wait_until_blocked(&child);

    // SAFETY: kill only sends the signal to the child, which is still running.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let status = wait_for(&mut child);

    assert_eq!(status.signal(), Some(libc::SIGTERM));
    assert_eq!(std::fs::read(&flash).unwrap().len(), FLASH_FILE_SIZE);
    // SAFETY: closing the two descriptors made above.
    unsafe {
        libc::close(master);
        libc::close(slave);
    }
    std::fs::remove_file(&flash).unwrap();
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn a_directory_that_cannot_be_read_does_not_fail_a_save_that_has_been_made() {
    if root() {
        return;
    }
    // Write and search but not read: the file can be made and renamed in, and the directory
    // cannot be opened to be synced, nor to look at its attributes. Both are said, as warnings.
    let dir = temp_path("dir-unreadable", "d");
    std::fs::create_dir(&dir).unwrap();
    let flash = dir.join("board.flash");
    mode(&dir, 0o300);

    let output = run_program(
        "dir-unreadable",
        &THREE,
        &["--run", "10", "--flash", flash.to_str().unwrap()],
    );

    mode(&dir, 0o755);
    assert!(output.status.success(), "{}", stderr(&output));
    let lines: Vec<String> = stderr(&output).lines().map(str::to_string).collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].contains("its directory cannot be read"),
        "{lines:?}"
    );
    assert!(
        lines[1].contains("written, but its directory could not be synced"),
        "{lines:?}"
    );
    assert_eq!(std::fs::read(&flash).unwrap().len(), FLASH_FILE_SIZE);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_temporary_file_nobody_holds_is_removed_at_the_start_and_one_that_is_held_is_kept() {
    let dir = temp_path("stale-temp", "d");
    std::fs::create_dir(&dir).unwrap();
    let flash = dir.join("board.flash");
    let old = std::time::SystemTime::now() - Duration::from_secs(3600);
    let left = dir.join(".cmm2.4242.1.tmp");
    let held = dir.join(".cmm2.4242.2.tmp");
    for file in [&left, &held] {
        std::fs::write(file, b"left behind").unwrap();
        std::fs::File::options()
            .write(true)
            .open(file)
            .unwrap()
            .set_modified(old)
            .unwrap();
    }
    // Held by a save that is going on, as this test's own lock stands for.
    let holder = std::fs::File::open(&held).unwrap();
    // SAFETY: flock on a descriptor that is open.
    assert_eq!(
        unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&holder), libc::LOCK_EX) },
        0
    );

    let output = run_program(
        "stale-temp",
        &THREE,
        &["--run", "10", "--flash", flash.to_str().unwrap()],
    );

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!left.exists());
    assert_eq!(std::fs::read(&held).unwrap(), b"left behind");
    drop(holder);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn output_to_the_master_of_a_terminal_arrives() {
    use std::os::fd::{FromRawFd, OwnedFd};
    let (mut master, mut slave) = (0, 0);
    // SAFETY: openpty fills in two descriptors; the slave is put in raw mode through a termios
    // that was got from it.
    unsafe {
        assert_eq!(
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null()
            ),
            0
        );
        let mut termios: libc::termios = std::mem::zeroed();
        libc::tcgetattr(slave, &mut termios);
        libc::cfmakeraw(&mut termios);
        libc::tcsetattr(slave, libc::TCSANOW, &termios);
    }
    // SAFETY: dup of a descriptor this test owns; the new one is handed to the child.
    let as_stdout = unsafe { OwnedFd::from_raw_fd(libc::dup(master)) };

    // A run that prints its summary to the master, which is not a thing to be reopened: that
    // would make a new pair, and the summary would go nowhere.
    let path = temp_program("master", &THREE);
    let status = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .args(["--firmware", path.to_str().unwrap(), "--run", "1000"])
        .stdout(Stdio::from(as_stdout))
        .status()
        .expect("failed to run cmm2-emulator");
    assert!(status.success());

    let mut got = vec![0u8; 4096];
    // What was sent is waiting, or nothing ever will be: a wait of a few seconds says which, and
    // a test that fails does not hang.
    let mut waiting = libc::pollfd {
        fd: slave,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: poll on one live pollfd.
    assert_eq!(
        unsafe { libc::poll(&mut waiting, 1, 5000) },
        1,
        "nothing arrived"
    );
    // SAFETY: reading into a live buffer from the slave, which has data waiting.
    let count = unsafe { libc::read(slave, got.as_mut_ptr().cast(), got.len()) };
    let got = String::from_utf8_lossy(&got[..usize::try_from(count).unwrap()]).into_owned();
    assert!(got.starts_with("stopped: instruction limit\n"), "{got:?}");
    assert!(got.contains("instructions 1000\n"), "{got:?}");
    // SAFETY: closing the two descriptors made above.
    unsafe {
        libc::close(master);
        libc::close(slave);
    }
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn an_old_fifo_named_like_a_temporary_file_does_not_hang_the_start() {
    let dir = temp_path("stale-fifo", "d");
    std::fs::create_dir(&dir).unwrap();
    let fifo = dir.join(".cmm2.1.1.tmp");
    make_fifo(&fifo);
    // Two hours old, by its path: opening a FIFO would wait for a writer.
    let two_hours_ago = std::time::SystemTime::now() - Duration::from_secs(7200);
    let secs = two_hours_ago
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let times = [
        libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_OMIT,
        },
        libc::timespec {
            tv_sec: secs as libc::time_t,
            tv_nsec: 0,
        },
    ];
    let name = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: utimensat with a NUL-terminated name and two timespecs that live through the call.
    assert_eq!(
        unsafe { libc::utimensat(libc::AT_FDCWD, name.as_ptr(), times.as_ptr(), 0) },
        0
    );
    let flash = dir.join("board.flash");

    let mut child = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .args([
            "--firmware",
            temp_program("stale-fifo", &THREE).to_str().unwrap(),
        ])
        .args(["--run", "10", "--flash", flash.to_str().unwrap()])
        .spawn()
        .expect("failed to run cmm2-emulator");
    let status = wait_for(&mut child);

    assert!(status.success());
    assert!(fifo.exists());
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_file(temp_path("stale-fifo", "bin")).unwrap();
}
