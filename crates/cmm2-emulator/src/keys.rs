//! SDL key presses as the bytes the firmware's console takes in. These are the bytes a host
//! terminal sends, so the window and `--console` reach the firmware the same way.

use sdl2::keyboard::{Keycode, Mod};

/// The control codes 0x01 to 0x1A, which Ctrl with A to Z sends.
static CONTROL: [u8; 26] = {
    let mut codes = [0; 26];
    let mut at = 0;
    while at < codes.len() {
        codes[at] = at as u8 + 1;
        at += 1;
    }
    codes
};

/// What the console is sent for a key going down, or nothing for a key that has no byte of its
/// own. Printable characters are not here: they come as SDL text input, which has already
/// applied the layout and shift.
///
/// The cursor and function keys are the xterm sequences, except Home and End, which are the
/// VT220 `ESC [ 1 ~` and `ESC [ 4 ~`: measured, the firmware reads those and not xterm's
/// `ESC [ H` and `ESC [ F`. Backspace is 0x08; the firmware ignores 0x7F.
///
/// AltGr is not Ctrl: where SDL reports it as Left Ctrl with Right Alt (as it does on Windows),
/// the key makes a character, which comes as text, and not a control code. Right Alt with Ctrl
/// gives no control code anywhere, as on Linux AltGr is Right Alt alone, and nobody breaks a
/// program with Ctrl and AltGr: Ctrl with Left Alt still does.
pub fn key_bytes(keycode: Keycode, keymod: Mod) -> Option<&'static [u8]> {
    let ctrl = keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD) && !keymod.contains(Mod::RALTMOD);
    if ctrl && let Some(code) = control_code(keycode) {
        return Some(code);
    }
    Some(match keycode {
        Keycode::Return | Keycode::KpEnter => b"\r",
        Keycode::Backspace => b"\x08",
        Keycode::Tab => b"\t",
        Keycode::Escape => b"\x1b",
        Keycode::Up => b"\x1b[A",
        Keycode::Down => b"\x1b[B",
        Keycode::Right => b"\x1b[C",
        Keycode::Left => b"\x1b[D",
        Keycode::Home => b"\x1b[1~",
        Keycode::Insert => b"\x1b[2~",
        Keycode::Delete => b"\x1b[3~",
        Keycode::End => b"\x1b[4~",
        Keycode::PageUp => b"\x1b[5~",
        Keycode::PageDown => b"\x1b[6~",
        Keycode::F1 => b"\x1bOP",
        Keycode::F2 => b"\x1bOQ",
        Keycode::F3 => b"\x1bOR",
        Keycode::F4 => b"\x1bOS",
        Keycode::F5 => b"\x1b[15~",
        Keycode::F6 => b"\x1b[17~",
        Keycode::F7 => b"\x1b[18~",
        Keycode::F8 => b"\x1b[19~",
        Keycode::F9 => b"\x1b[20~",
        Keycode::F10 => b"\x1b[21~",
        Keycode::F11 => b"\x1b[23~",
        Keycode::F12 => b"\x1b[24~",
        _ => return None,
    })
}

/// The control code for Ctrl with a letter key.
fn control_code(keycode: Keycode) -> Option<&'static [u8]> {
    let letter = i32::from(keycode) - i32::from(Keycode::A);
    let letter = usize::try_from(letter).ok()?;
    CONTROL.get(letter..=letter)
}

