//! Whether a file named by `--flash` or `--sd` can be read and then saved over, which is looked at
//! before the run, so that a long run is not lost to a path that could never be written.

use crate::atomic_write::resolve;
use crate::error::{say, shown};
use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Whether a path cannot be that of a file: it is empty, or ends in a slash, or in . or .., which
/// is a directory even if there is nothing there yet to say so.
fn not_a_file_name(path: &Path) -> bool {
    let name = path.as_os_str().as_bytes();
    let last = name.rsplit(|&byte| byte == b'/').next().unwrap_or(name);
    name.is_empty() || name.ends_with(b"/") || last == b"." || last == b".."
}

/// Refuses what the file named is if it is there and not a regular file: a device (a whole
/// card, with `sudo`, would be read through and then have a file renamed over its node), a
/// FIFO (the read would never end), a directory. This looks before anything is opened. A file
/// that is not there is for the read, or the first save, to say.
pub(crate) fn check_regular(path: &str) -> io::Result<()> {
    if not_a_file_name(Path::new(path)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a file name (it is empty, or ends in a slash, or in . or ..)",
        ));
    }
    match std::fs::metadata(path) {
        Ok(meta) if !meta.is_file() => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        )),
        // It is there and it has to be read, so that is said in words and not as a bare errno.
        Ok(_) if access(Path::new(path), libc::R_OK).is_err() => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the file cannot be read",
        )),
        _ => Ok(()),
    }
}

/// Whether the calling user can use `path` as `mode` (R_OK, W_OK, X_OK), as faccessat(2) says
/// with AT_EACCESS, so by the effective user (which is the one that will write), and which
/// looks at the file system too: a read-only one is EROFS.
pub(crate) fn access(path: &Path, mode: libc::c_int) -> io::Result<()> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: the pointer is to a NUL-terminated string that lives through the call.
    if unsafe { libc::faccessat(libc::AT_FDCWD, path.as_ptr(), mode, libc::AT_EACCESS) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// The attribute bits (FS_IMMUTABLE_FL and FS_APPEND_FL of linux/fs.h) that make the rename of
/// a new file over this one fail with EPERM, as the mode bits do not show.
const FS_IMMUTABLE_OR_APPEND: libc::c_long = 0x10 | 0x20;

/// Whether the file system attribute flags of the file forbid replacing it.
fn is_immutable(file: &File) -> bool {
    use std::os::fd::AsRawFd;
    let mut flags: libc::c_long = 0;
    // SAFETY: the ioctl fills one c_long through the pointer; a file system that does not know
    // it answers with an error and leaves it 0.
    unsafe { libc::ioctl(file.as_raw_fd(), libc::FS_IOC_GETFLAGS, &mut flags) };
    flags & FS_IMMUTABLE_OR_APPEND != 0
}

/// In a directory with the sticky bit (/tmp), only the owner of the file, the owner of the
/// directory or a user with CAP_FOWNER can rename a file away, which is what replacing it is.
fn sticky_refuses(
    dir_mode: u32,
    dir_owner: u32,
    file_owner: u32,
    user: u32,
    cap_fowner: bool,
) -> bool {
    dir_mode & libc::S_ISVTX != 0 && !cap_fowner && file_owner != user && dir_owner != user
}

/// What is wrong with a path that a save goes through, when the OS said no, in words. `denied`
/// is what EACCES means for what was being asked: that a directory cannot be searched, or that
/// one cannot be written to.
fn why_path_failed(err: &io::Error, path: &Path, denied: &str) -> String {
    match err.raw_os_error() {
        Some(libc::EROFS) => "its file system is read-only".to_string(),
        Some(libc::EACCES | libc::EPERM) => denied.to_string(),
        Some(libc::ENOENT) => format!("its directory {} does not exist", path.display()),
        Some(libc::ENOTDIR) => format!("{} is not a directory", path.display()),
        Some(libc::ELOOP) => "its path is a loop of symbolic links".to_string(),
        Some(libc::ENAMETOOLONG) => {
            "a name on its path is longer than the file system allows".to_string()
        }
        _ => format!("its directory {} cannot be used ({err})", path.display()),
    }
}

const NOT_SEARCHABLE: &str = "a directory on its path cannot be searched";
const NOT_WRITABLE: &str = "its directory is not writable";

/// Whether the effective capabilities hold CAP_FOWNER (bit 3), which is what lets a user rename
/// away another user's file in a sticky directory (as the kernel has it, and not just the
/// user being root: a root without it, and a user with it, are both possible). From the
/// CapEff line of /proc/self/status; where that cannot be read it is the effective user being root.
fn has_cap_fowner() -> bool {
    const CAP_FOWNER: u32 = 3;
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            let line = status
                .lines()
                .find_map(|line| line.strip_prefix("CapEff:"))?;
            u64::from_str_radix(line.trim(), 16).ok()
        })
        .map_or_else(
            // SAFETY: geteuid has no arguments and cannot fail.
            || unsafe { libc::geteuid() } == 0,
            |caps| caps >> CAP_FOWNER & 1 != 0,
        )
}

