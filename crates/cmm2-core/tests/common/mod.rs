//! What the real firmware tests share: finding the firmware, booting it, typing at it and
//! running it to a prompt.

// Each test file uses some of these and not others.
#![allow(dead_code)]

use cmm2_core::{Console, Cpu, Firmware, Stop};

pub fn firmware() -> Firmware {
    let path = std::env::var("CMM2_FIRMWARE")
        .expect("CMM2_FIRMWARE must be set to the path of CMM2_MMBasic_7.00.00b5.bin");
    Firmware::load(path).unwrap()
}

/// Runs in slices until the console output so far contains `text`, or `cap` instructions have
/// run. Returns the output so far.
pub fn run_to_text(cpu: &mut Cpu, console: &Console, text: &str, cap: u64) -> String {
    let limit = cpu.instruction_count() + cap;
    let mut output = String::new();
    while !output.contains(text) && cpu.instruction_count() < limit {
        let stop = cpu.run(1_000_000);
        assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");
        output.push_str(&String::from_utf8_lossy(&console.take_output()));
    }
    assert!(output.contains(text), "no {text:?} in {output:?}");
    output
}

/// Boots, answers the setup questions and runs through the reset to the second boot's prompt.
/// Returns the instruction count at the prompt.
pub fn boot_to_second_prompt(cpu: &mut Cpu, console: &Console) -> u64 {
    run_to_text(cpu, console, "Keyboard type", 300_000_000);
    console.push_input(b"1");
    run_to_text(cpu, console, "HH:MM:SS", 20_000_000);
    console.push_input(b"12:00:00\r");
    run_to_text(cpu, console, "DD/MM/YY", 20_000_000);
    console.push_input(b"12/11/26\r");
    while cpu.resets() == 0 {
        assert!(cpu.instruction_count() < 250_000_000, "no reset");
        let stop = cpu.run(1_000_000);
        assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");
        console.take_output();
    }
    run_to_text(cpu, console, "\r\n> ", 500_000_000);
    cpu.instruction_count()
}

// The text the firmware prints for one typed line: what lies between the echo of the line and
// the next prompt.
pub fn answer_to(cpu: &mut Cpu, console: &Console, line: &str) -> String {
    let (output, _) = type_keys(cpu, console, &format!("{line}\r"));
    output
        .strip_prefix(&format!("{line}\r\n"))
        .and_then(|rest| rest.strip_suffix("\r\n> "))
        .unwrap_or_else(|| panic!("{output:?}"))
        .to_string()
}

/// Types `keys` and runs to the next prompt. Returns everything the firmware printed on the
/// way, the echo of the keys included, and the instructions it took.
pub fn type_keys(cpu: &mut Cpu, console: &Console, keys: &str) -> (String, u64) {
    let start = cpu.instruction_count();
    console.push_input(keys.as_bytes());
    let mut output = String::new();
    while !output.ends_with("\r\n> ") {
        assert!(
            cpu.instruction_count() - start < 2_000_000_000,
            "{output:?}"
        );
        let stop = cpu.run(10_000);
        assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");
        output.push_str(&String::from_utf8_lossy(&console.take_output()));
    }
    let spent = cpu.instruction_count() - start;
    println!("{keys:?} -> {output:?} in {spent} instructions");
    (output, spent)
}

/// Runs in slices until `done` says so, or `cap` instructions have run. Returns everything the
/// console printed.
pub fn run_until(
    cpu: &mut Cpu,
    console: &Console,
    cap: u64,
    mut done: impl FnMut(&str) -> bool,
) -> String {
    let limit = cpu.instruction_count() + cap;
    let mut output = String::new();
    while !done(&output) && cpu.instruction_count() < limit {
        let stop = cpu.run(10_000);
        assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");
        output.push_str(&String::from_utf8_lossy(&console.take_output()));
    }
    assert!(done(&output), "{output:?}");
    output
}

/// Types each line of BASIC in turn, to its prompt. CLS clears the screen after the echo, so
/// the prompt is not always at the start of a line.
pub fn say_all(cpu: &mut Cpu, console: &Console, lines: &[&str]) {
    for line in lines {
        console.push_input(format!("{line}\r").as_bytes());
        run_until(cpu, console, 2_000_000_000, |output| output.ends_with("> "));
    }
}