/// The bytes for typed text. The console is ASCII, so anything else is dropped.
pub fn text_bytes(text: &str) -> Vec<u8> {
    text.bytes()
        .filter(|byte| (0x20..0x7F).contains(byte))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(keycode: Keycode) -> Option<&'static [u8]> {
        key_bytes(keycode, Mod::NOMOD)
    }

    #[test]
    fn the_editing_keys_are_their_control_characters() {
        assert_eq!(plain(Keycode::Return), Some(&b"\r"[..]));
        assert_eq!(plain(Keycode::KpEnter), Some(&b"\r"[..]));
        assert_eq!(plain(Keycode::Backspace), Some(&b"\x08"[..]));
        assert_eq!(plain(Keycode::Tab), Some(&b"\t"[..]));
        assert_eq!(plain(Keycode::Escape), Some(&b"\x1b"[..]));
    }

    #[test]
    fn the_cursor_keys_are_ansi_sequences() {
        assert_eq!(plain(Keycode::Up), Some(&b"\x1b[A"[..]));
        assert_eq!(plain(Keycode::Down), Some(&b"\x1b[B"[..]));
        assert_eq!(plain(Keycode::Right), Some(&b"\x1b[C"[..]));
        assert_eq!(plain(Keycode::Left), Some(&b"\x1b[D"[..]));
        assert_eq!(plain(Keycode::Home), Some(&b"\x1b[1~"[..]));
        assert_eq!(plain(Keycode::End), Some(&b"\x1b[4~"[..]));
        assert_eq!(plain(Keycode::Insert), Some(&b"\x1b[2~"[..]));
        assert_eq!(plain(Keycode::Delete), Some(&b"\x1b[3~"[..]));
        assert_eq!(plain(Keycode::PageUp), Some(&b"\x1b[5~"[..]));
        assert_eq!(plain(Keycode::PageDown), Some(&b"\x1b[6~"[..]));
    }

    #[test]
    fn the_function_keys_are_xterm_sequences() {
        let keys = [
            (Keycode::F1, &b"\x1bOP"[..]),
            (Keycode::F2, b"\x1bOQ"),
            (Keycode::F3, b"\x1bOR"),
            (Keycode::F4, b"\x1bOS"),
            (Keycode::F5, b"\x1b[15~"),
            (Keycode::F6, b"\x1b[17~"),
            (Keycode::F7, b"\x1b[18~"),
            (Keycode::F8, b"\x1b[19~"),
            (Keycode::F9, b"\x1b[20~"),
            (Keycode::F10, b"\x1b[21~"),
            (Keycode::F11, b"\x1b[23~"),
            (Keycode::F12, b"\x1b[24~"),
        ];
        for (keycode, bytes) in keys {
            assert_eq!(plain(keycode), Some(bytes), "{keycode:?}");
        }
    }

    #[test]
    fn ctrl_with_a_letter_is_its_control_code() {
        for keymod in [Mod::LCTRLMOD, Mod::RCTRLMOD] {
            assert_eq!(key_bytes(Keycode::A, keymod), Some(&b"\x01"[..]));
            assert_eq!(key_bytes(Keycode::C, keymod), Some(&b"\x03"[..]));
            assert_eq!(key_bytes(Keycode::Z, keymod), Some(&b"\x1a"[..]));
        }
        // Shift and Ctrl together still make the control code.
        assert_eq!(
            key_bytes(Keycode::C, Mod::LCTRLMOD | Mod::LSHIFTMOD),
            Some(&b"\x03"[..])
        );
    }

    #[test]
    fn altgr_is_not_ctrl_when_it_comes_as_ctrl_with_right_alt() {
        // The characters AltGr makes are text, so the letters have no bytes here.
        assert_eq!(key_bytes(Keycode::E, Mod::LCTRLMOD | Mod::RALTMOD), None);
        assert_eq!(key_bytes(Keycode::Q, Mod::LCTRLMOD | Mod::RALTMOD), None);
        // Left Alt is not AltGr.
        assert_eq!(
            key_bytes(Keycode::C, Mod::LCTRLMOD | Mod::LALTMOD),
            Some(&b"\x03"[..])
        );
        // And the keys that are not letters are what they were.
        assert_eq!(
            key_bytes(Keycode::Left, Mod::LCTRLMOD | Mod::RALTMOD),
            plain(Keycode::Left)
        );
    }

    #[test]
    fn ctrl_does_not_change_a_key_that_is_not_a_letter() {
        assert_eq!(
            key_bytes(Keycode::Left, Mod::LCTRLMOD),
            plain(Keycode::Left)
        );
        assert_eq!(key_bytes(Keycode::Num1, Mod::LCTRLMOD), None);
        assert_eq!(key_bytes(Keycode::Space, Mod::LCTRLMOD), None);
    }

    #[test]
    fn a_letter_alone_is_text_and_has_no_bytes_here() {
        assert_eq!(plain(Keycode::A), None);
        assert_eq!(plain(Keycode::Space), None);
        assert_eq!(key_bytes(Keycode::A, Mod::LSHIFTMOD), None);
        assert_eq!(plain(Keycode::LShift), None);
    }

    #[test]
    fn typed_text_keeps_its_printable_ascii() {
        assert_eq!(text_bytes("a"), b"a");
        assert_eq!(text_bytes("Hi there! ~"), b"Hi there! ~");
        assert_eq!(text_bytes(""), b"");
    }

    #[test]
    fn typed_text_drops_what_the_console_cannot_take() {
        assert_eq!(text_bytes("é"), b"");
        assert_eq!(text_bytes("a£b"), b"ab");
        assert_eq!(text_bytes("\x7f\x1b\r"), b"");
    }
}

