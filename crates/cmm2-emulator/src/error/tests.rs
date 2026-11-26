use super::*;

#[test]
fn a_failure_of_the_run_comes_before_those_of_the_trace_and_the_save() {
    let _cursor = CURSOR_TEST.lock().unwrap_or_else(|held| held.into_inner());
    take_cursor();
    let err = |text: &str| RunError::Fault(text.to_string());
    let mut said = Vec::new();
    let last = settle(
        Err(err("fault")),
        [Err(err("trace")), Err(err("shot")), Err(err("save"))],
        &mut said,
    )
    .unwrap_err();
    // The last is returned for the caller to say; the others were said, in order, before it.
    assert_eq!(last.to_string(), "save");
    assert_eq!(
        String::from_utf8(said).unwrap(),
        "cmm2-emulator: fault\ncmm2-emulator: trace\ncmm2-emulator: shot\n"
    );

    // With no fault the order is the trace, the screenshot, the save.
    let mut said = Vec::new();
    let last = settle(
        Ok(true),
        [Err(err("trace")), Ok(()), Err(err("save"))],
        &mut said,
    )
    .unwrap_err();
    assert_eq!(last.to_string(), "save");
    assert_eq!(String::from_utf8(said).unwrap(), "cmm2-emulator: trace\n");

    // One error alone is only returned, and none says nothing.
    let mut said = Vec::new();
    let last = settle(Err(err("fault")), [Ok(()), Ok(()), Ok(())], &mut said).unwrap_err();
    assert_eq!(last.to_string(), "fault");
    let last = settle(Ok(true), [Ok(()), Ok(()), Err(err("save"))], &mut said).unwrap_err();
    assert_eq!(last.to_string(), "save");
    assert!(matches!(
        settle(Ok(false), [Ok(()), Ok(()), Ok(())], &mut said),
        Ok(false)
    ));
    assert!(said.is_empty());
}

const CLEAN: Cursor = Cursor {
    away: false,
    written: false,
    escape: Escape::None,
};

#[test]
fn a_line_said_to_a_raw_terminal_ends_in_cr_lf() {
    assert_eq!(said("x", false, CLEAN), "cmm2-emulator: x\n");
    assert_eq!(said("x", true, CLEAN), "cmm2-emulator: x\r\n");
}

#[test]
fn a_line_said_part_way_along_a_line_starts_a_new_one() {
    let mid = CLEAN.after(b"b", false);
    assert_eq!(said("x", false, mid), "\ncmm2-emulator: x\n");
    assert_eq!(said("x", true, mid), "\r\ncmm2-emulator: x\r\n");
}

#[test]
fn the_cursor_is_followed_through_what_the_guest_wrote() {
    // A line ended in CR LF leaves it clean, and a lone CR after that leaves it clean too.
    assert_eq!(CLEAN.after(b"ready\r\n", false), CLEAN);
    assert_eq!(CLEAN.after(b"ready\r\n\r", false), CLEAN);
    // A lone CR after text is back at the start of a row that has text on it.
    assert!(CLEAN.after(b"abc\r", false).written);
    // A raw terminal's lone LF moves down and not to the first column; a cooked one's does.
    let lone = CLEAN.after(b"b\n", false);
    assert_eq!((lone.away, lone.written), (true, false));
    assert_eq!(said("x", true, lone), "\rcmm2-emulator: x\r\n");
    assert_eq!(CLEAN.after(b"b\n", true), CLEAN);
}

#[test]
fn a_colour_change_or_a_title_leaves_the_cursor_where_it_was() {
    assert_eq!(CLEAN.after(b"ok\r\n\x1b[0m", false), CLEAN);
    assert_eq!(CLEAN.after(b"ok\r\n\x1b[1;31m", false), CLEAN);
    assert_eq!(CLEAN.after(b"ok\r\n\x1b]0;title\x07", false), CLEAN);
    assert_eq!(CLEAN.after(b"ok\r\n\x1b]0;title\x1b\\", false), CLEAN);
    // The same in pieces, as the guest's output comes in as it is written.
    let part = CLEAN.after(b"\x1b[1", false).after(b";3", false);
    assert_eq!(part.after(b"1m", false), CLEAN);
}

#[test]
fn an_escape_that_may_move_the_cursor_counts_as_a_row_with_something_on_it() {
    for moved in [&b"\x1b[2J"[..], b"\x1b[10;5H", b"\x1bc", b"\x1b7"] {
        assert!(CLEAN.after(moved, false).written, "{moved:?}");
    }
    // Text after a colour change is text.
    assert!(CLEAN.after(b"\x1b[0mx", false).written);
}

