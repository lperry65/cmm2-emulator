//! The real firmware's USB stack against the keyboard on the OTG model.

mod common;

use common::*;

use std::cell::RefCell;
use std::rc::Rc;

use cmm2_core::{Console, Cpu, HidKeyboard, Irqs, KeyReport, Stop, UsbPort, cmm2_memory};

const KEY_A: u8 = 0x04;

const LEFT_SHIFT: u8 = 0x02;

/// What `boot_with_usb_keyboard` hands back: the machine with the keyboard enumerated and polled.
struct UsbBoot {
    cpu: Cpu,
    console: Console,
    keys: KeyReport,
    usb: UsbPort,
    /// What the firmware printed from the second prompt to the connect message.
    connect_output: String,
}

// Boots with a USB keyboard plugged in, and runs until the firmware has enumerated it and
// started to poll it. The USB task starts a second or so after the prompt, and the keyboard is
// usable about a second after that (docs/usb-host.md section 3), so there is no point in
// setting a key before the first poll.
fn boot_with_usb_keyboard() -> UsbBoot {
    let firmware = firmware();
    let irqs = Irqs::new();
    let mut memory = cmm2_memory(&irqs);
    let console = memory.console();
    let keys = KeyReport::new();
    let usb = memory.usb_port();
    usb.attach(HidKeyboard::new(keys.clone()));
    // The firmware polls with a write of HCCHAR for channel 1 that has the enable bit set.
    let polls = Rc::new(RefCell::new(0u32));
    let sink = polls.clone();
    memory.set_peripheral_trace(Some(Box::new(move |access| {
        if access.write && access.addr == 0x4008_0520 && access.value & 1 << 31 != 0 {
            *sink.borrow_mut() += 1;
        }
    })));
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();

    let prompt = boot_to_second_prompt(&mut cpu, &console);
    println!(
        "prompt at {prompt} instructions, cycle {}",
        cpu.cycle_count()
    );
    let connect_output = run_until(&mut cpu, &console, 200_000_000, |text| {
        text.contains("USB Keyboard Connected on channel 1\r\n")
    });
    println!(
        "connected at {} instructions, cycle {}",
        cpu.instruction_count(),
        cpu.cycle_count()
    );
    run_until(&mut cpu, &console, 400_000_000, |_| *polls.borrow() > 0);
    println!(
        "first poll at {} instructions, cycle {}",
        cpu.instruction_count(),
        cpu.cycle_count()
    );
    UsbBoot {
        cpu,
        console,
        keys,
        usb,
        connect_output,
    }
}

// Runs `instructions` and returns what the console printed meanwhile.
fn run_and_collect(boot: &mut UsbBoot, instructions: u64) -> String {
    let stop = boot.cpu.run(instructions);
    assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");
    String::from_utf8_lossy(&boot.console.take_output()).into_owned()
}

// Types `line`, then runs on, and sets each report after the number of instructions that go with
// it. Returns the output up to the next prompt.
fn run_line_with_keys(boot: &mut UsbBoot, line: &str, events: &[(u64, [u8; 8])]) -> String {
    let start = boot.cpu.instruction_count();
    boot.console.push_input(line.as_bytes());
    let mut output = String::new();
    for &(instructions, report) in events {
        output += &run_and_collect(boot, instructions);
        boot.keys.set(report);
    }
    while !output.ends_with("\r\n> ") {
        assert!(
            boot.cpu.instruction_count() - start < 500_000_000,
            "{output:?}"
        );
        output += &run_and_collect(boot, 10_000);
    }
    output
}

// The firmware's USB stack enumerates the keyboard on the OTG model, and prints the message it
// prints for a keyboard on the real board.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_firmware_enumerates_a_usb_keyboard() {
    let boot = boot_with_usb_keyboard();
    // Measured: the first boot's questions end with the UK keyboard, so the message says UK.
    assert_eq!(
        boot.connect_output, "UK USB Keyboard Connected on channel 1\r\n",
        "{:?}",
        boot.connect_output
    );
}

// Keys set in the report reach the firmware through the polling: KEYDOWN() counts them and names
// the first, and a release brings the count back to 0.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_firmware_reads_the_held_keys_of_a_usb_keyboard() {
    let mut boot = boot_with_usb_keyboard();

    // 'a' goes down while the first PAUSE runs and up during the second: the first reading is
    // taken while it is down, and the second after it has been let go. 100 ms is about 22 million
    // instructions, and the key is down for 38 million, short of the firmware's repeat delay.
    let output = run_line_with_keys(
        &mut boot,
        "PAUSE 100:a=KEYDOWN(0):b=KEYDOWN(1):PAUSE 200:c=KEYDOWN(0):PRINT STR$(a)+\",\"+STR$(b)+\",\"+STR$(c)\r",
        &[
            (2_000_000, [0, 0, KEY_A, 0, 0, 0, 0, 0]),
            (38_000_000, [0; 8]),
        ],
    );
    // 97 is "a": the firmware reports the character, not the usage.
    assert!(output.ends_with("\r\n1,97,0\r\n> "), "{output:?}");

    // Two keys and a modifier: the count, and KEYDOWN(7), which has the modifier bits.
    let output = run_line_with_keys(
        &mut boot,
        "PAUSE 100:a=KEYDOWN(0):b=KEYDOWN(7):PAUSE 200:PRINT STR$(a)+\",\"+STR$(b)\r",
        &[
            (2_000_000, [LEFT_SHIFT, 0, KEY_A, KEY_A + 1, 0, 0, 0, 0]),
            (38_000_000, [0; 8]),
        ],
    );
    assert!(output.ends_with("\r\n2,8\r\n> "), "{output:?}");
}