/// The bytes `key_bytes` makes, typed into the real firmware: what the window sends is what the
/// firmware's prompt, editor and program break take. They need the firmware and are run with
/// `CMM2_FIRMWARE=<path> cargo test --release -- --ignored`.
#[cfg(test)]
mod real_firmware {
    use super::*;
    use cmm2_core::{Console, Cpu, Firmware, Irqs, Stop, cmm2_memory};

    /// What the firmware prints when it shows the prompt again, which ends every command.
    const PROMPT: &str = "\r\n> ";
    /// The editor clears the screen and prints this as it ends, which leaves the prompt.
    const EDITOR_CLOSED: &str = "t> ";

    /// The firmware booted to its prompt, and the way to type at it.
    struct Board {
        cpu: Cpu,
        console: Console,
    }

    impl Board {
        /// Boots, answers the setup questions and runs through the reset to the prompt.
        fn boot() -> Board {
            let path = std::env::var("CMM2_FIRMWARE")
                .expect("CMM2_FIRMWARE must be set to the path of CMM2_MMBasic_7.00.00b5.bin");
            let firmware = Firmware::load(path).unwrap();
            let irqs = Irqs::new();
            let memory = cmm2_memory(&irqs);
            let console = memory.console();
            let mut board = Board {
                cpu: Cpu::new(&firmware, memory, irqs).unwrap(),
                console,
            };
            board.answer("Keyboard type", "1", 300_000_000);
            board.answer("HH:MM:SS", "12:00:00\r", 20_000_000);
            board.answer("DD/MM/YY", "12/11/26\r", 20_000_000);
            while board.cpu.resets() == 0 {
                board.run(1_000_000);
            }
            board.until(PROMPT, 500_000_000);
            board
        }

        /// Puts a blank FAT32 card in the socket, so a program can be saved. Returns false, with
        /// a note, when there is no `mkfs.vfat` to make one.
        fn insert_card(&mut self, name: &str) -> bool {
            let path =
                std::env::temp_dir().join(format!("cmm2-keys-{}-{name}.img", std::process::id()));
            std::fs::File::create(&path)
                .and_then(|file| file.set_len(64 << 20))
                .unwrap();
            let made = std::process::Command::new("mkfs.vfat")
                .args(["-F", "32"])
                .arg(&path)
                .output();
            let image = made.as_ref().is_ok_and(|made| made.status.success());
            if image {
                let card = std::fs::read(&path).unwrap();
                self.cpu.memory().sd_slot().insert(card).unwrap();
            } else {
                eprintln!("skipped: no mkfs.vfat on this machine");
            }
            std::fs::remove_file(&path).unwrap();
            image
        }

        fn run(&mut self, count: u64) -> String {
            let stop = self.cpu.run(count);
            assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");
            String::from_utf8_lossy(&self.console.take_output()).into_owned()
        }

        /// Runs until the output so far has `text` in it, or `cap` instructions have run. Returns
        /// the output so far, which starts at the last time the output was read.
        fn until(&mut self, text: &str, cap: u64) -> String {
            let limit = self.cpu.instruction_count() + cap;
            let mut output = String::new();
            while !output.contains(text) && self.cpu.instruction_count() < limit {
                output.push_str(&self.run(1_000_000));
            }
            assert!(output.contains(text), "no {text:?} in {output:?}");
            output
        }

        /// Runs for `count` instructions and returns what the firmware printed.
        fn watch(&mut self, count: u64) -> String {
            self.run(count)
        }

        fn answer(&mut self, text: &str, keys: &str, cap: u64) {
            self.until(text, cap);
            self.console.push_input(keys.as_bytes());
        }

        fn type_text(&mut self, text: &str) {
            self.console.push_input(&text_bytes(text));
        }

        fn type_line(&mut self, text: &str) {
            self.type_text(text);
            self.press(Keycode::Return);
        }

        fn press(&mut self, keycode: Keycode) {
            self.press_with(keycode, Mod::NOMOD);
        }

        fn press_with(&mut self, keycode: Keycode, keymod: Mod) {
            let bytes = key_bytes(keycode, keymod).expect("a key with bytes");
            self.console.push_input(bytes);
        }

        fn press_times(&mut self, keycode: Keycode, times: usize) {
            for _ in 0..times {
                self.press(keycode);
            }
        }