#[test]
fn the_cursor_goes_through_its_bits_and_back() {
    for escape in [
        Escape::None,
        Escape::Seen,
        Escape::Control,
        Escape::Command,
        Escape::CommandEnd,
    ] {
        for (away, written) in [(false, false), (true, false), (false, true), (true, true)] {
            let cursor = Cursor {
                away,
                written,
                escape,
            };
            assert_eq!(Cursor::from_bits(cursor.bits()), cursor);
        }
    }
}

#[test]
fn a_line_said_inside_an_escape_sequence_cancels_it_first() {
    for open in [&b"\x1b"[..], b"\x1b[3", b"\x1b]0;t", b"\x1b]0;t\x1b"] {
        let cursor = CLEAN.after(open, false);
        assert_eq!(
            said("x", true, cursor),
            "\x18\r\ncmm2-emulator: x\r\n",
            "{open:?}"
        );
        assert_eq!(
            said("x", false, cursor),
            "\x18\ncmm2-emulator: x\n",
            "{open:?}"
        );
    }
}

#[test]
fn cancel_and_substitute_end_an_operating_system_command() {
    // What follows them is shown, so it is text.
    for cancel in [0x18, 0x1A] {
        let ended = CLEAN.after(&[0x1B, b']', b't', cancel], false);
        assert_eq!(ended, CLEAN);
        assert!(ended.after(b"a", false).written);
    }
    let ended = CLEAN.after(b"\x1b]t\x1b\x18", false);
    assert_eq!(ended, CLEAN);
}

#[test]
fn an_escape_in_a_command_that_is_not_its_end_is_an_escape_of_its_own() {
    // ESC \ ends it and moves nothing; ESC E does something else after ending it.
    assert_eq!(CLEAN.after(b"\x1b]t\x1b\\", false), CLEAN);
    assert!(CLEAN.after(b"\x1b]t\x1bE", false).written);
    // And ESC [ starts a control sequence, which a colour change then ends.
    assert_eq!(CLEAN.after(b"\x1b]t\x1b[0m", false), CLEAN);
}

#[test]
fn a_write_cut_short_leaves_nothing_known() {
    let _cursor = CURSOR_TEST.lock().unwrap_or_else(|held| held.into_inner());
    note_cut_short();
    assert_eq!(said("x", false, take_cursor()), "\x18\ncmm2-emulator: x\n");
    // It was used up.
    assert_eq!(take_cursor(), CLEAN);
}

// A name is shown to the user, not handed to the terminal: a control character in one must not
// recolour the line a message is said on or break that line in two. A backslash is doubled, so
// that a name cannot imitate one of the escapes this makes.
#[test]
fn a_control_character_in_a_name_is_shown_as_an_escape() {
    // A name that needs nothing is left exactly as it is.
    assert_eq!(
        shown("/tmp/board/settings.flash"),
        "/tmp/board/settings.flash"
    );
    // The three that have a letter of their own, and the rest by their byte.
    assert_eq!(shown("a\nb"), r"a\nb");
    assert_eq!(shown("a\rb"), r"a\rb");
    assert_eq!(shown("a\tb"), r"a\tb");
    assert_eq!(shown("\x1b[31mRED\x1b[0m"), r"\x1B[31mRED\x1B[0m");
    // DEL and the 8-bit control characters, the control sequence introducer of them included.
    assert_eq!(shown("\u{7f}"), r"\x7F");
    assert_eq!(shown("\u{9b}31m"), r"\x9B31m");
    assert_eq!(shown(r"a\nb"), r"a\\nb");
    assert_eq!(shown(r"a\x1Bb"), r"a\\x1Bb");
    // Nothing else is changed, a name in another alphabet included.
    assert_eq!(shown("/tmp/ünï—✓.flash"), "/tmp/ünï—✓.flash");
}

// A name with a line feed and a colour change in it: what a file may be called, and what a refusal
// must never act on. The rule it is tested against is `shown`'s: a name is escaped for display
// exactly once, by the `Display` or `say` that puts it in words.
use crate::test_support::{HOSTILE_NAME as HOSTILE, HOSTILE_SHOWN as SAFE};

/// What an arm of `RunError`, or of `BoardError`, does with the names it is handed.
#[derive(Debug, PartialEq)]
enum Says {
    /// Its arm says a path, in a field of its own or in words it borrows. A sample of it is one
    /// escape case, named this.
    APath(&'static str),
    /// `RunError::Board`, whose names are the board's arms': every board arm that says a path is an
    /// escape case whose name starts here.
    ABoardPath(&'static str),
    /// Its arm says no path: an address, a syscall's own words, or a word of the core's. Its
    /// plain sample carries `HOSTILE` nowhere: a name in it would fail the `Says::Nothing`
    /// assertion of `every_variant_that_names_a_path_has_an_escape_case` even when the arm
    /// escapes correctly. That assertion therefore sees a mis-classified variant whose own
    /// plain sample carries a name, and nothing else; a name reaching the line through a
    /// nested error's words is planted and checked by
    /// `no_message_says_the_words_it_borrows_with_a_control_character`.
    Nothing,
}

/// A syscall's refusal with nothing in its words but its own.
fn io_err() -> io::Error {
    io::Error::other("refused")
}

/// A syscall's refusal with the hostile name inside its own words: the shape a reason from
/// `saveable` or a wrapped path failure arrives in.
fn named_io_err() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("{HOSTILE} could not be reached"),
    )
}