/// Why `write_atomically` could not save `path`, if it could not, as far as can be seen at the
/// start. It makes a temporary file in the directory of the file it ends up writing (a link is
/// followed), and renames it over the file. So that directory has to be there and writable, and
/// not on a read-only file system; the file has to be one that can be replaced (not immutable
/// or append-only, and not another user's in a sticky directory). The file's own permissions do
/// not matter to that. What cannot be seen from here (a full disk, say) is left to the save.
pub(crate) fn why_not_saveable(path: &str) -> Option<String> {
    why_not_saveable_as(path, true)
}

/// `why_not_saveable`, for a save that follows a link at `path` (`follow`) or replaces it.
fn why_not_saveable_as(path: &str, follow: bool) -> Option<String> {
    let target = if follow {
        match resolve(Path::new(path)) {
            Ok(target) => target,
            Err(err) => return Some(why_path_failed(&err, Path::new(path), NOT_SEARCHABLE)),
        }
    } else {
        PathBuf::from(path)
    };
    // A link can point at a name that is not one (`ln -s newfile/ link`), which the name given
    // does not show.
    if not_a_file_name(&target) {
        return Some(format!(
            "it leads to {}, which cannot be the name of a file",
            target.display()
        ));
    }
    let dir = match target.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let dir_meta = match std::fs::metadata(dir) {
        Ok(meta) if !meta.is_dir() => return Some(format!("{} is not a directory", dir.display())),
        Ok(meta) => meta,
        Err(err) => return Some(why_path_failed(&err, dir, NOT_SEARCHABLE)),
    };
    // Searching the directory and writing to it are different rights, and are said apart.
    if let Err(err) = access(dir, libc::X_OK) {
        return Some(why_path_failed(
            &err,
            dir,
            "its directory cannot be searched",
        ));
    }
    if let Err(err) = access(dir, libc::W_OK) {
        return Some(why_path_failed(&err, dir, NOT_WRITABLE));
    }
    // A directory that is append-only lets a file be made in it but not renamed over another.
    // Its flags are asked of a descriptor, which takes the right to read it, so a directory
    // that cannot be read is one whose flags cannot be seen, and that is said.
    match File::open(dir) {
        Ok(dir) if is_immutable(&dir) => {
            return Some("its directory is append-only or immutable".to_string());
        }
        Ok(_) => {}
        // Both names are escaped at the line they are said on: the words of the `io::Error` are its
        // own and nothing escaped them before this (see `shown`).
        Err(err) => say(format!(
            "{}: its directory cannot be read ({}), so whether it is append-only is not known",
            shown(path),
            shown(&err.to_string())
        )),
    }
    // What is known of the file without opening it comes first, as a file that cannot be read
    // (which `check_regular` has already said) has its attributes too.
    let file_meta = if follow {
        std::fs::metadata(&target)
    } else {
        std::fs::symlink_metadata(&target)
    }
    .ok()?;
    // A link itself has no attribute flags (`chattr` refuses one), so only what it leads to
    // could be immutable, and a save that replaces the link does not touch that.
    if !file_meta.is_symlink() && File::open(&target).is_ok_and(|file| is_immutable(&file)) {
        return Some("the file is immutable or append-only".to_string());
    }
    // SAFETY: geteuid has no arguments and cannot fail.
    let user = unsafe { libc::geteuid() };
    sticky_refuses(
        dir_meta.mode(),
        dir_meta.uid(),
        file_meta.uid(),
        user,
        has_cap_fowner(),
    )
    .then(|| "its directory is sticky and the file is another user's".to_string())
}

/// `why_not_saveable` for a file that is replaced and not written through (`replace_atomically`:
/// the board's v2 settings). A link at the name is not followed by the save, so it is judged as
/// the thing that is replaced: by its own directory, where the temporary file is made, and by its
/// own owner for the sticky bit, and not by where it leads.
pub(crate) fn why_not_replaceable(path: &str) -> Option<String> {
    why_not_saveable_as(path, false)
}

#[cfg(test)]
mod tests;
