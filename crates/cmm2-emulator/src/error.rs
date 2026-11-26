//! What can end a run in an error, and how the errors of one run are said.

use crate::board::BoardError;
use crate::output::Output;
use crate::window::WindowError;
use cmm2_core::{
    CpuError, FirmwareError, FrameError, ImageSizeError, MIN_IMAGE_SIZE, MemoryError, NoLayout,
    PersistentError, SettingsError,
};
use std::borrow::Cow;
use std::fmt;
use std::io;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug)]
pub(crate) enum RunError {
    Firmware(FirmwareError),
    Memory(MemoryError),
    Cpu(CpuError),
    Trace {
        path: String,
        source: io::Error,
    },
    Write(io::Error),
    Console(io::Error),
    Window(WindowError),
    /// There is no screen to take a picture of.
    Frame(FrameError),
    Screenshot {
        path: String,
        source: io::Error,
    },
    /// The `--flash` file could not be read or written.
    Flash {
        path: String,
        source: io::Error,
    },
    /// The `--flash` file is not the size of the saved state.
    FlashSize {
        path: String,
        source: PersistentError,
    },
    /// The settings file was refused as v2 (issue 82): too new, saved for another firmware,
    /// larger than any settings file is, or corrupt. `backups` lists the copies of older settings
    /// that sit beside it, which is worth saying only when there are any to restore.
    Settings {
        path: String,
        source: Box<SettingsError>,
        backups: Vec<String>,
    },
    /// A run that reads the raw layout was given a board's v2 file (issue 82): the file is
    /// fine, the run is the wrong kind.
    V2InRawRun {
        path: String,
    },
    /// A migration refused because the name its copy of the old settings goes to is held by
    /// something that is not a plain file (issue 82): it is neither followed nor replaced.
    BackupNotAFile {
        path: String,
        backup: String,
    },
    /// A migration refused because every name for a copy of the old settings is taken, from
    /// `first` to `last` (issue 82).
    BackupsFull {
        path: String,
        first: String,
        last: String,
    },
    /// A migration refused because the name it chose for a copy of the old settings was taken
    /// again after every try, `tries` of them (issue 82). None of the names need be taken: this is
    /// a race lost over and over, and it says so rather than that the cascade is full.
    BackupRaceLost {
        path: String,
        tries: usize,
    },
    /// A migration refused because the file system would not make the hard link that a copy of
    /// the old settings is made with (issue 82). Nothing was written.
    BackupNeedsLink {
        path: String,
        backup: String,
        source: io::Error,
    },
    /// A migration refused because the new settings file it made did not read back (issue 82):
    /// a fault of the encoder, found before any file was touched.
    NotReadBack {
        path: String,
        reason: String,
    },
    /// The firmware has no settings layout, so a settings file for it would be wrong data. `path`
    /// is the file that asked for one.
    NoLayout {
        path: String,
        source: NoLayout,
    },
    /// The firmware on the board has no settings layout; `path` is its `firmware.bin`.
    BoardNoLayout {
        path: String,
    },
    /// The variable names no layout I know.
    BadLayoutName {
        variable: String,
        name: String,
    },
    /// The `--sd` file could not be read or written.
    Sd {
        path: String,
        source: io::Error,
    },
    /// The `--sd` file is not a whole number of blocks.
    SdSize {
        path: String,
        source: ImageSizeError,
    },
    /// The `--sd` file is too small for a card whose CSD can say its size.
    SdTooSmall {
        path: String,
        size: usize,
    },
    /// The board's directory, its lock, or what it remembers.
    Board(BoardError),
    /// The guest faulted; the text is the core's description.
    Fault(String),
}