/// The samples that say a name have `HOSTILE` in them by a convention this set cannot re-check
/// for its neighbours, so the count of each is pinned once, here, for both sample sets.
const RUN_ERRORS: usize = 26;
const BOARD_ERRORS: usize = 10;

/// One value of every variant of `RunError`, with `HOSTILE` planted in every slot that can hold
/// borrowed words: the message of a nested `io::Error`, a reason, a string of the core's. Where
/// the plain sample already carries the name — in the arm's own field or in the words it borrows —
/// the value is the same one. The set exists because a `Says::Nothing` plain sample must carry no
/// name (see its doc), so an arm that says a nested error's words raw puts any name in them on the
/// line with nothing the plain set can see. `no_message_says_the_words_it_borrows_with_a_control_
/// character` renders this set; a new variant of `RunError` needs a value here, and that value
/// must hold `HOSTILE` in every slot of it that can hold words, whatever its arm escapes today.
fn every_variant_with_named_words() -> Vec<(&'static str, RunError)> {
    use cmm2_core::{B5, PersistentSizeError};
    let name = HOSTILE;
    vec![
        (
            "firmware",
            RunError::Firmware(FirmwareError::Io {
                path: name.into(),
                source: named_io_err(),
            }),
        ),
        // No slot in either of these can hold words: an address, and a fixed sentence.
        (
            "memory",
            RunError::Memory(MemoryError::Unmapped(0x2000_0000)),
        ),
        (
            "cpu",
            RunError::Cpu(CpuError::Reset(format!("{name} is not the stack top"))),
        ),
        (
            "trace",
            RunError::Trace {
                path: name.into(),
                source: named_io_err(),
            },
        ),
        ("write", RunError::Write(named_io_err())),
        ("console", RunError::Console(named_io_err())),
        (
            "window",
            RunError::Window(crate::window::WindowError::for_tests(&format!(
                "SDL could not open the video device for {name}"
            ))),
        ),
        ("frame", RunError::Frame(FrameError::Off)),
        (
            "screenshot",
            RunError::Screenshot {
                path: name.into(),
                source: named_io_err(),
            },
        ),
        // The plain sample already carries the name in the borrowed words of `source`.
        (
            "flash",
            RunError::Flash {
                path: name.to_string(),
                source: io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "its directory {name} does not exist, so the run could not save its \
                         flash and clock"
                    ),
                ),
            },
        ),
        (
            "flash size",
            RunError::FlashSize {
                path: name.into(),
                source: PersistentError::Size(PersistentSizeError {
                    got: 1,
                    profile: &B5,
                }),
            },
        ),
        (
            "settings",
            RunError::Settings {
                path: name.into(),
                source: Box::new(SettingsError::Corrupt(format!(
                    "{name} does not read as a settings file"
                ))),
                backups: vec![format!("{name}.v1"), format!("{name}.v1.2")],
            },
        ),
        (
            "v2 in a raw run",
            RunError::V2InRawRun { path: name.into() },
        ),
        ("backup not a file", {
            RunError::BackupNotAFile {
                path: name.into(),
                backup: format!("{name}.v1"),
            }
        }),
        ("backups full", {
            RunError::BackupsFull {
                path: name.into(),
                first: format!("{name}.v1"),
                last: format!("{name}.v1.99"),
            }
        }),
        ("backups race", {
            RunError::BackupRaceLost {
                path: name.into(),
                tries: 99,
            }
        }),
        ("needs a hard link", {
            RunError::BackupNeedsLink {
                path: name.into(),
                backup: format!("{name}.v1"),
                source: named_io_err(),
            }
        }),
        ("did not read back", {
            RunError::NotReadBack {
                path: name.into(),
                reason: format!("{name} is not what was written"),
            }
        }),
        // `NoLayout`'s words are fixed; the name is only in the arm's own field.
        ("no layout", {
            RunError::NoLayout {
                path: name.into(),
                source: NoLayout,
            }
        }),
        (
            "board no layout",
            RunError::BoardNoLayout { path: name.into() },
        ),
        // Here the variable's value is the planted name, unlike the plain set's `b4`.
        (
            "bad layout name",
            RunError::BadLayoutName {
                variable: "CMM2_BOARD_LAYOUT_FOR_TESTS".to_string(),
                name: name.to_string(),
            },
        ),
        (
            "sd",
            RunError::Sd {
                path: name.into(),
                source: named_io_err(),
            },
        ),
        (
            "sd size",
            RunError::SdSize {
                path: name.into(),
                source: ImageSizeError(7),
            },
        ),
        (
            "sd too small",
            RunError::SdTooSmall {
                path: name.into(),
                size: 1,
            },
        ),
        (
            // The board's samples are checked by the same test, on `every_board_variant`.
            "board",
            RunError::Board(BoardError::Io {
                path: name.into(),
                source: named_io_err(),
            }),
        ),
        (
            "fault",
            RunError::Fault(format!("the guest faulted at {name}")),
        ),
    ]
}

