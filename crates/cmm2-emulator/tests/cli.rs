use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
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

const USAGE: &str = "usage: cmm2-emulator --version | --lookup <address> | --firmware <file> [--run <count>] [--stop-at <address>] [--trace <file>] [--trace-mmio] [--trace-from <count>] [--console] [--on <text> <keys>] [--screenshot <file>] [--window] [--unpaced] [--flash <file>] [--sd <file>]";

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
// start of AXI SRAM, reloaded, and one red pixel stored there. The addresses are literals at +52.
const SCREEN: [u16; 44] = [
    0x480C, 0x490D, 0x4A0D, 0x6182, 0x4A0D, 0x6102, 0x4A0B, 0x604A, 0x4A0B, 0x608A, 0x4A0A, 0x60CA,
    0x4A0A, 0x614A, 0x4A0A, 0x618A, 0x4A0A, 0x62CA, 0x4A0A, 0x630A, 0x4A04, 0x6242, 0x4A09, 0x4B07,
    0x801A, 0xE7FE, 0x1000, 0x5000, 0x1080, 0x5000, 0x0001, 0x0000, 0x0001, 0x0001, 0x0002, 0x0000,
    0x00FF, 0x0000, 0x0000, 0x2400, 0x0000, 0x0002, 0xF800, 0x0000,
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
    let flash = flash_path("flash-temp");
    let _ = std::fs::remove_file(&flash);
    let temp = PathBuf::from(format!("{}.tmp", flash.display()));

    run_program(
        "flash-temp",
        &THREE,
        &["--run", "10", "--flash", flash.to_str().unwrap()],
    );

    assert!(flash.exists());
    assert!(!temp.exists());
    std::fs::remove_file(&flash).unwrap();
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
fn a_flash_file_that_cannot_be_written_is_an_error() {
    let missing = std::env::temp_dir().join("cmm2-cli-no-such-directory/f.flash");

    let output = run_program(
        "flash-unwritable",
        &THREE,
        &["--run", "10", "--flash", missing.to_str().unwrap()],
    );

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!(
            "cmm2-emulator: {}: No such file or directory (os error 2)\n",
            missing.display()
        )
    );
}

// A card image of four blocks, each filled with its own number, and its inode. The image is
// written back through a temporary file and a rename, so a card that was written back has a
// new inode, and the same bytes, as the guest never touches this card.
fn sd_card(name: &str) -> (PathBuf, Vec<u8>) {
    let path = temp_path(name, "img");
    let image: Vec<u8> = (1..=4u8).flat_map(|b| [b; 512]).collect();
    std::fs::write(&path, &image).unwrap();
    (path, image)
}

fn written_back(path: &PathBuf, image: &[u8], inode: u64) -> bool {
    std::fs::metadata(path).unwrap().ino() != inode && std::fs::read(path).unwrap() == image
}

#[test]
fn the_sd_file_is_written_back_whichever_way_the_run_ends() {
    let cases: [(&str, &[u16], &[&str]); 4] = [
        ("sd-limit", &THREE, &["--run", "10"]),
        ("sd-console", &THREE, &["--console", "--run", "10"]),
        ("sd-stop-at", &THREE, &["--stop-at", "0x08000106"]),
        ("sd-fault", &UNDEFINED, &["--run", "100"]),
    ];
    for (name, code, args) in cases {
        let (card, image) = sd_card(name);
        let inode = std::fs::metadata(&card).unwrap().ino();
        let mut all = vec!["--sd", card.to_str().unwrap()];
        all.extend_from_slice(args);

        run_program(name, code, &all);

        assert!(written_back(&card, &image, inode), "{name}");
        std::fs::remove_file(&card).unwrap();
    }
}

#[test]
fn the_sd_file_is_written_back_when_a_live_run_faults() {
    let (card, image) = sd_card("sd-fault-live");
    let inode = std::fs::metadata(&card).unwrap().ino();

    let output = run_program(
        "sd-fault-live",
        &UNDEFINED,
        &["--console", "--sd", card.to_str().unwrap()],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(written_back(&card, &image, inode));
    std::fs::remove_file(&card).unwrap();
}

#[test]
fn the_sd_file_is_written_back_when_the_quit_key_ends_the_run() {
    let (card, image) = sd_card("sd-quit");
    let inode = std::fs::metadata(&card).unwrap().ino();
    let path = temp_program("sd-quit", &THREE);
    let mut child = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .args(["--firmware", path.to_str().unwrap(), "--console", "--sd"])
        .arg(&card)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("failed to run cmm2-emulator");
    child.stdin.as_mut().unwrap().write_all(b"\x1D").unwrap();
    let status = child.wait().unwrap();

    assert!(status.success());
    assert!(written_back(&card, &image, inode));
    std::fs::remove_file(&card).unwrap();
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn saving_the_sd_file_leaves_no_temporary_file() {
    let (card, _) = sd_card("sd-temp");
    let temp = PathBuf::from(format!("{}.tmp", card.display()));

    run_program(
        "sd-temp",
        &THREE,
        &["--run", "10", "--sd", card.to_str().unwrap()],
    );

    assert!(!temp.exists());
    std::fs::remove_file(&card).unwrap();
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

#[test]
fn an_sd_file_that_cannot_be_written_back_is_an_error() {
    // The temporary file the card is written to first is in the way, as a directory.
    let (card, _) = sd_card("sd-unwritable");
    let temp = PathBuf::from(format!("{}.tmp", card.display()));
    std::fs::create_dir(&temp).unwrap();

    let output = run_program(
        "sd-unwritable",
        &THREE,
        &["--run", "10", "--sd", card.to_str().unwrap()],
    );

    std::fs::remove_dir(&temp).unwrap();
    std::fs::remove_file(&card).unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!(
            "cmm2-emulator: {}: Is a directory (os error 21)\n",
            card.display()
        )
    );
}
