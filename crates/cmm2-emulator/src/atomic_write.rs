//! Writing a file so that a crash leaves the old one or the new one and never half of each, and
//! clearing up the temporary files that a save that was cut short leaves behind.

use crate::error::say;
use std::fs::{File, OpenOptions};
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The file a symlink chain ends at, which need not exist yet: a link is followed even when
/// what it points to is not there, so that the first save creates the target and leaves the link
/// as it is. A loop is an error (ELOOP).
pub(crate) fn resolve(path: &Path) -> io::Result<PathBuf> {
    let mut path = path.to_path_buf();
    for _ in 0..MAX_LINKS {
        match std::fs::read_link(&path) {
            Ok(link) => path = path.parent().unwrap_or(Path::new("")).join(link),
            // Not a link, or not there at all (or under something that is not a directory).
            Err(err)
                if matches!(
                    err.raw_os_error(),
                    Some(libc::EINVAL | libc::ENOENT | libc::ENOTDIR)
                ) =>
            {
                return Ok(path);
            }
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::from_raw_os_error(libc::ELOOP))
}

/// How many links to follow before it is a loop, as Linux has it.
const MAX_LINKS: usize = 40;

/// Writes `bytes` to `path` so that a crash leaves the old file and not half of the new one: to
/// a temporary file in the same directory, which is synced, and then renamed over the file, and
/// the directory is synced after. A symlink is followed, even a dangling one, so the file it
/// points to is the one written and the link stays. The new file has the old one's permissions
/// (not its owner, which only a privileged process can keep, and a hard link to the old file
/// does not see the new contents). The temporary file is made beside the file, with a short name
/// of its own (the process's id and the time), so that it fits whatever the length of the
/// file's name, and so that two emulators on one file, or a temporary file that an earlier
/// crash left under a repeated process id (in a container), do not get in the way.
pub(crate) fn write_atomically(path: &str, bytes: &[u8]) -> io::Result<()> {
    write_over(path, resolve(Path::new(path))?, bytes)
}

/// `write_atomically`, but a symbolic link at `path` is replaced by the new file and not followed:
/// what it points to is left alone, and the link is never missing, as the rename puts the new
/// file in its place in one step. This is for the board's own files, which are the board's
/// and not the user's to link elsewhere.
pub(crate) fn replace_atomically(path: &str, bytes: &[u8]) -> io::Result<()> {
    write_over(path, PathBuf::from(path), bytes)
}

fn write_over(path: &str, target: PathBuf, bytes: &[u8]) -> io::Result<()> {
    let dir = match target.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let temp = dir.join(format!(".cmm2.{}.{nanos}.tmp", std::process::id()));

    let mut file = create_locked(&temp)?;
    let written =
        write_synced(&mut file, &target, bytes).and_then(|()| std::fs::rename(&temp, &target));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written?;
    // The file is in its place by now, so a directory that cannot be synced (one that cannot be
    // read, or a file system that has no such thing) is something to say and not a failed save.
    if let Err(err) = File::open(dir).and_then(|dir| dir.sync_all()) {
        say(format!(
            "{path}: written, but its directory could not be synced ({err})"
        ));
    }
    Ok(())
}

/// Takes an advisory lock on the open file, without waiting: whether it was got, or the error
/// if the file system cannot lock at all. The lock goes with the open file description, so it is
/// gone when the file is closed or the process is, however that happens, and another open of the
/// same file (by another process, or here) cannot have it while it is held.
pub(crate) fn try_lock_exclusive(file: &File) -> io::Result<bool> {
    use std::os::fd::AsRawFd;
    // SAFETY: flock only takes a descriptor that is open, as this is.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let err = io::Error::last_os_error();
    // EWOULDBLOCK is another holder, and is the same number as EAGAIN.
    if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(false)
    } else {
        Err(err)
    }
}

/// `try_lock_exclusive` for a file that is only to be kept from the cleanup: an error is no lock.
fn try_lock(file: &File) -> bool {
    try_lock_exclusive(file).unwrap_or(false)
}

