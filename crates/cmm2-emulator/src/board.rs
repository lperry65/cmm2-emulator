//! The board: what a real one keeps between switching it on and off. Its firmware, its settings
//! (the `--flash` file) and the card in its socket live in a directory, and a plain
//! `cmm2-emulator` powers the board on as it was left. The directory is the board: another
//! `CMM2_EMULATOR_HOME` is another board.

use crate::atomic_write::{remove_stale_temps_in, replace_atomically, try_lock_exclusive};
use crate::error::RunError;
use crate::saveable::{check_regular, why_not_replaceable};
use cmm2_core::Firmware;
use std::ffi::OsString;
use std::fmt;
use std::fs::{DirBuilder, File, OpenOptions};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

/// The environment variable that names the board's directory, for a run that must not touch the
/// usual one (the tests use it).
pub(crate) const HOME_VARIABLE: &str = "CMM2_EMULATOR_HOME";

const FIRMWARE: &str = "firmware.bin";
const SETTINGS: &str = "settings.flash";
const CARD: &str = "sd.path";
const LOCK: &str = "board.lock";

/// What is wrong with the board, said as the other errors of a run are.
#[derive(Debug)]
pub(crate) enum BoardError {
    /// There is nowhere to look for the board's directory.
    NoHome,
    /// Nothing has been burned onto the board in this directory.
    NothingBurned(PathBuf),
    /// `CMM2_EMULATOR_HOME` is set, to nothing.
    EmptyHome,
    /// The directory's name is not UTF-8, which the paths of a run (strings) cannot hold.
    NotUtf8(PathBuf),
    /// Another emulator has the board switched on.
    Busy(PathBuf),
    Io {
        path: PathBuf,
        source: io::Error,
    },
    /// The file system cannot lock the lock file, so that two runs could not be kept apart.
    NoLock {
        path: PathBuf,
        source: io::Error,
        /// The lock file could only be opened to read.
        read_only: bool,
    },
    /// The file that remembers the card holds something that is not a path.
    CardRecord(PathBuf),
    /// A card name that the record could not keep.
    CardName(String),
    /// The card that the board remembers cannot be used; the text is why.
    Card(String),
}

impl fmt::Display for BoardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BoardError::NoHome => write!(
                f,
                "nowhere to keep the board: {HOME_VARIABLE} is not set, XDG_DATA_HOME is not set \
                 to an absolute path, and neither HOME nor USERPROFILE is set. Set \
                 {HOME_VARIABLE} to a directory for it"
            ),
            BoardError::NothingBurned(dir) => write!(
                f,
                "no firmware is burned onto the board in {}. Burn it once with: \
                 cmm2-emulator --burn <file>",
                dir.display()
            ),
            BoardError::EmptyHome => write!(
                f,
                "{HOME_VARIABLE} is set but empty: set it to a directory for the board, or unset it"
            ),
            BoardError::NotUtf8(dir) => write!(
                f,
                "{}: the name of the board's directory is not valid UTF-8, which is not \
                 supported yet. Set {HOME_VARIABLE} to another directory",
                dir.display()
            ),
            BoardError::Busy(dir) => write!(
                f,
                "the board in {} is already switched on, by another cmm2-emulator. A board can \
                 only be switched on once: close that one first",
                dir.display()
            ),
            BoardError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            BoardError::NoLock {
                path,
                source,
                read_only,
            } => write!(
                f,
                "{}: the board cannot be locked, so two runs could not be kept off it ({source}; \
                 the lock file was opened {})",
                path.display(),
                if *read_only { "to read" } else { "to write" }
            ),
            BoardError::CardRecord(path) => write!(
                f,
                "{}: it does not hold the path of a card (an absolute one, as UTF-8 text). Give a \
                 card with --sd <file>, or take it out with --no-sd",
                path.display()
            ),
            BoardError::CardName(name) => write!(
                f,
                "{name:?}: a card's name cannot end in a line end, which the board's record of \
                 it could not keep"
            ),
            BoardError::Card(why) => write!(
                f,
                "the card in the socket cannot be used: {why}. Give another with --sd <file>, or \
                 take it out with --no-sd"
            ),
        }
    }
}

impl From<BoardError> for RunError {
    fn from(err: BoardError) -> RunError {
        RunError::Board(err)
    }
}