/// How a name gets into a message, in one sentence: a name is escaped for display exactly once, by
/// the `Display` or `say` that puts it in words, and a name offered as a command goes through
/// `command_quoted` instead: a string is either what a human reads or what the shell is handed,
/// never both.
///
/// Three follow from that. A message built out of another message in this crate escapes only what
/// it adds itself, because the inner one escaped what it added (`BoardError::Card` holds words
/// escaped once already). The words of an `io::Error` and of another crate's error are never
/// escaped where they are made — a syscall's own message, or a path `saveable` put in a reason —
/// so the arm that says them escapes them. And a line said to a terminal carries no control
/// character, so a command quoting a name that holds one quotes the escaped form, which has to be
/// retyped either way; an ordinary name, a backslash included, is quoted as it is so that the
/// command names the file that exists.
///
/// `text` as it is safe to put in a message: every control character is written out where it
/// stands, `\n`, `\r` and `\t` by name and the rest by their byte (`\x1B`), and a backslash as two
/// so that a name cannot imitate one of those. Text that needs nothing is returned as it is. A
/// file may be called anything its file system takes, and a message that said those bytes raw
/// would let the name recolour the terminal the message is read in, or break the line it is on and
/// forge one of its own behind the program's name.
pub(crate) fn shown(text: &str) -> Cow<'_, str> {
    if !text.chars().any(|c| c.is_control() || c == '\\') {
        return Cow::Borrowed(text);
    }
    let mut safe = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '\\' => safe.push_str(r"\\"),
            '\n' => safe.push_str(r"\n"),
            '\r' => safe.push_str(r"\r"),
            '\t' => safe.push_str(r"\t"),
            // `is_control` is the C0 and C1 ranges and DEL, every one of them at or below 0x9F, so
            // two hex digits say any of them.
            c if c.is_control() => safe.push_str(&format!("\\x{:02X}", c as u32)),
            c => safe.push(c),
        }
    }
    Cow::Owned(safe)
}

/// A path as it is safe to put in a message: `shown` of what a path's own `Display` makes of it,
/// which stands in a byte it cannot read with U+FFFD.
pub(crate) fn shown_path(path: &Path) -> String {
    shown(&path.display().to_string()).into_owned()
}

