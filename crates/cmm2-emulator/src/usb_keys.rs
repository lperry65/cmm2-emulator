//! The window's keys as the USB keyboard's report: which keys are down, in the order they went
//! down, as the 8-byte boot report the firmware polls for. This is the path that gives the
//! firmware held-key state (`KEYDOWN()`); until it starts polling, the window types into the
//! console instead (see `UsbKeys::active`).

use cmm2_core::KeyReport;
use sdl2::keyboard::Scancode;

/// The most keys a boot report has room for.
const SLOTS: usize = 6;

/// The usage the report has in every slot when more keys are down than fit: ErrorRollOver.
const ROLLOVER: u8 = 0x01;

/// The modifier usages, Left Control to Right GUI, are bits 0 to 7 of the report's first byte.
const FIRST_MODIFIER: u8 = 0xE0;
const LAST_MODIFIER: u8 = 0xE7;

/// The USB HID usage of a key, from the keyboard page. SDL numbers its scancodes by these
/// usages, so the letters, digits, punctuation, F1 to F12, the cursor and editing keys and the
/// keypad (usages 0x04 to 0x65) and the eight modifiers (0xE0 to 0xE7) are the same numbers.
/// Anything else is a key the boot report cannot carry: media keys, F13 and up, and so on.
pub fn usage(scancode: Scancode) -> Option<u8> {
    let number = scancode as i32;
    match u8::try_from(number) {
        Ok(usage @ (0x04..=0x65 | FIRST_MODIFIER..=LAST_MODIFIER)) => Some(usage),
        _ => None,
    }
}

fn is_modifier(usage: u8) -> bool {
    (FIRST_MODIFIER..=LAST_MODIFIER).contains(&usage)
}

struct Key {
    usage: u8,
    down: bool,
    /// The firmware has been sent a report with this key in it.
    seen: bool,
}

/// The keys held and the report they make. A cheap handle on the keyboard's `KeyReport`, which
/// the keyboard sends the firmware when it polls.
pub struct UsbKeys {
    report: KeyReport,
    /// In the order they were pressed. A key that has been let go stays until the firmware has
    /// had a report with it in, so a tap between two polls is not lost.
    keys: Vec<Key>,
}

impl UsbKeys {
    pub fn new(report: KeyReport) -> UsbKeys {
        UsbKeys {
            report,
            keys: Vec::new(),
        }
    }

    /// Whether the firmware is polling the keyboard. Until then the keys are not USB's: they go
    /// to the console, which is all that answers the first-boot question and anything else
    /// typed before the USB stack is up.
    ///
    /// A key that was pressed before this became true, and is still down, was typed on the
    /// console, and is not in the report: it is typed once, and does not turn up again as a key
    /// down at the switch. It counts again from its next press. The modifiers are kept all the
    /// while, so Shift held across the switch still shifts the next key.
    pub fn active(&self) -> bool {
        self.report.polled()
    }

    /// A key went down. Pressing a key that is down does nothing: the firmware repeats the key
    /// itself, and SDL's repeats must not reach it.
    pub fn press(&mut self, usage: u8) {
        if !is_modifier(usage) && !self.active() {
            return;
        }
        self.note_what_was_sent();
        match self.keys.iter_mut().find(|key| key.usage == usage) {
            Some(key) => key.down = true,
            None => self.keys.push(Key {
                usage,
                down: true,
                seen: false,
            }),
        }
        self.publish();
    }

    /// A key came up. One that was never pressed (a console key) is ignored.
    pub fn release(&mut self, usage: u8) {
        self.note_what_was_sent();
        if let Some(key) = self.keys.iter_mut().find(|key| key.usage == usage) {
            key.down = false;
        }
        self.publish();
    }

    /// Every key up, as when the window loses the focus and will not see the releases.
    pub fn release_all(&mut self) {
        self.note_what_was_sent();
        for key in &mut self.keys {
            key.down = false;
        }
        self.publish();
    }

    /// Takes in what the keyboard has sent since the last call: a key that has been let go goes
    /// out of the report once the firmware has had it.
    pub fn sync(&mut self) {
        self.note_what_was_sent();
        self.publish();
    }

    // Every key is in the report that was last set, so if the keyboard has sent that, the
    // firmware has seen them all.
    fn note_what_was_sent(&mut self) {
        if self.report.was_sent() {
            for key in &mut self.keys {
                key.seen = true;
            }
        }
    }