/// Where the board's directory is, from the environment as `var` has it: `CMM2_EMULATOR_HOME`
/// itself, else `cmm2-emulator` in `XDG_DATA_HOME` (when it is an absolute path, as the XDG
/// specification has it), else on Windows (issue 75) in `LOCALAPPDATA`, which is the convention
/// there, else in `.local/share` of the home directory (`HOME`, or `USERPROFILE`). The path is
/// built from components, so that no separator is mixed with another. The name has to be UTF-8.
pub(crate) fn board_dir(var: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf, BoardError> {
    let set = |name: &str| var(name).filter(|value| !value.is_empty());
    // An empty value is not "unset": it is a mistake (an unset shell variable in a script gives
    // one), and falling back to the usual directory would put the run on a board it did not mean.
    if var(HOME_VARIABLE).is_some_and(|value| value.is_empty()) {
        return Err(BoardError::EmptyHome);
    }
    let dir = if let Some(home) = set(HOME_VARIABLE) {
        let home = PathBuf::from(home);
        std::path::absolute(&home).unwrap_or(home)
    } else if let Some(data) = set("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|data| data.is_absolute())
    {
        data.join("cmm2-emulator")
    } else if let Some(local) = set("LOCALAPPDATA").filter(|_| cfg!(windows)) {
        PathBuf::from(local).join("cmm2-emulator")
    } else {
        let home = set("HOME").or_else(|| set("USERPROFILE"));
        let home = home.ok_or(BoardError::NoHome)?;
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("cmm2-emulator")
    };
    if dir.to_str().is_none() {
        return Err(BoardError::NotUtf8(dir));
    }
    Ok(dir)
}

/// A board that is switched on: its directory, and the lock on it, held until it is dropped.
pub(crate) struct Board {
    dir: PathBuf,
    _lock: File,
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> BoardError {
    let path = path.to_path_buf();
    move |source| BoardError::Io { path, source }
}

/// Opens the lock file, making it if it is not there, and says whether it could only be opened to
/// read. It is opened to write first: flock(2) has the file open for writing for an exclusive lock
/// on NFS, where it is a byte-range lock. Only when that is refused (EACCES, or EROFS on a
/// read-only file system) is a lock file that is there opened to read, which flock is as good
/// with elsewhere, so that a board whose directory or lock file cannot be written still has its
/// lock taken.
/// Whether a lock file that could not be opened to write may be opened to read: permission was
/// refused (EACCES, or EPERM for an immutable file, `chattr +i`), or the file system is
/// read-only (EROFS).
fn opens_to_read_instead(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::EACCES | libc::EPERM | libc::EROFS)
    )
}

pub(crate) fn open_lock(path: &Path) -> io::Result<(File, bool)> {
    let write = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path);
    match write {
        Ok(lock) => Ok((lock, false)),
        Err(err) if opens_to_read_instead(&err) => {
            // Not there, in a directory that cannot be written: the error stands.
            File::open(path).map(|lock| (lock, true)).map_err(|_| err)
        }
        Err(err) => Err(err),
    }
}

impl Board {
    /// Switches on the board in `dir`: takes the lock, so that a second emulator cannot be on it
    /// too. A board that is not being burned has to have been burned already, which is looked at
    /// before anything is made, the directory and the lock file included.
    pub(crate) fn switch_on(dir: PathBuf, burning: bool) -> Result<Board, BoardError> {
        // This is looked at before the lock, so a plain start that races a first burn can say
        // "nothing is burned" and not "already switched on"; the next one will say the latter.
        if !burning && !dir.join(FIRMWARE).is_file() {
            return Err(BoardError::NothingBurned(dir));
        }
        // The XDG specification has such a directory private to the user; where it is made over
        // missing parents (a first `~/.local/share`), they are too.
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(io_error(&dir))?;
        let lock_path = dir.join(LOCK);
        let (lock, read_only) = open_lock(&lock_path).map_err(io_error(&lock_path))?;
        match try_lock_exclusive(&lock) {
            Ok(true) => {}
            Ok(false) => return Err(BoardError::Busy(dir)),
            Err(source) => {
                return Err(BoardError::NoLock {
                    path: lock_path,
                    source,
                    read_only,
                });
            }
        }
        // A burn that was cut short left its temporary file, and nobody else is burning now. It is
        // looked for in the board's directory itself, with no file name to resolve, so that a
        // `firmware.bin` linked elsewhere (or any link in the directory) does not take the cleanup
        // out of the board.
        remove_stale_temps_in(&dir);
        Ok(Board { dir, _lock: lock })
    }

    /// The firmware image on the board, as a path for `build_machine`.
    pub(crate) fn firmware_path(&self) -> String {
        self.dir.join(FIRMWARE).display().to_string()
    }