/// `text` as one word of a shell command: as it is when it holds only characters the shell leaves
/// alone, and otherwise in single quotes (a quote inside is closed, escaped and opened again). A
/// name that starts with a dash is a relative path, and gets `./` in front so that it is not
/// taken for an option.
fn shell_quoted(text: &str) -> String {
    let text = &if text.starts_with('-') {
        format!("./{text}")
    } else {
        text.to_string()
    };
    let plain = |c: char| c.is_ascii_alphanumeric() || "_-./:@%+=,".contains(c);
    if !text.is_empty() && text.chars().all(plain) {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

/// `text` as one word of a command the reader can paste: `shell_quoted` of the name as it is, so
/// that a name holding a backslash names the file that exists. Only a name with a control
/// character in it is quoted around `shown` instead, because a line said to a terminal carries no
/// control character — such a name has to be retyped either way. A string is never both what a
/// human reads and what the shell is handed.
fn command_quoted(text: &str) -> String {
    if text.chars().any(char::is_control) {
        shell_quoted(&shown(text))
    } else {
        shell_quoted(text)
    }
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Every path is said through `shown`, and so is every borrowed word, whoever wrote it, so
        // that no arm has to judge which words could name a file. What this does not allow is the
        // same text escaped twice: see `shown`, and `BoardError::Card`, which holds words its own
        // arm escaped already.
        match self {
            // Shown as a whole, because the core's message for a firmware it could not read names
            // the file in its words.
            RunError::Firmware(err) => write!(f, "{}", shown(&err.to_string())),
            RunError::Memory(err) => write!(f, "{}", shown(&err.to_string())),
            RunError::Cpu(err) => write!(f, "{}", shown(&err.to_string())),
            RunError::Trace { path, source } => {
                write!(f, "{}: {}", shown(path), shown(&source.to_string()))
            }
            RunError::Write(err) => write!(f, "writing the trace: {}", shown(&err.to_string())),
            RunError::Console(err) => {
                write!(f, "writing the console output: {}", shown(&err.to_string()))
            }
            RunError::Window(err) => write!(f, "{}", shown(&err.to_string())),
            RunError::Frame(err) => write!(f, "no screenshot: {}", shown(&err.to_string())),
            RunError::Screenshot { path, source } => {
                write!(f, "{}: {}", shown(path), shown(&source.to_string()))
            }
            // The words of `source` name a path too when `saveable` made them (its reason for a
            // `--flash` file that could not be saved), and an `io::Error`'s words are never escaped
            // where they are made, so this arm escapes them (see the strategy on `shown`).
            RunError::Flash { path, source } => {
                write!(f, "{}: {}", shown(path), shown(&source.to_string()))
            }
            RunError::FlashSize { path, source } => {
                write!(f, "{}: {}", shown(path), shown(&source.to_string()))
            }
            RunError::Settings {
                path,
                source,
                backups,
            } => {
                // The clause that ends the message offers to delete the file, so it is a command
                // and quotes the raw name (see `command_quoted`); the display is escaped.
                let quoted = command_quoted(path);
                let path = shown(path);
                write!(f, "{path}: {}", shown(&source.to_string()))?;
                if !backups.is_empty() {
                    let copies: Vec<String> = backups
                        .iter()
                        .map(|copy| shown(copy).into_owned())
                        .collect();
                    write!(
                        f,
                        " Older settings are kept beside it: {}. Restoring one of them gives back \
                         the settings of the time it was made, and everything saved since is \
                         lost; which one holds the settings you want is for you to say. Or start \
                         fresh by deleting {quoted}.",
                        copies.join(", ")
                    )?;
                }
                Ok(())
            }
            RunError::V2InRawRun { path } => {
                // The advice is a command, so it quotes the raw name (see `command_quoted`); the
                // display is escaped, and the two differ whenever the name holds a backslash.
                let quoted = command_quoted(path);
                let path = shown(path);
                write!(
                    f,
                    "{path}: these settings were saved by the board of this emulator (format v2), \
                 and this run reads the raw layout, which cannot hold them. Read them with the \
                 board (cmm2-emulator --flash {quoted}, with no --firmware) once the firmware \
                 they were made for is burned onto it, or point --flash at a raw file. To start \
                 fresh, move {quoted} away."
                )
            }
            RunError::BackupNotAFile { path, backup } => write!(
                f,
                "{}: not backing up the old settings to {}: that name is not a regular file. Move \
                 it out of the way and start again; no file was written.",
                shown(path),
                shown(backup)
            ),
            RunError::BackupsFull { path, first, last } => write!(
                f,
                "{}: not backing up the old settings: the names from {} to {} are all taken. Move \
                 some of them out of the way and start again; no file was written.",
                shown(path),
                shown(first),
                shown(last)
            ),
            RunError::BackupRaceLost { path, tries } => write!(
                f,
                "{}: not backing up the old settings: the name chosen for the copy was taken \
                 before the copy could be made, {tries} times running. Something else is writing \
                 in this directory; nothing was written, and {} is untouched.",
                shown(path),
                shown(path)
            ),
            RunError::BackupNeedsLink {
                path,
                backup,
                source,
            } => write!(
                f,
                "{}: not backing up the old settings to {}: the copy is made with a hard link, \
                 and this file system refused one ({}). Nothing was written, and {} is untouched.",
                shown(path),
                shown(backup),
                shown(&source.to_string()),
                shown(path)
            ),
            RunError::NotReadBack { path, reason } => {
                let path = shown(path);
                write!(
                    f,
                    "{path}: the new settings file made for the migration did not read back ({}); \
                     nothing was written, and {path} is untouched.",
                    shown(reason)
                )
            }
            RunError::NoLayout { path, source } => {
                write!(f, "{}: {}", shown(path), shown(&source.to_string()))
            }
            RunError::BoardNoLayout { path } => write!(
                f,
                "{}: the firmware burned onto the board has no settings layout yet (see issue 82), \
                 so the board cannot keep its settings. Burn a firmware I know with: \
                 cmm2-emulator --burn <file>",
                shown(path)
            ),
            // The value comes from the environment and can hold anything a file name can, so it
            // is escaped like every other user-supplied string. Said raw it broke the line and
            // recoloured it: a value with a line feed and a colour change put two line feeds and
            // two escape bytes on standard error.
            RunError::BadLayoutName { variable, name } => {
                write!(f, "{variable} is \"{}\", not b5 or rc6", shown(name))
            }
            RunError::Sd { path, source } => {
                write!(f, "{}: {}", shown(path), shown(&source.to_string()))
            }
            RunError::SdSize { path, source } => {
                write!(f, "{}: {}", shown(path), shown(&source.to_string()))
            }
            RunError::SdTooSmall { path, size } => write!(
                f,
                "{}: {size} bytes is smaller than the {MIN_IMAGE_SIZE} bytes (512 KB) of the \
                 smallest card",
                shown(path)
            ),
            // Handed over as it is: an arm of the board's escapes the names in its own words (see
            // the strategy on `shown`), and escaping the whole message again is what read as two
            // backslashes where a name had one.
            RunError::Board(err) => err.fmt(f),
            // The core's words about a guest fault, which name no file and are escaped anyway: the
            // strategy does not ask an arm whether the words it says could hold a name.
            RunError::Fault(description) => f.write_str(&shown(description)),
        }
    }
}

impl std::error::Error for RunError {}

impl From<FirmwareError> for RunError {
    fn from(err: FirmwareError) -> RunError {
        RunError::Firmware(err)
    }
}

impl From<MemoryError> for RunError {
    fn from(err: MemoryError) -> RunError {
        RunError::Memory(err)
    }
}

impl From<CpuError> for RunError {
    fn from(err: CpuError) -> RunError {
        RunError::Cpu(err)
    }
}

/// The outcome of a run, given its other errors in the order they happened (the trace, the
/// screenshot, the save). When more than one failed, every one but the last is said to `say`
/// (standard error, for the caller) and the last is returned for the caller to say, so that none
/// hides another, and the run's own comes first.
pub(crate) fn settle(
    run: Result<bool, RunError>,
    rest: [Result<(), RunError>; 3],
    say: &mut impl Write,
) -> Result<bool, RunError> {
    let mut errors: Vec<RunError> = rest.into_iter().filter_map(Result::err).collect();
    match run {
        Ok(ok) if errors.is_empty() => return Ok(ok),
        Ok(_) => {}
        Err(run) => errors.insert(0, run),
    }
    let last = errors.pop().expect("something failed");
    for err in errors {
        let _ = say.write_all(line(err).as_bytes());
    }
    Err(last)
}

/// How far into an escape sequence the guest's output is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum Escape {
    #[default]
    None,
    /// After ESC.
    Seen,
    /// In a control sequence (ESC [), until its final byte.
    Control,
    /// In an operating system command (ESC ]), until BEL or ESC \.
    Command,
    /// In an operating system command, after an ESC that ends it with the byte that follows.
    CommandEnd,
}