/// One value of every variant of `RunError`, in the order the enum declares them. The count is
/// pinned in `every_variant_that_names_a_path_has_an_escape_case`, so a variant of `RunError` that
/// is not sampled here fails that test, and one that is not classified in `says_a_path` stops the
/// test crate from compiling at all.
fn every_variant() -> Vec<(&'static str, RunError)> {
    use cmm2_core::{B5, PersistentSizeError};
    let name = HOSTILE;
    vec![
        (
            "firmware",
            RunError::Firmware(FirmwareError::Io {
                path: name.into(),
                source: io_err(),
            }),
        ),
        (
            "memory",
            RunError::Memory(MemoryError::Unmapped(0x2000_0000)),
        ),
        (
            "cpu",
            RunError::Cpu(CpuError::Reset("the stack top is not in RAM".to_string())),
        ),
        (
            "trace",
            RunError::Trace {
                path: name.into(),
                source: io_err(),
            },
        ),
        ("write", RunError::Write(io_err())),
        ("console", RunError::Console(io_err())),
        (
            "window",
            RunError::Window(crate::window::WindowError::for_tests(
                "SDL could not open the video device",
            )),
        ),
        ("frame", RunError::Frame(FrameError::Off)),
        (
            "screenshot",
            RunError::Screenshot {
                path: name.into(),
                source: io_err(),
            },
        ),
        (
            "flash",
            RunError::Flash {
                path: name.to_string(),
                // The shape `saveable::why_path_failed` makes for a `--flash` file whose directory
                // is not there, and `storage::load_flash` wraps in an `io::Error`: the name arrives
                // in the borrowed words as well as in the arm's own field.
                source: io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "its directory {name} does not exist, so the run could not save its \
                         flash and clock"
                    ),
                ),
            },
        ),
        (
            "flash size",
            RunError::FlashSize {
                path: name.into(),
                source: PersistentError::Size(PersistentSizeError {
                    got: 1,
                    profile: &B5,
                }),
            },
        ),
        (
            "settings",
            RunError::Settings {
                path: name.into(),
                source: Box::new(SettingsError::Corrupt("the tail is short".to_string())),
                backups: vec![format!("{name}.v1"), format!("{name}.v1.2")],
            },
        ),
        (
            "v2 in a raw run",
            RunError::V2InRawRun { path: name.into() },
        ),
        ("backup not a file", {
            RunError::BackupNotAFile {
                path: name.into(),
                backup: format!("{name}.v1"),
            }
        }),
        ("backups full", {
            RunError::BackupsFull {
                path: name.into(),
                first: format!("{name}.v1"),
                last: format!("{name}.v1.99"),
            }
        }),
        ("backups race", {
            RunError::BackupRaceLost {
                path: name.into(),
                tries: 99,
            }
        }),
        ("needs a hard link", {
            RunError::BackupNeedsLink {
                path: name.into(),
                backup: format!("{name}.v1"),
                source: io_err(),
            }
        }),
        ("did not read back", {
            RunError::NotReadBack {
                path: name.into(),
                reason: "the checksum is wrong".to_string(),
            }
        }),
        ("no layout", {
            RunError::NoLayout {
                path: name.into(),
                source: NoLayout,
            }
        }),
        (
            "board no layout",
            RunError::BoardNoLayout { path: name.into() },
        ),
        // The value of a variable rather than a path, and a plain one here: its escaping is
        // pinned by `a_bad_layout_name_escapes_the_value_it_was_handed`.
        (
            "bad layout name",
            RunError::BadLayoutName {
                variable: "CMM2_BOARD_LAYOUT_FOR_TESTS".to_string(),
                name: "b4".to_string(),
            },
        ),
        (
            "sd",
            RunError::Sd {
                path: name.into(),
                source: io_err(),
            },
        ),
        (
            "sd size",
            RunError::SdSize {
                path: name.into(),
                source: ImageSizeError(7),
            },
        ),
        (
            "sd too small",
            RunError::SdTooSmall {
                path: name.into(),
                size: 1,
            },
        ),
        (
            // The board's own errors are sampled by `every_board_variant`.
            "board",
            RunError::Board(BoardError::NothingBurned(name.into())),
        ),
        (
            "fault",
            RunError::Fault("the guest read outside its memory".to_string()),
        ),
    ]
}

