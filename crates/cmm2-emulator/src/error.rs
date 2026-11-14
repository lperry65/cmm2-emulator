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
        let _ = writeln!(say, "cmm2-emulator: {err}");
    }
    Err(last)
}

/// Says a line on standard error, with the program's name in front. It goes through the same
/// writer as the rest of the output of a run, so that a standard error that nobody reads does
/// not stop a signal from ending the run (and a line that a signal gave up on is dropped).
pub(crate) fn say(line: impl fmt::Display) {
    let line = format!("cmm2-emulator: {line}\n");
    let _ = Output::stderr().write_all(line.as_bytes());
}

#[cfg(test)]
mod tests;