/// Where the guest's output has left the cursor, as far as can be told from the bytes: in the
/// first column or not, and on a row with something on it or not. It starts clean. Text and
/// control bytes move it as a terminal would. An escape sequence is taken as leaving it on a row
/// with something on it, except those that are known to move nothing: a colour or attribute
/// change (a control sequence ending in `m`) and an operating system command (a window title).
/// What `Cursor` cannot know is where any other escape sequence moves the cursor, so a screen
/// that the guest has drawn all over gets the line on a row of its own, which may be a row that
/// has something on it already; and a sequence that it does not know to be harmless leaves an
/// empty row before the line. Output that ends inside an escape sequence is cancelled by the
/// line, with CAN (see `said`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) struct Cursor {
    /// The cursor is not in the first column.
    pub(crate) away: bool,
    /// The row has had something written on it.
    pub(crate) written: bool,
    pub(crate) escape: Escape,
}

impl Cursor {
    /// Where the cursor is after `bytes` are written. `newline_returns` is for a terminal that
    /// turns a line feed into CR LF (a raw one does not).
    pub(crate) fn after(self, bytes: &[u8], newline_returns: bool) -> Cursor {
        let mut cursor = self;
        for &byte in bytes {
            cursor = cursor.byte(byte, newline_returns);
        }
        cursor
    }

    fn byte(mut self, byte: u8, newline_returns: bool) -> Cursor {
        match (self.escape, byte) {
            (Escape::None, 0x1B) => self.escape = Escape::Seen,
            (Escape::None, b'\r') => self.away = false,
            (Escape::None, b'\n') => {
                self.written = false;
                self.away &= !newline_returns;
            }
            // BEL, NUL, BS, SO and SI do not print, and are counted as text: the cost is an empty
            // row before the line, which is the safe way to be wrong.
            (Escape::None, _) => self = self.marked(),
            (Escape::Seen, b'[') => self.escape = Escape::Control,
            (Escape::Seen, b']') => self.escape = Escape::Command,
            // Any other escape is two bytes, or the start of what is shown as text.
            (Escape::Seen, _) => self = self.ended().marked(),
            // Parameters and intermediate bytes; the final byte ends it (CAN and SUB, which
            // cancel it, are counted as one: it is text that follows them).
            (Escape::Control, 0x20..=0x3F) => {}
            (Escape::Control, b'm') => self = self.ended(),
            (Escape::Control, _) => self = self.ended().marked(),
            // CAN and SUB cancel an operating system command, as terminals have it, and what
            // follows them is shown.
            (Escape::Command | Escape::CommandEnd, 0x18 | 0x1A) => self = self.ended(),
            (Escape::Command, 0x07) => self = self.ended(),
            (Escape::Command, 0x1B) => self.escape = Escape::CommandEnd,
            (Escape::Command, _) => {}
            (Escape::CommandEnd, b'\\') => self = self.ended(),
            // Any other byte after the ESC ends the command, and the ESC and it are an escape.
            (Escape::CommandEnd, _) => {
                self.escape = Escape::Seen;
                return self.byte(byte, newline_returns);
            }
        }
        self
    }

