//! What the unit tests of more than one module share.

use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cmm2_core::{Cpu, Firmware, Irqs, cmm2_memory};

// A directory under the temp directory that is this test's own.
pub(crate) fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cmm2-main-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).unwrap();
    dir
}

pub(crate) fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

// The tests that panic on purpose and look at `HELD_PANICS` take turns, as it is shared.
pub(crate) static PANICS: std::sync::Mutex<()> = std::sync::Mutex::new(());

// As root a mode bit stops nothing, so what depends on one is not tried.
pub(crate) fn root() -> bool {
    // SAFETY: geteuid has no arguments and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

pub(crate) fn mode(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

// A machine that does nothing, with `options` for its files.
pub(crate) fn idle_cpu() -> Cpu {
    let mut image = vec![0; 64];
    image[..4].copy_from_slice(&0x2000_8000u32.to_le_bytes());
    image[4..8].copy_from_slice(&0x0800_0021u32.to_le_bytes());
    let irqs = Irqs::new();
    Cpu::new(
        &Firmware::from_bytes(image).unwrap(),
        cmm2_memory(&irqs),
        irqs,
    )
    .unwrap()
}

// Makes a file's time of last change `secs` seconds ago, by its path and without opening it
// (a FIFO would wait for a reader).
pub(crate) fn age(path: &Path, secs: u64) {
    let then = std::time::SystemTime::now() - std::time::Duration::from_secs(secs);
    let since = then.duration_since(std::time::UNIX_EPOCH).unwrap();
    let times = [
        libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_OMIT,
        },
        libc::timespec {
            tv_sec: since.as_secs() as libc::time_t,
            tv_nsec: 0,
        },
    ];
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: utimensat with a NUL-terminated name and two timespecs that live through the
    // call.
    let done = unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            name.as_ptr(),
            times.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    assert_eq!(done, 0);
}