    /// The board's own settings file, used when no `--flash` file is named.
    pub(crate) fn settings_path(&self) -> String {
        self.dir.join(SETTINGS).display().to_string()
    }

    /// Burns the image onto the board, which `load_image` has looked at, so that an image that
    /// is refused leaves the old firmware as it was and makes nothing. The settings file is not
    /// touched: it holds only what the user changed. This is done before the machine is built,
    /// so a start that then fails (a card that has gone) has burned all the same.
    pub(crate) fn burn(&self, firmware: &Firmware) -> Result<(), RunError> {
        self.write_file(&self.firmware_path(), firmware.image(), "burn the firmware")
    }

    /// The card that the last `--sd` put in the socket, if there is one.
    pub(crate) fn remembered_card(&self) -> Result<Option<String>, RunError> {
        let record = self.dir.join(CARD);
        let bytes = match std::fs::read(&record) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            // Something that is not a file is not a record, and is said as one.
            Err(_) if record.is_dir() => return Err(BoardError::CardRecord(record).into()),
            Err(err) => {
                return Err(BoardError::Io {
                    path: record,
                    source: err,
                }
                .into());
            }
        };
        // A hand-written file ends in a line end (or two), which is not part of the path; blanks
        // are, as a name may end in one (`remember_card` keeps it as it is).
        let text = String::from_utf8(bytes)
            .ok()
            .map(|text| text.trim_end_matches(['\r', '\n']).to_string());
        // It is written as an absolute path, so anything else is not what this wrote.
        let Some(text) = text.filter(|text| Path::new(text).is_absolute()) else {
            return Err(BoardError::CardRecord(record).into());
        };
        Ok(Some(text))
    }

    /// Puts the card in `path` in the socket for good, as its absolute path (the file stays
    /// where it is, so the board has to find it again from wherever the next run starts). A
    /// record that is broken, even a directory (an empty one), is replaced.
    pub(crate) fn remember_card(&self, path: &str) -> Result<(), RunError> {
        // The record ends at a line end, so a name that ends in one could not be read back.
        if path.ends_with(['\r', '\n']) {
            return Err(BoardError::CardName(path.to_string()).into());
        }
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.into());
        let record = self.dir.join(CARD);
        self.clear_directory(&record)?;
        self.write_file(
            &record.display().to_string(),
            absolute.to_string_lossy().as_bytes(),
            "remember the card",
        )
    }

    /// Takes the card out of the socket. Nothing to take out is not an error, and a record that
    /// is a directory (an empty one) is taken out too.
    pub(crate) fn forget_card(&self) -> Result<(), RunError> {
        let record = self.dir.join(CARD);
        self.clear_directory(&record)?;
        match std::fs::remove_file(&record) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => Err(BoardError::Io {
                path: record,
                source: err,
            }
            .into()),
            _ => Ok(()),
        }
    }

    /// Removes `record` if it is a directory, which only works when it is empty: a person's
    /// files are not for this to delete.
    fn clear_directory(&self, record: &Path) -> Result<(), RunError> {
        if std::fs::symlink_metadata(record).is_ok_and(|meta| meta.is_dir()) {
            std::fs::remove_dir(record).map_err(io_error(record))?;
        }
        Ok(())
    }

    /// Writes one of the board's files as the other saves are written, after looking, as they
    /// are, at whether it could be: `what` is what it was for. A symbolic link in the file's
    /// place (the firmware linked to a vendor's `.bin`, say) is replaced, in one step by the
    /// rename and so never missing, and not followed, so what it points to is never written.
    fn write_file(&self, path: &str, bytes: &[u8], what: &str) -> Result<(), RunError> {
        let error = |source| BoardError::Io {
            path: PathBuf::from(path),
            source,
        };
        let link = std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_symlink());
        // What a link leads to is not looked at (it is not written), only the directory is.
        if !link {
            check_regular(path).map_err(error)?;
        }
        if let Some(why) = why_not_replaceable(path) {
            let source = io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{why}, so the board could not {what}"),
            );
            return Err(error(source).into());
        }
        replace_atomically(path, bytes).map_err(|source| error(source).into())
    }
}

/// An error of loading the card the board remembers, which the owner did not name in this run, is
/// told how to get rid of it, as the file may be long gone from where it was. Another error is
/// left as it is.
pub(crate) fn about_the_remembered_card(err: RunError) -> RunError {
    match err {
        RunError::Sd { .. } | RunError::SdSize { .. } | RunError::SdTooSmall { .. } => {
            BoardError::Card(err.to_string()).into()
        }
        other => other,
    }
}

#[cfg(test)]
mod tests;