/// One value of every variant of `BoardError`, in the order the enum declares them. `NothingBurned`
/// and `NotUtf8` are given a name that is valid UTF-8 here: what their arm does with it is the same
/// whatever made it invalid, and the unreadable bytes themselves are pinned in `board::tests`. The
/// board has no second sample set: the test of the words an arm borrows
/// (`no_message_says_the_words_it_borrows_with_a_control_character`) renders this one, so every
/// slot whose words the arm escapes carries the hostile name in them (`Io` and `NoLock`, in
/// `source`), and a new variant adds its value here, under that rule.
fn every_board_variant() -> Vec<(&'static str, BoardError)> {
    let name = HOSTILE;
    vec![
        ("nowhere to keep it", BoardError::NoHome),
        ("nothing burned", BoardError::NothingBurned(name.into())),
        ("home set to nothing", BoardError::EmptyHome),
        ("not utf-8", BoardError::NotUtf8(name.into())),
        ("busy", BoardError::Busy(name.into())),
        (
            "io",
            BoardError::Io {
                path: name.into(),
                // As `board::write_file` wraps `saveable`'s reason: the name arrives in the words
                // of the `io::Error` as well as in the arm's own field.
                source: io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("{name} is not a directory, so the board could not remember the card"),
                ),
            },
        ),
        (
            "the lock",
            BoardError::NoLock {
                path: name.into(),
                // The arm says these words, so the sample carries the name in them like `Io`'s.
                // In production this slot holds a lock attempt's own error and names no file
                // (`board.rs`: `try_lock_exclusive`); the name is planted so the net over this
                // set can see the route.
                source: named_io_err(),
                read_only: false,
            },
        ),
        ("the card record", BoardError::CardRecord(name.into())),
        ("a card's name", BoardError::CardName(name.to_string())),
        (
            // The words of the card's own refusal, as `about_the_remembered_card` hands them over
            // after that arm has escaped the name once.
            "the card said",
            BoardError::Card(
                RunError::Sd {
                    path: name.to_string(),
                    source: io::Error::from(io::ErrorKind::NotFound),
                }
                .to_string(),
            ),
        ),
    ]
}

/// Whether the arm for `err` says a path, and under what name its escape case goes. The match is
/// exhaustive and has no wildcard: a variant added to `RunError` is a compile error here until it
/// is classified, which is what stops a new arm that carries a name going unnoticed.
fn says_a_path(err: &RunError) -> Says {
    match err {
        RunError::Firmware(_) => Says::APath("firmware"),
        RunError::Memory(_) => Says::Nothing,
        RunError::Cpu(_) => Says::Nothing,
        RunError::Trace { .. } => Says::APath("trace"),
        RunError::Write(_) => Says::Nothing,
        RunError::Console(_) => Says::Nothing,
        RunError::Window(_) => Says::Nothing,
        RunError::Frame(_) => Says::Nothing,
        RunError::Screenshot { .. } => Says::APath("screenshot"),
        RunError::Flash { .. } => Says::APath("flash"),
        RunError::FlashSize { .. } => Says::APath("flash size"),
        RunError::Settings { .. } => Says::APath("settings"),
        RunError::V2InRawRun { .. } => Says::APath("v2 in a raw run"),
        RunError::BackupNotAFile { .. } => Says::APath("backup not a file"),
        RunError::BackupsFull { .. } => Says::APath("backups full"),
        RunError::BackupRaceLost { .. } => Says::APath("backups race"),
        RunError::BackupNeedsLink { .. } => Says::APath("needs a hard link"),
        RunError::NotReadBack { .. } => Says::APath("did not read back"),
        RunError::NoLayout { .. } => Says::APath("no layout"),
        RunError::BoardNoLayout { .. } => Says::APath("board no layout"),
        RunError::BadLayoutName { .. } => Says::Nothing,
        RunError::Sd { .. } => Says::APath("sd"),
        RunError::SdSize { .. } => Says::APath("sd size"),
        RunError::SdTooSmall { .. } => Says::APath("sd too small"),
        RunError::Board(_) => Says::ABoardPath("board: "),
        RunError::Fault(_) => Says::Nothing,
    }
}

