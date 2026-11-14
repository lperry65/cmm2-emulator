//! The real firmware at its console: a key typed in, and a command run at the prompt.

mod common;

use cmm2_core::{Cpu, Irqs, Stop, cmm2_memory};
use common::*;

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_receives_a_key_from_the_console() {
    let firmware = firmware();
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();

    let stop = cpu.run(200_000_000);
    assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");
    let before = String::from_utf8_lossy(&console.take_output()).into_owned();
    assert!(before.contains("Keyboard type"), "{before:?}");

    console.push_input(b"1");
    let stop = cpu.run(20_000_000);
    assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");
    let after = String::from_utf8_lossy(&console.take_output()).into_owned();
    // Measured: the key is echoed and the next setup prompt follows.
    assert!(after.contains("Enter time (24hr clock)"), "{after:?}");
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_runs_a_command_at_the_prompt() {
    let firmware = firmware();
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();

    boot_to_second_prompt(&mut cpu, &console);
    // Without the LTDC line interrupt the main thread waited at 0x0809A508 for it and never ran
    // the line. Measured: the first answer is back 17 million instructions after the keys and
    // the later ones 7 to 8 million, each the line echoed, the answer and a new prompt.
    let mut answer = |keys: &[u8]| {
        console.push_input(keys);
        let output = run_to_text(&mut cpu, &console, "\r\n> ", 50_000_000);
        let echo = String::from_utf8_lossy(&keys[..keys.len() - 1]).into_owned();
        let answer = output
            .strip_prefix(&format!("{echo}\r\n"))
            .and_then(|rest| rest.strip_suffix("\r\n> "))
            .unwrap_or_else(|| panic!("{output:?}"))
            .to_string();
        println!("{echo} -> {answer:?} at {}", cpu.instruction_count());
        answer
    };
    assert_eq!(answer(b"PRINT 1+2\r"), " 3");
    // The clock was set to 12:00:00 on 12/11/26 in the setup, and under a second has run since.
    let time = answer(b"PRINT TIME$\r");
    assert!(time.starts_with("12:00:0"), "{time:?}");
    assert_eq!(answer(b"PRINT DATE$\r"), "12-11-2026");
}