// INKEY$ returns the character that a key down sends, and at the prompt it is typed into the
// console, shifted when the modifier byte says so. The device sends a report only when it has
// changed, so a press shorter than the poll is not seen.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_firmware_types_the_keys_of_a_usb_keyboard() {
    let mut boot = boot_with_usb_keyboard();

    // The line's own end of line is still in the input when it starts, so it empties the input
    // first. 20 ms is about 4.4 million instructions.
    let output = run_line_with_keys(
        &mut boot,
        "PAUSE 20:DO WHILE INKEY$<>\"\":LOOP:DO:a$=INKEY$:LOOP UNTIL a$<>\"\":PRINT \"got \"+a$\r",
        &[
            (8_000_000, [0, 0, KEY_A, 0, 0, 0, 0, 0]),
            (5_000_000, [0; 8]),
        ],
    );
    assert!(output.ends_with("\r\ngot a\r\n> "), "{output:?}");

    // The release has not been polled yet, and a report that is set and reset between two polls
    // is never seen: give the firmware time to see it.
    run_and_collect(&mut boot, 5_000_000);

    // Shift and 'a' at the idle prompt type "A".
    boot.keys.set([LEFT_SHIFT, 0, KEY_A, 0, 0, 0, 0, 0]);
    let mut typed = run_and_collect(&mut boot, 5_000_000);
    boot.keys.set([0; 8]);
    typed += &run_and_collect(&mut boot, 5_000_000);
    assert_eq!(typed, "A");
}

// Pulling the keyboard out is noticed, and plugging one in again is a fresh enumeration.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_firmware_sees_a_usb_keyboard_come_and_go() {
    let mut boot = boot_with_usb_keyboard();

    boot.usb.detach();
    run_until(&mut boot.cpu, &boot.console, 100_000_000, |text| {
        text.contains("USB Keyboard Disconnected\r\n")
    });
    boot.usb.attach(HidKeyboard::new(boot.keys.clone()));
    run_until(&mut boot.cpu, &boot.console, 400_000_000, |text| {
        text.contains("UK USB Keyboard Connected on channel 1\r\n")
    });
}

// A restart takes the USB stack down: the keyboard is not polled until the firmware has brought
// it up again, so a front end knows to type into the console in between.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_firmware_polls_a_usb_keyboard_again_after_a_restart() {
    let mut boot = boot_with_usb_keyboard();
    // The first poll has been started, and the keyboard has answered it a little later.
    run_and_collect(&mut boot, 5_000_000);
    assert!(boot.keys.polled() && boot.keys.was_sent());
    let restarts = boot.cpu.resets();
    boot.console.push_input(b"CPU RESTART\r");
    let limit = boot.cpu.instruction_count() + 100_000_000;
    while boot.cpu.resets() == restarts {
        assert!(boot.cpu.instruction_count() < limit, "no restart");
        run_and_collect(&mut boot, 100_000);
    }
    assert!(!boot.keys.polled() && !boot.keys.was_sent());
    let limit = boot.cpu.instruction_count() + 1_500_000_000;
    while !boot.keys.polled() {
        assert!(
            boot.cpu.instruction_count() < limit,
            "no poll after the restart"
        );
        run_and_collect(&mut boot, 1_000_000);
    }
}

// The firmware shows the lock keys on the keyboard's LEDs with SET_REPORT, which a front end reads
// to keep the host's locks and the firmware's the same.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_firmware_shows_the_lock_keys_of_a_usb_keyboard_on_its_leds() {
    const CAPS_LOCK: u8 = 0x39;
    let mut boot = boot_with_usb_keyboard();
    assert_eq!(boot.keys.leds(), 0);
    boot.keys.set([0, 0, CAPS_LOCK, 0, 0, 0, 0, 0]);
    run_and_collect(&mut boot, 5_000_000);
    boot.keys.set([0; 8]);
    run_and_collect(&mut boot, 5_000_000);
    assert_eq!(boot.keys.leds(), 0b010, "caps lock is bit 1");
    // Tapped again, it is off.
    boot.keys.set([0, 0, CAPS_LOCK, 0, 0, 0, 0, 0]);
    run_and_collect(&mut boot, 5_000_000);
    boot.keys.set([0; 8]);
    run_and_collect(&mut boot, 5_000_000);
    assert_eq!(boot.keys.leds(), 0);
}