/// The same for the board's arms, which say the names themselves and reach the line through
/// `RunError::Board`.
fn says_a_board_path(err: &BoardError) -> Says {
    match err {
        BoardError::NoHome => Says::Nothing,
        BoardError::NothingBurned(_) => Says::APath("nothing burned"),
        BoardError::EmptyHome => Says::Nothing,
        BoardError::NotUtf8(_) => Says::APath("not utf-8"),
        BoardError::Busy(_) => Says::APath("busy"),
        BoardError::Io { .. } => Says::APath("io"),
        BoardError::NoLock { .. } => Says::APath("the lock"),
        BoardError::CardRecord(_) => Says::APath("the card record"),
        BoardError::CardName(_) => Says::APath("a card's name"),
        BoardError::Card(_) => Says::APath("the card said"),
    }
}

/// The escape cases: every sample of `RunError` whose arm says a path, and every one of the board's
/// with `board: ` in front of its name. Nothing else has to be added to this: an arm that says a
/// path and is not here fails `every_variant_that_names_a_path_has_an_escape_case`.
fn path_cases() -> Vec<(String, RunError)> {
    let mut cases: Vec<(String, RunError)> = every_variant()
        .into_iter()
        .filter(|(what, err)| says_a_path(err) == Says::APath(what))
        .map(|(what, err)| (what.to_string(), err))
        .collect();
    cases.extend(
        every_board_variant()
            .into_iter()
            .filter(|(what, err)| says_a_board_path(err) == Says::APath(what))
            .map(|(what, err)| (format!("board: {what}"), RunError::Board(err))),
    );
    cases
}

#[test]
fn no_message_says_a_path_with_a_control_character_unfiltered() {
    let cases = path_cases();
    for (what, err) in &cases {
        let said = err.to_string();
        assert!(
            !said.chars().any(char::is_control),
            "{what} said a control character: {said:?}"
        );
        // The name is still in the message, and still one name: the escapes are visible text.
        assert!(said.contains(SAFE), "{what}: {said}");
    }
    // The copies beside the file are listed shown too, and the way out of a v2 file is a command,
    // so it is quoted around the escapes rather than broken by them.
    let settings = RunError::Settings {
        path: HOSTILE.to_string(),
        source: Box::new(SettingsError::Corrupt("x".to_string())),
        backups: vec![format!("{HOSTILE}.v1")],
    }
    .to_string();
    assert_eq!(
        settings.matches(SAFE).count(),
        3,
        "the file, its copy, and the way out: {settings}"
    );
    let advice = RunError::V2InRawRun {
        path: HOSTILE.to_string(),
    }
    .to_string();
    assert!(
        advice.contains(&format!("--flash '{SAFE}', with")),
        "{advice}"
    );
    assert!(
        advice.ends_with(&format!("move '{SAFE}' away.")),
        "{advice}"
    );
    // Two of the messages are pinned as they render and not just as free of control characters,
    // because both were escaped twice over until this round: `CardName` said its name with `{:?}`
    // and the board's arm escaped that again, and `Card` holds the words of a card error that its
    // own arm escaped once. A name escaped twice reads as two backslashes where the file has one,
    // and the line end that `CardName` is about stopped reading as a line end.
    let rendered = |what: &str| {
        cases
            .iter()
            .find(|(name, _)| name == what)
            .unwrap_or_else(|| panic!("no case called {what}"))
            .1
            .to_string()
    };
    assert_eq!(
        rendered("board: a card's name"),
        format!(
            "{SAFE}: a card's name cannot end in a line end, which the board's record of it could \
             not keep"
        )
    );
    assert_eq!(
        rendered("board: the card said"),
        format!(
            "the card in the socket cannot be used: {SAFE}: entity not found. Give another with \
             --sd <file>, or take it out with --no-sd"
        )
    );
}