        /// Presses Return and runs to the next prompt. Returns what was printed on the way.
        fn enter(&mut self) -> String {
            self.press(Keycode::Return);
            self.until(PROMPT, 100_000_000)
        }
    }

    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
    fn real_firmware_edits_the_line_at_the_prompt() {
        let mut board = Board::boot();

        // Backspace takes the character before the cursor.
        board.type_text("PRINT 1");
        board.press(Keycode::Backspace);
        board.type_text("2");
        assert!(board.enter().ends_with("\r\n 2\r\n> "));

        // Left puts the cursor before the 5, Delete takes it, and the 6 goes in its place.
        board.type_text("PRINT 5");
        board.press(Keycode::Left);
        board.press(Keycode::Delete);
        board.type_text("6");
        assert!(board.enter().ends_with("\r\n 6\r\n> "));

        // Home, Delete takes the stray X off the front; End, then 3 goes on the end.
        board.type_text("XPRINT 12");
        board.press(Keycode::Home);
        board.press(Keycode::Delete);
        board.press(Keycode::End);
        board.type_text("3");
        assert!(board.enter().ends_with("\r\n 123\r\n> "));

        // Up brings back the line before.
        board.type_text("PRINT 7");
        board.enter();
        board.press(Keycode::Up);
        assert!(board.enter().ends_with("\r\n 7\r\n> "));

        // A function key types its string, and F2 runs RUN. F11 and F12 have none, and their
        // sequences must not turn up as text ahead of it.
        board.press(Keycode::F11);
        board.press(Keycode::F12);
        board.press(Keycode::F2);
        assert!(board.until(PROMPT, 100_000_000).starts_with("RUN\r\n"));

        // F1 is ESC O P: the firmware tells it from a lone Escape and runs FM, the file manager.
        board.press(Keycode::F1);
        board.until("FM\r\n", 100_000_000);
    }

    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
    fn real_firmware_stops_a_running_program_on_ctrl_c() {
        let mut board = Board::boot();
        // A loop that would take far longer than the test to end.
        board.type_line("FOR I = 1 TO 1000000000 : NEXT");
        let running = board.watch(30_000_000);
        assert!(!running.contains("> "), "{running:?}");

        board.press_with(Keycode::C, Mod::LCTRLMOD);
        let stopped = board.until(PROMPT, 100_000_000);
        // The loop has ended, and the prompt works again.
        board.type_line("PRINT 1+2");
        assert!(
            board.until(PROMPT, 100_000_000).ends_with("\r\n 3\r\n> "),
            "{stopped:?}"
        );
    }

    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
    fn real_firmware_edits_a_program_in_the_editor() {
        let mut board = Board::boot();
        if !board.insert_card("editor") {
            return;
        }
        // F1 ends AUTOSAVE, and with it the program is saved and in memory.
        board.type_line("AUTOSAVE \"T.BAS\"");
        board.watch(5_000_000);
        board.type_line("PRINT 12345");
        board.watch(5_000_000);
        board.type_line("PRINT 678");
        board.watch(5_000_000);
        board.press(Keycode::F1);
        board.until("Saved to SDcard\r\n> ", 100_000_000);

        // The editor takes a while to draw; the status line says INS or OVR once it has.
        let edit = |board: &mut Board| {
            board.type_line("EDIT");
            board.until("INS,bas", 100_000_000);
        };
        edit(&mut board);
        // Insert flips the editor between inserting and overwriting.
        board.press(Keycode::Insert);
        board.until("OVR,bas", 10_000_000);
        board.press(Keycode::Insert);
        board.until("INS,bas", 10_000_000);
        // Page Up and Page Down are keys the editor knows, so nothing of them is typed. Home
        // and Right put the cursor on the 1, and Delete takes it.
        board.press(Keycode::PageDown);
        board.press(Keycode::PageUp);
        board.press(Keycode::Home);
        board.press_times(Keycode::Right, 6);
        board.press(Keycode::Delete);
        // Down to the next line, End, Left puts the cursor before the 8, and Backspace takes the 7.
        board.press(Keycode::Down);
        board.press(Keycode::End);
        board.press(Keycode::Left);
        board.press(Keycode::Backspace);
        // F1 saves and leaves the editor.
        board.press(Keycode::F1);
        board.until(EDITOR_CLOSED, 100_000_000);
        board.type_text("LIST");
        let listing = board.enter();
        assert!(
            listing.contains("\r\nPRINT 2345\r\nPRINT 68\r\n"),
            "{listing:?}"
        );

        // Escape leaves an unchanged program with no question.
        edit(&mut board);
        board.press(Keycode::Escape);
        board.until(EDITOR_CLOSED, 100_000_000);
        // With a change it asks, and Y with Return throws the change away.
        edit(&mut board);
        board.type_text("9");
        board.press(Keycode::Escape);
        board.until("Exit and discard all changes", 100_000_000);
        board.type_line("Y");
        board.until(EDITOR_CLOSED, 100_000_000);
        board.type_text("LIST");
        let listing = board.enter();
        assert!(listing.contains("\r\nPRINT 2345\r\n"), "{listing:?}");
    }
}