    fn marked(mut self) -> Cursor {
        self.away = true;
        self.written = true;
        self
    }

    fn ended(mut self) -> Cursor {
        self.escape = Escape::None;
        self
    }

    /// What is known after a write that a signal cut short: nothing, so the cursor is taken as
    /// somewhere on a row with something on it and in an escape that the line must cancel.
    fn cut_short() -> Cursor {
        Cursor {
            away: true,
            written: true,
            escape: Escape::Seen,
        }
    }

    fn bits(self) -> u8 {
        u8::from(self.away) | u8::from(self.written) << 1 | (self.escape as u8) << 2
    }

    fn from_bits(bits: u8) -> Cursor {
        Cursor {
            away: bits & 1 != 0,
            written: bits & 2 != 0,
            escape: match bits >> 2 {
                1 => Escape::Seen,
                2 => Escape::Control,
                3 => Escape::Command,
                4 => Escape::CommandEnd,
                _ => Escape::None,
            },
        }
    }
}

/// The cursor as the last output of the guest to the terminal left it, as bits (see `Cursor`).
/// Only `run_live` sets it, and only when standard output and error are a terminal.
static CURSOR: AtomicU8 = AtomicU8::new(0);

/// Notes that a write of the guest's output was cut short, so that none of it is known.
pub(crate) fn note_cut_short() {
    CURSOR.store(Cursor::cut_short().bits(), Ordering::Relaxed);
}

/// The cursor as it was left, which is used up: whatever is said next moves it.
pub(crate) fn take_cursor() -> Cursor {
    Cursor::from_bits(CURSOR.swap(0, Ordering::Relaxed))
}

/// Notes that `bytes` went to the terminal.
pub(crate) fn note_output(bytes: &[u8], newline_returns: bool) {
    let before = Cursor::from_bits(CURSOR.load(Ordering::Relaxed));
    CURSOR.store(
        before.after(bytes, newline_returns).bits(),
        Ordering::Relaxed,
    );
}

/// The bytes of a line said on standard error: `line` with the program's name in front. When a
/// line feed does not return the cursor to its column (a raw terminal), it ends in CR LF, as a
/// bare LF would step down and leave the next line where this one ended. When the guest left
/// the cursor somewhere that is not the start of an empty row, it starts on a new one (or, on an
/// empty row, in its first column). When the guest's output ended inside an escape sequence, it
/// starts with CAN, which cancels it, as the line would be taken into it (an `ESC c` would clear
/// the screen) and then on a new row.
pub(crate) fn said(line: impl fmt::Display, lf_keeps_column: bool, cursor: Cursor) -> String {
    let newline = if lf_keeps_column { "\r\n" } else { "\n" };
    let start = if cursor.escape != Escape::None {
        format!("\x18{newline}")
    } else if cursor.written {
        newline.to_string()
    } else if cursor.away {
        "\r".to_string()
    } else {
        String::new()
    };
    format!("{start}cmm2-emulator: {line}{newline}")
}

/// A line as it is said now: `said`, for where standard error is and where the guest left the
/// cursor, which it uses up, as the line has moved it to the start of a row.
pub(crate) fn line(text: impl fmt::Display) -> String {
    let cursor = take_cursor();
    // This is for the terminal that standard error is on, which may not be the one that the
    // guest's output went to (its row may then get an empty row before the line); it is not worth
    // finding out.
    said(text, crate::terminal::lf_keeps_column(2), cursor)
}

/// Says a line on standard error, with the program's name in front. It goes through the same
/// writer as the rest of the output of a run, so that a standard error that nobody reads does
/// not stop a signal from ending the run (and a line that a signal gave up on is dropped).
pub(crate) fn say(text: impl fmt::Display) {
    let _ = Output::stderr().write_all(line(text).as_bytes());
}

#[cfg(test)]
mod tests;

#[cfg(test)]
/// Held by the tests that use the cursor, which is one for the whole process.
pub(crate) static CURSOR_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());