// The completeness of the escape cases is what a new arm slips past, and this is what stops that.
// `every_variant` and `every_board_variant` hold one value of every variant, and `says_a_path` and
// `says_a_board_path` classify every value by a match with no wildcard. So a variant added to
// either enum is a compile error until it is classified, fails one of the two counts until it has
// a sample, and, once sampled and said to say a path, becomes an escape case by itself. A new arm
// that carries a name cannot therefore be nobody's test, which is how `Flash` went unchecked for
// two rounds: it was in the list because somebody had thought of it, and its name in another
// error's words was not something the list could see. A variant classified as saying nothing while
// its own plain sample carries a name is caught too: the `Says::Nothing` assertions below fail on
// a line that renders it. What they cannot catch is a name that arrives only inside the words of
// a nested error, because a `Says::Nothing` plain sample must carry no name at all — putting one
// in would fail the same assertion on a correct arm. That route is what
// `no_message_says_the_words_it_borrows_with_a_control_character` plants and checks.
#[test]
fn every_variant_that_names_a_path_has_an_escape_case() {
    assert_eq!(
        every_variant().len(),
        RUN_ERRORS,
        "one sample for each variant of RunError, and this count with it"
    );
    assert_eq!(
        every_board_variant().len(),
        BOARD_ERRORS,
        "one sample for each variant of BoardError, and this count with it"
    );
    let cases = path_cases();
    let named = |what: &str| cases.iter().any(|(name, _)| name == what);
    for (what, err) in every_variant() {
        match says_a_path(&err) {
            Says::APath(case) => assert!(
                named(case),
                "{what} says a path and has no escape case: add it to `path_cases` by adding the \
                 name to its sample in `every_variant`"
            ),
            Says::ABoardPath(prefix) => assert!(
                cases.iter().any(|(name, _)| name.starts_with(prefix)),
                "{what} hands its names to the board's arms, and none of them is tested"
            ),
            Says::Nothing => {
                assert!(
                    !named(what),
                    "{what} says no path, so why is it an escape case?"
                );
                // What this can see is a plain sample that carries `HOSTILE` in one of its own
                // fields while the variant is classified as saying nothing. A `Says::Nothing`
                // plain sample carries no name, so a name reaching the line through a nested
                // error's words is out of reach here: the named-words test plants it in those
                // words and checks that rendering.
                let said = err.to_string();
                assert!(
                    !said.contains(HOSTILE) && !said.contains(SAFE),
                    "{what} is said to say no path, and says one: {said:?}"
                );
            }
        }
    }
    for (what, err) in every_board_variant() {
        match says_a_board_path(&err) {
            Says::APath(case) => assert!(
                named(&format!("board: {case}")),
                "the board's {what} says a path and has no escape case"
            ),
            Says::ABoardPath(_) => unreachable!("no arm of the board's hands over to the board"),
            Says::Nothing => {
                assert!(
                    !named(&format!("board: {what}")),
                    "the board's {what} says no path, so why is it an escape case?"
                );
                // The same net as the loop above, for the board's arms.
                let said = err.to_string();
                assert!(
                    !said.contains(HOSTILE) && !said.contains(SAFE),
                    "the board's {what} is said to say no path, and says one: {said:?}"
                );
            }
        }
    }
}

// A name can reach a message through the words of a nested error rather than through a field the
// arm says itself, and the plain samples cannot carry that name (see `Says::Nothing`). This is
// the net for that route: every variant rendered with `HOSTILE` in every slot of caller's words
// the arm borrows and escapes, and no rendering of it may put a control character on the line. An
// arm that says such words raw fails it whatever the variant is classified as, because the fault
// itself puts the hostile bytes on the line. What it cannot see is a nested error whose words no
// caller can affect: an address, a size or a fixed sentence (`MemoryError`, `FrameError`,
// `PersistentError`, `PersistentSizeError`, `OffsetError`, `ImageSizeError`, `NoLayout`, in
// `cmm2-core`) carries nothing a name could ride on, so an arm that says it raw neither fails this
// net nor leaks a name. That exemption rests on `cmm2-core`, not on me: those types hold no
// caller's words today, and `FrameError::Unsupported` holds only literals, by construction
// discipline. A future variant carrying a caller's string would restore the hole with this net
// still green on it. The board's plain set is already its named-words set — every slot of its
// samples whose words the arm escapes holds `HOSTILE` in them today, and `NoHome` and `EmptyHome`
// have no such slot — so its loop rides `every_board_variant`; a variant added there must keep
// that rule, which is a visible choice in its sample line. `Card` is the one slot apart from that
// rule: production hands it words escaped once already (see its sample), so it carries the name
// escaped and its arm says them as they are; an arm that escaped them again fails
// `no_message_says_a_path_with_a_control_character_unfiltered` first at the assert that the name
// is still on the line, and then at its exact renderings.
#[test]
fn no_message_says_the_words_it_borrows_with_a_control_character() {
    let named = every_variant_with_named_words();
    assert_eq!(
        named.len(),
        RUN_ERRORS,
        "one named-words value for each variant of RunError, and this count with it"
    );
    assert_eq!(
        named.iter().map(|(what, _)| *what).collect::<Vec<_>>(),
        every_variant()
            .iter()
            .map(|(what, _)| *what)
            .collect::<Vec<_>>(),
        "the named-words set covers the same variants, in the same order, as the plain one"
    );
    for (what, err) in named {
        let said = err.to_string();
        assert!(
            !said.chars().any(char::is_control),
            "{what} said a control character: {said:?}"
        );
    }
    assert_eq!(
        every_board_variant().len(),
        BOARD_ERRORS,
        "one sample for each variant of BoardError, and this count with it"
    );
    for (what, err) in every_board_variant() {
        let said = RunError::Board(err).to_string();
        assert!(
            !said.chars().any(char::is_control),
            "the board's {what} said a control character: {said:?}"
        );
    }
}