    // A key that is up goes once it has been seen. With no polling there is nothing to hold it for.
    fn publish(&mut self) {
        let polling = self.active();
        self.keys.retain(|key| key.down || (polling && !key.seen));
        self.report.set(self.build());
    }

    fn build(&self) -> [u8; 8] {
        let mut report = [0; 8];
        let mut count = 0;
        for key in &self.keys {
            if is_modifier(key.usage) {
                report[0] |= 1 << (key.usage - FIRST_MODIFIER);
            } else {
                if count < SLOTS {
                    report[2 + count] = key.usage;
                }
                count += 1;
            }
        }
        if count > SLOTS {
            report[2..].fill(ROLLOVER);
        }
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmm2_core::{HidKeyboard, In, UsbDevice};

    const A: u8 = 0x04;
    const B: u8 = 0x05;
    const LEFT_SHIFT: u8 = 0xE1;
    const RIGHT_GUI: u8 = 0xE7;

    /// The keys, the keyboard they feed and the report between them, before the firmware has
    /// polled.
    fn unpolled() -> (UsbKeys, HidKeyboard, KeyReport) {
        let report = KeyReport::new();
        let mut keyboard = HidKeyboard::new(report.clone());
        keyboard.setup([0x00, 0x09, 1, 0, 0, 0, 0, 0]);
        keyboard.in_token(0, 8);
        (UsbKeys::new(report.clone()), keyboard, report)
    }

    /// As `unpolled`, once the firmware has polled once.
    fn polled() -> (UsbKeys, HidKeyboard, KeyReport) {
        let (keys, mut keyboard, report) = unpolled();
        keyboard.in_token(1, 8);
        (keys, keyboard, report)
    }

    fn poll(keyboard: &mut HidKeyboard) -> In {
        keyboard.in_token(1, 8)
    }

    #[test]
    fn scancodes_are_the_usb_usages() {
        assert_eq!(usage(Scancode::A), Some(0x04));
        assert_eq!(usage(Scancode::Z), Some(0x1D));
        assert_eq!(usage(Scancode::Num1), Some(0x1E));
        assert_eq!(usage(Scancode::Num0), Some(0x27));
        assert_eq!(usage(Scancode::Return), Some(0x28));
        assert_eq!(usage(Scancode::Space), Some(0x2C));
        assert_eq!(usage(Scancode::F1), Some(0x3A));
        assert_eq!(usage(Scancode::F12), Some(0x45));
        assert_eq!(usage(Scancode::Right), Some(0x4F));
        assert_eq!(usage(Scancode::Up), Some(0x52));
        assert_eq!(usage(Scancode::KpEnter), Some(0x58));
        assert_eq!(usage(Scancode::NonUsBackslash), Some(0x64));
        assert_eq!(usage(Scancode::Application), Some(0x65));
        assert_eq!(usage(Scancode::LCtrl), Some(0xE0));
        assert_eq!(usage(Scancode::LShift), Some(0xE1));
        assert_eq!(usage(Scancode::RGui), Some(0xE7));
    }

    #[test]
    fn keys_the_boot_report_cannot_carry_have_no_usage() {
        assert_eq!(usage(Scancode::Power), None);
        assert_eq!(usage(Scancode::F13), None);
        assert_eq!(usage(Scancode::AudioMute), None);
        assert_eq!(usage(Scancode::Mode), None);
    }

    #[test]
    fn it_is_active_once_the_firmware_polls() {
        let (keys, _, _) = unpolled();
        assert!(!keys.active());
        let (keys, _, _) = polled();
        assert!(keys.active());
    }

    #[test]
    fn keys_go_into_the_report_in_the_order_they_went_down() {
        let (mut keys, _, report) = polled();
        keys.press(B);
        keys.press(A);
        keys.press(0x1D);
        assert_eq!(report.get(), [0, 0, B, A, 0x1D, 0, 0, 0]);
    }

    #[test]
    fn the_modifiers_are_the_bits_of_the_first_byte() {
        let (mut keys, mut keyboard, report) = polled();
        keys.press(0xE0);
        keys.press(LEFT_SHIFT);
        keys.press(0xE2);
        keys.press(0xE3);
        keys.press(0xE4);
        keys.press(0xE5);
        keys.press(0xE6);
        keys.press(RIGHT_GUI);
        assert_eq!(report.get(), [0xFF, 0, 0, 0, 0, 0, 0, 0]);
        poll(&mut keyboard);
        keys.release(LEFT_SHIFT);
        keys.release(RIGHT_GUI);
        assert_eq!(report.get(), [0b0111_1101, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn a_modifier_does_not_take_a_key_slot() {
        let (mut keys, _, report) = polled();
        keys.press(LEFT_SHIFT);
        for usage in A..A + 6 {
            keys.press(usage);
        }
        assert_eq!(report.get(), [2, 0, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn a_key_let_go_leaves_the_others_in_their_order() {
        let (mut keys, mut keyboard, report) = polled();
        keys.press(A);
        keys.press(B);
        keys.press(0x06);
        poll(&mut keyboard);
        keys.release(B);
        assert_eq!(report.get(), [0, 0, A, 0x06, 0, 0, 0, 0]);
    }

    #[test]
    fn more_than_six_keys_is_the_rollover_error_in_every_slot() {
        let (mut keys, mut keyboard, report) = polled();
        keys.press(LEFT_SHIFT);
        for usage in A..A + 7 {
            keys.press(usage);
        }
        assert_eq!(report.get(), [2, 0, 1, 1, 1, 1, 1, 1]);
        // Back to six, and the keys are reported again.
        poll(&mut keyboard);
        keys.release(A + 6);
        assert_eq!(report.get(), [2, 0, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn six_keys_are_not_a_rollover() {
        let (mut keys, _, report) = polled();
        for usage in A..A + 6 {
            keys.press(usage);
        }
        assert_eq!(report.get(), [0, 0, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn a_key_pressed_again_while_down_is_one_key() {
        let (mut keys, _, report) = polled();
        keys.press(A);
        keys.press(A);
        assert_eq!(report.get(), [0, 0, A, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn a_key_let_go_that_was_polled_is_out_at_once() {
        let (mut keys, mut keyboard, report) = polled();
        keys.press(A);
        poll(&mut keyboard);
        keys.release(A);
        assert_eq!(report.get(), [0; 8]);
    }

    #[test]
    fn letting_go_of_a_key_never_pressed_changes_nothing() {
        let (mut keys, _, report) = polled();
        keys.press(A);
        keys.release(B);
        assert_eq!(report.get(), [0, 0, A, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn losing_the_focus_lets_go_of_every_key() {
        let (mut keys, mut keyboard, report) = polled();
        keys.press(LEFT_SHIFT);
        keys.press(A);
        keys.press(B);
        poll(&mut keyboard);
        keys.release_all();
        assert_eq!(report.get(), [0; 8]);
        // And a key pressed after it is a fresh press.
        keys.press(B);
        assert_eq!(report.get(), [0, 0, B, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn a_key_tapped_between_two_polls_is_held_until_it_is_polled() {
        let (mut keys, mut keyboard, report) = polled();
        keys.press(A);
        keys.release(A);
        assert_eq!(report.get(), [0, 0, A, 0, 0, 0, 0, 0]);
        // Nothing has polled yet, so it stays however often the window looks.
        keys.sync();
        keys.sync();
        assert_eq!(report.get(), [0, 0, A, 0, 0, 0, 0, 0]);
        assert_eq!(poll(&mut keyboard), In::Data(vec![0, 0, A, 0, 0, 0, 0, 0]));
        // The firmware has it, and the next look lets it go.
        keys.sync();
        assert_eq!(report.get(), [0; 8]);
        assert_eq!(poll(&mut keyboard), In::Data(vec![0; 8]));
    }

    #[test]
    fn a_tapped_modifier_is_held_until_it_is_polled_too() {
        let (mut keys, mut keyboard, report) = polled();
        keys.press(LEFT_SHIFT);
        keys.release(LEFT_SHIFT);
        assert_eq!(report.get(), [2, 0, 0, 0, 0, 0, 0, 0]);
        poll(&mut keyboard);
        keys.sync();
        assert_eq!(report.get(), [0; 8]);
    }

    #[test]
    fn a_key_that_is_pressed_again_before_it_was_polled_stays_one_tap() {
        let (mut keys, mut keyboard, report) = polled();
        keys.press(A);
        keys.release(A);
        keys.press(A);
        keys.release(A);
        assert_eq!(report.get(), [0, 0, A, 0, 0, 0, 0, 0]);
        poll(&mut keyboard);
        keys.sync();
        assert_eq!(report.get(), [0; 8]);
    }

    #[test]
    fn a_key_pressed_again_before_it_was_polled_is_held_down() {
        let (mut keys, mut keyboard, report) = polled();
        keys.press(A);
        keys.release(A);
        keys.press(A);
        poll(&mut keyboard);
        keys.sync();
        assert_eq!(report.get(), [0, 0, A, 0, 0, 0, 0, 0]);
        keys.release(A);
        assert_eq!(report.get(), [0; 8]);
    }

    #[test]
    fn a_tap_waits_with_the_keys_held_beside_it() {
        let (mut keys, mut keyboard, report) = polled();
        keys.press(A);
        poll(&mut keyboard);
        keys.press(B);
        keys.release(B);
        keys.sync();
        assert_eq!(report.get(), [0, 0, A, B, 0, 0, 0, 0]);
        poll(&mut keyboard);
        keys.sync();
        assert_eq!(report.get(), [0, 0, A, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn a_key_still_down_when_it_is_polled_stays() {
        let (mut keys, mut keyboard, report) = polled();
        keys.press(A);
        poll(&mut keyboard);
        keys.sync();
        assert_eq!(report.get(), [0, 0, A, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn a_key_pressed_again_after_a_release_unseen_by_the_firmware_is_still_down() {
        let (mut keys, mut keyboard, report) = polled();
        keys.press(A);
        poll(&mut keyboard);
        keys.release(A);
        keys.press(A);
        // The report is back to what the firmware has, and it is held.
        assert_eq!(report.get(), [0, 0, A, 0, 0, 0, 0, 0]);
        assert_eq!(poll(&mut keyboard), In::Nak);
        keys.sync();
        assert_eq!(report.get(), [0, 0, A, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn before_the_firmware_polls_the_keys_are_not_the_reports() {
        let (mut keys, _, report) = unpolled();
        keys.press(A);
        keys.release(A);
        assert_eq!(report.get(), [0; 8]);
    }

    #[test]
    fn before_the_firmware_polls_the_modifiers_are_kept() {
        let (mut keys, _, report) = unpolled();
        keys.press(LEFT_SHIFT);
        assert_eq!(report.get(), [2, 0, 0, 0, 0, 0, 0, 0]);
        keys.release(LEFT_SHIFT);
        // Not held for a poll that may never come: a tap here is gone.
        assert_eq!(report.get(), [0; 8]);
    }

    #[test]
    fn a_key_held_across_the_switch_is_typed_once_and_is_not_pressed_again() {
        let (mut keys, mut keyboard, report) = unpolled();
        // A and Shift go down before the firmware polls: A is the console's, Shift is kept.
        keys.press(LEFT_SHIFT);
        keys.press(A);
        assert!(!keys.active());
        assert_eq!(poll(&mut keyboard), In::Data(vec![2, 0, 0, 0, 0, 0, 0, 0]));
        assert!(keys.active());
        keys.sync();
        assert_eq!(report.get(), [2, 0, 0, 0, 0, 0, 0, 0]);
        // Its release is a key the report never had, and a new press is USB's.
        keys.release(A);
        assert_eq!(report.get(), [2, 0, 0, 0, 0, 0, 0, 0]);
        keys.press(A);
        assert_eq!(report.get(), [2, 0, A, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn a_bus_reset_goes_back_to_the_console() {
        let (mut keys, mut keyboard, _) = polled();
        keyboard.reset();
        assert!(!keys.active());
        keys.press(A);
        keys.release(A);
        assert_eq!(keys.report.get(), [0; 8]);
    }
}

/// The keys the window would send, typed into the real firmware over USB: what `UsbKeys` makes of
/// the key events is what the firmware's prompt, `KEYDOWN()` and program break take. They need the
/// firmware and are run with `CMM2_FIRMWARE=<path> cargo test --release -- --ignored`.
#[cfg(test)]
mod real_firmware {
    use super::*;
    use cmm2_core::{Console, Cpu, Firmware, HidKeyboard, Irqs, Stop, cmm2_memory};
    use std::time::Instant;

    const PROMPT: &str = "\r\n> ";

    /// How many instructions run between looks at the keys, as the window looks every 16 ms
    /// of wall time, and about 3 million instructions are 10 ms of the machine's.
    const SLICE: u64 = 3_000_000;

    /// The firmware at its prompt with the keyboard polling, and the keys that drive it.
    struct Board {
        cpu: Cpu,
        console: Console,
        keys: UsbKeys,
    }

    impl Board {
        /// Boots as the window does: the first-boot questions are answered over the console,
        /// as no USB keyboard is up for them, and then the machine runs until the firmware
        /// polls the keyboard.
        fn boot() -> Board {
            let path = std::env::var("CMM2_FIRMWARE")
                .expect("CMM2_FIRMWARE must be set to the path of CMM2_MMBasic_7.00.00b5.bin");
            let firmware = Firmware::load(path).unwrap();
            let irqs = Irqs::new();
            let memory = cmm2_memory(&irqs);
            let console = memory.console();
            let report = KeyReport::new();
            memory.usb_port().attach(HidKeyboard::new(report.clone()));
            let mut board = Board {
                cpu: Cpu::new(&firmware, memory, irqs).unwrap(),
                console,
                keys: UsbKeys::new(report),
            };
            board.answer("Keyboard type", "1", 300_000_000);
            board.answer("HH:MM:SS", "12:00:00\r", 20_000_000);
            board.answer("DD/MM/YY", "12/11/26\r", 20_000_000);
            while board.cpu.resets() == 0 {
                board.run(1_000_000);
            }
            board.until(PROMPT, 500_000_000);
            let (prompt_at, prompt_instructions) = (Instant::now(), board.cpu.instruction_count());
            assert!(!board.keys.active());
            while !board.keys.active() {
                assert!(
                    board.cpu.instruction_count() - prompt_instructions < 1_500_000_000,
                    "the firmware never polled the keyboard"
                );
                board.run(1_000_000);
            }
            println!(
                "USB keys work {:.1} s after the prompt ({} million instructions)",
                prompt_at.elapsed().as_secs_f64(),
                (board.cpu.instruction_count() - prompt_instructions) / 1_000_000
            );
            board
        }

        fn run(&mut self, count: u64) -> String {
            let stop = self.cpu.run(count);
            assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");
            String::from_utf8_lossy(&self.console.take_output()).into_owned()
        }

        /// Runs until the output has `text` in it, or `cap` instructions have run.
        fn until(&mut self, text: &str, cap: u64) -> String {
            let limit = self.cpu.instruction_count() + cap;
            let mut output = String::new();
            while !output.contains(text) && self.cpu.instruction_count() < limit {
                output.push_str(&self.run(1_000_000));
            }
            assert!(output.contains(text), "no {text:?} in {output:?}");
            output
        }

        fn answer(&mut self, text: &str, keys: &str, cap: u64) {
            self.until(text, cap);
            self.console.push_input(keys.as_bytes());
        }

        /// Runs a slice, as the emulator does between two looks of the window.
        fn tick(&mut self) -> String {
            let output = self.run(SLICE);
            self.keys.sync();
            output
        }

        fn press(&mut self, scancode: Scancode) {
            self.keys.press(usage(scancode).unwrap());
        }

        fn release(&mut self, scancode: Scancode) {
            self.keys.release(usage(scancode).unwrap());
        }

        /// Presses and lets go at once, with no poll between: the firmware has to be sent the
        /// key all the same. Then it runs until the keyboard has sent it, and a little more,
        /// as the firmware types a key a little after it polls.
        fn tap(&mut self, scancode: Scancode) -> String {
            self.press(scancode);
            self.release(scancode);
            let mut output = String::new();
            for _ in 0..20 {
                output += &self.tick();
                if self.keys.report.was_sent() {
                    break;
                }
            }
            assert!(self.keys.report.was_sent(), "the firmware did not poll");
            output += &self.tick();
            output
        }

        /// Taps a key with a modifier held, from a little before it to a little after, as
        /// fingers do: the firmware types the key with the modifiers it has at that point.
        fn tap_with(&mut self, modifier: Scancode, scancode: Scancode) -> String {
            self.press(modifier);
            let mut output = self.tick();
            output += &self.tap(scancode);
            self.release(modifier);
            output += &self.tick();
            output
        }

        /// Types text of letters, digits, space and the few marks used here, with Shift for the
        /// ones that need it, as a UK keyboard does. Returns what the firmware printed.
        fn type_text(&mut self, text: &str) -> String {
            let mut output = String::new();
            for character in text.chars() {
                let (scancode, shift) = match character {
                    'A'..='Z' => (letter(character), true),
                    'a'..='z' => (letter(character.to_ascii_uppercase()), false),
                    ' ' => (Scancode::Space, false),
                    '0'..='9' => (digit(character), false),
                    '"' => (Scancode::Num2, true),
                    '+' => (Scancode::Equals, true),
                    '=' => (Scancode::Equals, false),
                    '$' => (Scancode::Num4, true),
                    '(' => (Scancode::Num9, true),
                    ')' => (Scancode::Num0, true),
                    ':' => (Scancode::Semicolon, true),
                    '<' => (Scancode::Comma, true),
                    '>' => (Scancode::Period, true),
                    _ => panic!("no key for {character:?}"),
                };
                output += &if shift {
                    self.tap_with(Scancode::LShift, scancode)
                } else {
                    self.tap(scancode)
                };
            }
            output
        }

        /// Presses Return and runs to the next prompt. Returns what was printed on the way.
        fn enter(&mut self) -> String {
            let mut output = self.tap(Scancode::Return);
            let limit = self.cpu.instruction_count() + 200_000_000;
            while !output.ends_with(PROMPT) {
                assert!(self.cpu.instruction_count() < limit, "{output:?}");
                output += &self.tick();
            }
            output
        }
    }

    fn digit(digit: char) -> Scancode {
        const DIGITS: [Scancode; 10] = [
            Scancode::Num0,
            Scancode::Num1,
            Scancode::Num2,
            Scancode::Num3,
            Scancode::Num4,
            Scancode::Num5,
            Scancode::Num6,
            Scancode::Num7,
            Scancode::Num8,
            Scancode::Num9,
        ];
        DIGITS[(digit as u8 - b'0') as usize]
    }

    fn letter(upper: char) -> Scancode {
        const LETTERS: [Scancode; 26] = [
            Scancode::A,
            Scancode::B,
            Scancode::C,
            Scancode::D,
            Scancode::E,
            Scancode::F,
            Scancode::G,
            Scancode::H,
            Scancode::I,
            Scancode::J,
            Scancode::K,
            Scancode::L,
            Scancode::M,
            Scancode::N,
            Scancode::O,
            Scancode::P,
            Scancode::Q,
            Scancode::R,
            Scancode::S,
            Scancode::T,
            Scancode::U,
            Scancode::V,
            Scancode::W,
            Scancode::X,
            Scancode::Y,
            Scancode::Z,
        ];
        LETTERS[(upper as u8 - b'A') as usize]
    }

    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
    fn real_firmware_takes_typing_at_the_prompt_over_usb() {
        let mut board = Board::boot();
        board.type_text("PRINT 1+2");
        let answer = board.enter();
        assert!(answer.ends_with("\r\n 3\r\n> "), "{answer:?}");

        // The firmware's layout makes the characters: Shift with 2 is the quote on its UK
        // layout, and Shift with a letter is the capital.
        board.type_text("PRINT \"Ab\"");
        let answer = board.enter();
        assert!(answer.ends_with("\r\nAb\r\n> "), "{answer:?}");
    }

    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
    fn real_firmware_reads_a_held_key_in_a_running_loop_over_usb() {
        let mut board = Board::boot();
        // The loop ends when 'a' is down, and then when no key is.
        board.type_text(
            "DO WHILE KEYDOWN(1)<>97:LOOP:PRINT KEYDOWN(0):DO WHILE KEYDOWN(0)>0:LOOP:PRINT 0",
        );
        board.tap(Scancode::Return);
        // It runs and does not finish with nothing held.
        for _ in 0..5 {
            let output = board.tick();
            assert!(!output.contains(PROMPT), "{output:?}");
        }
        board.press(Scancode::A);
        let mut output = String::new();
        for _ in 0..10 {
            output += &board.tick();
        }
        assert!(output.contains(" 1\r\n"), "{output:?}");
        // Still held, the second loop has not ended.
        assert!(!output.ends_with(PROMPT), "{output:?}");
        board.release(Scancode::A);
        for _ in 0..10 {
            output += &board.tick();
        }
        assert!(output.ends_with("\r\n 0\r\n> "), "{output:?}");
    }

    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
    fn real_firmware_stops_a_running_program_on_ctrl_c_over_usb() {
        let mut board = Board::boot();
        board.type_text("FOR I = 1 TO 1000000000 : NEXT");
        board.tap(Scancode::Return);
        let running = board.tick();
        assert!(!running.contains(PROMPT), "{running:?}");

        let mut stopped = board.tap_with(Scancode::LCtrl, Scancode::C);
        let limit = board.cpu.instruction_count() + 100_000_000;
        while !stopped.ends_with(PROMPT) {
            assert!(board.cpu.instruction_count() < limit, "{stopped:?}");
            stopped += &board.tick();
        }
        board.type_text("PRINT 1+2");
        let answer = board.enter();
        assert!(answer.ends_with("\r\n 3\r\n> "), "{answer:?}");
    }
}