/// Makes the temporary file, which must not be there, and locks it at once, for as long as it is
/// open, so that `remove_stale_temps` can tell it from one that nobody is writing.
fn create_locked(temp: &Path) -> io::Result<File> {
    let file = OpenOptions::new().write(true).create_new(true).open(temp)?;
    // A file system that has no locks (some network ones) says no, and then the file is simply
    // not protected from the cleanup, which keeps to files it can lock.
    try_lock(&file);
    Ok(file)
}

/// How old a temporary file has to be (by its time of last change) to be removed. A new one may
/// be between being made and being locked, which is a matter of microseconds; a minute is far
/// longer than that.
const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(60);

/// Takes `.cmm2.<pid>.<nanos>.tmp` out of the directory of the file `path` leads to when nobody
/// has it. A save that was cut short (a SIGKILL, an abort, the power, a second Ctrl-C in the
/// middle of a slow one) leaves its temporary file, which is as big as the card. A save that is
/// going on holds an advisory lock on its file (`create_locked`), which is gone when the process
/// is, so a file that can be locked here and has not been changed for a minute belongs to no one.
/// That does not depend on what the pid in the name means here (another host or namespace, the
/// same directory seen by both), or on the clock. A file system with no locks (some network
/// ones, where locking can also fail or mislead) makes the lock fail, and then nothing is
/// removed: it is kept when it cannot be told. Errors are not reported: this is housekeeping.
pub(crate) fn remove_stale_temps(path: &str) {
    let Ok(target) = resolve(Path::new(path)) else {
        return;
    };
    let dir = match target.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    remove_stale_temps_in(dir);
}

/// `remove_stale_temps` for the directory itself, with no name to resolve: nothing is followed to
/// find it, so a link put where a file name would have been cannot lead the cleanup elsewhere.
pub(crate) fn remove_stale_temps_in(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_str().is_some_and(is_temp_name) {
            continue;
        }
        // What the directory says about the entry itself, not what a link in its place leads
        // to: a FIFO or a link to a device is not ours to open.
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|changed| changed.elapsed().ok())
            .is_some_and(|age| age >= STALE_AFTER);
        // The file is kept open, and so locked, until it has been removed, so that a save that
        // starts meanwhile cannot be using it (it makes its own, new, file).
        let Some(file) = old.then(|| open_to_lock(&entry.path())).flatten() else {
            continue;
        };
        if try_lock(&file) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Opens a file that is to be locked and not read: not following a link, not waiting for a
/// FIFO's other end, and only if it turns out to be a regular file. It is opened for reading,
/// or if that is not allowed (a left-over file whose mode was copied from a write-only one) for
/// writing; a file with neither right cannot be locked, and is left (README says how to clear
/// it by hand).
fn open_to_lock(path: &Path) -> Option<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let flags = libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY;
    let file = File::options()
        .read(true)
        .custom_flags(flags)
        .open(path)
        .or_else(|_| File::options().write(true).custom_flags(flags).open(path))
        .ok()?;
    file.metadata().ok()?.is_file().then_some(file)
}

/// Whether a name is that of a temporary file `write_atomically` makes: `.cmm2.<pid>.<nanos>.tmp`,
/// with a process id above 0 and both numbers in digits and nothing else (not a sign).
fn is_temp_name(name: &str) -> bool {
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    name.strip_prefix(".cmm2.")
        .and_then(|rest| rest.strip_suffix(".tmp"))
        .and_then(|rest| rest.split_once('.'))
        .is_some_and(|(pid, made)| {
            digits(pid) && digits(made) && pid.parse::<libc::pid_t>().is_ok_and(|pid| pid > 0)
        })
}

/// Fills `file` with `bytes` and the permissions `target` has, if it exists, and syncs it.
fn write_synced(file: &mut File, target: &Path, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)?;
    // A link left in place (see `replace_atomically`) has no permissions to keep.
    if let Ok(old) = std::fs::symlink_metadata(target)
        && !old.is_symlink()
    {
        file.set_permissions(old.permissions())?;
    }
    file.sync_all()
}

#[cfg(test)]
mod tests;