// The advice to run `--flash {path}` is a command, so a path that the shell would split or
// expand is quoted, and one that it would not is left as it is.
#[test]
fn the_v2_in_raw_run_advice_quotes_a_path_that_needs_it() {
    let said = |path: &str| {
        RunError::V2InRawRun {
            path: path.to_string(),
        }
        .to_string()
    };

    assert!(said("/tmp/board/mine.flash").contains("--flash /tmp/board/mine.flash, with"));
    assert!(said("/tmp/my board/a.flash").contains("--flash '/tmp/my board/a.flash', with"));
    assert!(said("/tmp/it's.flash").contains(r"--flash '/tmp/it'\''s.flash', with"));
    assert!(said("/tmp/a$b.flash").contains("--flash '/tmp/a$b.flash', with"));
    // The advice to move the file is a command too.
    assert!(said("/tmp/my board/a.flash").ends_with("move '/tmp/my board/a.flash' away."));
    assert!(said("/tmp/board/mine.flash").ends_with("move /tmp/board/mine.flash away."));
    // A relative path that starts with a dash would be taken for an option.
    let dash = said("-x.flash");
    assert!(dash.contains("--flash ./-x.flash, with"), "{dash}");
    assert!(dash.ends_with("move ./-x.flash away."), "{dash}");
    assert!(said("-a b.flash").ends_with("move './-a b.flash' away."));
    // A backslash and a quote: the command has to name the file that exists, so it holds the
    // file's own single backslash, not the two the display shows, and the quote is closed,
    // escaped and opened again.
    let mixed = said(r"/tmp/back\slash'q.flash");
    assert!(
        mixed.contains(r"--flash '/tmp/back\slash'\''q.flash', with"),
        "{mixed}"
    );
    assert!(
        mixed.ends_with(r"move '/tmp/back\slash'\''q.flash' away."),
        "{mixed}"
    );
    // The display still doubles the backslash, so a name cannot imitate one of `shown`'s escapes.
    assert!(mixed.starts_with(r"/tmp/back\\slash'q.flash: "), "{mixed}");
}

// The clause that offers to delete the settings file hands the reader a command, so the name it
// offers goes through `command_quoted` like the advice of `V2InRawRun`: one word, that names the
// file which exists. The display at the head of the message stays escaped.
#[test]
fn the_settings_delete_clause_quotes_the_name_it_offers() {
    let said = |path: &str| {
        RunError::Settings {
            path: path.to_string(),
            source: Box::new(SettingsError::Corrupt("x".to_string())),
            backups: vec![format!("{path}.v1")],
        }
        .to_string()
    };
    assert!(
        said("/tmp/board/settings.flash")
            .ends_with("Or start fresh by deleting /tmp/board/settings.flash.")
    );
    assert!(
        said("/tmp/my board/settings.flash")
            .ends_with("Or start fresh by deleting '/tmp/my board/settings.flash'.")
    );
    // A backslash: the command holds the file's own, not the two the display shows.
    let back = said(r"/tmp/back\slash.flash");
    assert!(
        back.ends_with(r"Or start fresh by deleting '/tmp/back\slash.flash'."),
        "{back}"
    );
    assert!(back.starts_with(r"/tmp/back\\slash.flash: "), "{back}");
    // A quote: closed, escaped, opened again, and the head says the name for a human.
    let quoted = said("/tmp/it's.flash");
    assert!(
        quoted.ends_with(r"Or start fresh by deleting '/tmp/it'\''s.flash'."),
        "{quoted}"
    );
    assert!(quoted.starts_with("/tmp/it's.flash: "), "{quoted}");
}

// The value of `CMM2_BOARD_LAYOUT_FOR_TESTS` can hold anything a file name can, and said raw it
// broke the line and recoloured it: measured with the real binary and `--burn` of an image the
// board cannot identify, a value holding a line feed and a colour change gave two line feeds and
// two escape bytes on standard error. It is escaped like every other user-supplied string.
#[test]
fn a_bad_layout_name_escapes_the_value_it_was_handed() {
    let said = RunError::BadLayoutName {
        variable: "CMM2_BOARD_LAYOUT_FOR_TESTS".to_string(),
        name: HOSTILE.to_string(),
    }
    .to_string();
    assert!(!said.chars().any(char::is_control), "{said:?}");
    assert_eq!(
        said,
        format!("CMM2_BOARD_LAYOUT_FOR_TESTS is \"{SAFE}\", not b5 or rc6")
    );
    // A value that needs nothing is left exactly as it is, which is what the four tests that pin
    // this sentence rely on: they all give `8`.
    assert_eq!(
        RunError::BadLayoutName {
            variable: "V".to_string(),
            name: "8".to_string(),
        }
        .to_string(),
        "V is \"8\", not b5 or rc6"
    );
}
