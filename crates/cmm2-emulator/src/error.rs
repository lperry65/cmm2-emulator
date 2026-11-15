//! What can end a run in an error, and how the errors of one run are said.

use crate::output::Output;
use crate::window::WindowError;
use cmm2_core::{
    CpuError, FirmwareError, FrameError, ImageSizeError, MIN_IMAGE_SIZE, MemoryError,
    PersistentError,
};
use std::fmt;
use std::io;
use std::io::Write;
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
    /// The guest faulted; the text is the core's description.
    Fault(String),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Firmware(err) => err.fmt(f),
            RunError::Memory(err) => err.fmt(f),
            RunError::Cpu(err) => err.fmt(f),
            RunError::Trace { path, source } => write!(f, "{path}: {source}"),
            RunError::Write(err) => write!(f, "writing the trace: {err}"),
            RunError::Console(err) => write!(f, "writing the console output: {err}"),
            RunError::Window(err) => err.fmt(f),
            RunError::Frame(err) => write!(f, "no screenshot: {err}"),
            RunError::Screenshot { path, source } => write!(f, "{path}: {source}"),
            RunError::Flash { path, source } => write!(f, "{path}: {source}"),
            RunError::FlashSize { path, source } => write!(f, "{path}: {source}"),
            RunError::Sd { path, source } => write!(f, "{path}: {source}"),
            RunError::SdSize { path, source } => write!(f, "{path}: {source}"),
            RunError::SdTooSmall { path, size } => write!(
                f,
                "{path}: {size} bytes is smaller than the {MIN_IMAGE_SIZE} bytes (512 KB) of the smallest card"
            ),
            RunError::Fault(description) => f.write_str(description),
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
