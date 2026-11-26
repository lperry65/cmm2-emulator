//! What the unit tests of more than one module share.

use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cmm2_core::{B5, Cpu, Firmware, Irqs, Profile, cmm2_memory};

/// A name with a line feed and a colour change in it: a name a file system takes and a terminal
/// must not act on. `crate::error::shown` is how a message says it, and `HOSTILE_SHOWN` is that.
pub(crate) const HOSTILE_NAME: &str = "evil\n\x1b[31mRED\x1b[0m";

/// `HOSTILE_NAME` as a message says it: one backslash where the file has one, and the line feed and
/// the escape written out where they stand.
pub(crate) const HOSTILE_SHOWN: &str = r"evil\n\x1B[31mRED\x1B[0m";

// A directory under the temp directory that is this test's own, removed when it is dropped, a
// failed test too (a test that ends the process with `_exit`, as `in_own_process`'s child does,
// removes its own).
pub(crate) struct Scratch(PathBuf);

pub(crate) fn scratch(name: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("cmm2-main-{}-{name}", crate::run_id::run_id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).unwrap();
    Scratch(dir)
}

impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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

// A machine that does nothing, with the settings layout of b5.
pub(crate) fn idle_cpu() -> Cpu {
    idle_cpu_with(&B5)
}

// A machine that does nothing, with the settings layout of `profile`.
pub(crate) fn idle_cpu_with(profile: &'static Profile) -> Cpu {
    let mut image = vec![0; 64];
    image[..4].copy_from_slice(&0x2000_8000u32.to_le_bytes());
    image[4..8].copy_from_slice(&0x0800_0021u32.to_le_bytes());
    let irqs = Irqs::new();
    Cpu::new(
        &Firmware::from_bytes(image).unwrap().with_profile(profile),
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

// What the tests here that start a process or use a terminal rely on is Unix only (libc's
// `openpty` and `tcgetattr`, `_exit`, `fcntl`, `ExitStatusExt::signal`, and `fork` and
// `close_range` in panic_hook's tests; signals.rs is Unix only as a whole).

// A process that is made shares the descriptors of the test process from the `fork` (or
// `clone`) until its `exec`, whatever their close-on-exec flags say, as the flag only acts at the
// `exec`. `flock` belongs to the open file, not to the descriptor, so a lock that a test lets go
// of in that time is still held by the new process, and a test that looks at whether it is held
// (atomic_write's) fails now and then. The program does not have this: it opens its lock files
// with `O_CLOEXEC` already, and there is nothing in it to change; the gap is in the tests,
// which start processes while others have locks.
//
// So a test that starts a process does it through `output_of` (or `in_own_process`, which uses
// it), which holds this lock until the process has exec'd, and a test that needs a lock to be
// let go of at once holds it (`spawning`) for as long as it has the lock. The `fork` test in
// panic_hook holds it too, until its child is gone. Nothing here stops a test from starting a
// process some other way, so every spawn in the unit tests goes through `output_of`.
static SPAWNING: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn spawning() -> std::sync::MutexGuard<'static, ()> {
    SPAWNING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

// Runs `command` to the end, with its standard output and error taken and nothing on its
// standard input, as `Command::output` does, but with `spawning` held until it has exec'd.
// `inherit` are descriptors that the process is to have open (their close-on-exec flag is
// cleared in it, and not here).
pub(crate) fn output_of(
    command: &mut std::process::Command,
    inherit: &[libc::c_int],
) -> std::io::Result<std::process::Output> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    let inherit = inherit.to_vec();
    // SAFETY: runs in the child between fork and exec, and only calls fcntl, which is
    // async-signal-safe, on descriptors of its own; `inherit` is not changed or allocated for.
    unsafe {
        command.pre_exec(move || {
            for &fd in &inherit {
                if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    let child = {
        // `spawn` returns when the new process has exec'd, and let go of what is not its.
        let _alone = spawning();
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?
    };
    child.wait_with_output()
}

// How a process that a test started ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Ended {
    Signal(i32),
    Exit(i32),
}

// What `in_own_process` found: how the process ended, and what it wrote to standard error.
pub(crate) struct Ran {
    pub(crate) ended: Ended,
    pub(crate) stderr: String,
}

const CHILD_OF: &str = "CMM2_TEST_CHILD_OF";
const CHILD_FDS: &str = "CMM2_TEST_CHILD_FDS";
const CHILD_RAN: &str = "the child ran its body";

// Runs `body` in a process of its own, for a test that makes the process end, or changes what is
// the process's alone (signal handlers, the terminal). It is not a `fork`, as a fork shares the
// descriptors and the statics of the whole test process: a lock or a raw terminal that another
// test has at that moment would be the child's too.
//
// `test` is the path of the test that calls this, as `cargo test` names it
// (`signals::tests::a_test`), and is checked against the name of the thread it is called on. The
// test binary is started again to run that test alone. In that process, the call that is for the
// right test and `case` runs `body` and then ends the process with 0 (101 if `body` panicked),
// and does not return; any other call there (another case of the same test, in a loop) returns
// `None` so that the test goes on to its own. A call in the test process returns `Some`, with
// how the process ended, after it has seen that the process ran `body`.
//
// `case` tells apart the runs of one test that makes more than one ("" for a test that makes
// one). The `fds` stay open in the process (their close-on-exec flag is cleared, so it does not
// matter how they were opened), and `body` is told their numbers, which are the same as here, so
// that a test can hand over a pty it has made.
pub(crate) fn in_own_process(
    test: &str,
    case: &str,
    fds: &[libc::c_int],
    body: impl FnOnce(&[libc::c_int]),
) -> Option<Ran> {
    let key = format!("{test} {case}");
    let thread = std::thread::current();
    if let Ok(wanted) = std::env::var(CHILD_OF) {
        // Only the call that is for this test and case runs `body`.
        if wanted != key {
            return None;
        }
        let fds: Vec<libc::c_int> = std::env::var(CHILD_FDS)
            .unwrap_or_default()
            .split(',')
            .filter(|fd| !fd.is_empty())
            .map(|fd| fd.parse().unwrap())
            .collect();
        // Said first, as a test that is not found runs nothing and would look as if it passed.
        println!("{CHILD_RAN}");
        body(&fds);
        // SAFETY: _exit only ends this process.
        unsafe { libc::_exit(0) };
    }
    // The name given is the caller's own, or the child would run some other test and say it
    // had run this one's body. (A test run on the main thread, with one test thread, is not
    // named, so it is taken on trust.)
    if let Some(name) = thread.name().filter(|name| *name != "main") {
        assert_eq!(
            name, test,
            "in_own_process is called with another test's name"
        );
    }
    use std::os::unix::process::ExitStatusExt;
    let numbers: Vec<String> = fds.iter().map(ToString::to_string).collect();
    let output = output_of(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture", "--test-threads=2"])
            .env(CHILD_OF, &key)
            .env(CHILD_FDS, numbers.join(",")),
        fds,
    )
    .unwrap();
    let said = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        said.contains(CHILD_RAN),
        "no test called {test}, so its child ran nothing:\n{said}{stderr}"
    );
    let ended = match (output.status.signal(), output.status.code()) {
        (Some(signal), _) => Ended::Signal(signal),
        (None, code) => {
            if code == Some(101) {
                eprintln!("the child of {test} panicked:\n{stderr}");
            }
            Ended::Exit(code.unwrap_or(-1))
        }
    };
    Some(Ran { ended, stderr })
}

// A pty (master, slave) for a test of what is done to a terminal.
pub(crate) fn pty() -> (libc::c_int, libc::c_int) {
    let (mut master, mut slave) = (0, 0);
    // SAFETY: openpty fills in two descriptors, which the caller closes.
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(opened, 0);
    (master, slave)
}

// The local mode flags of the terminal on `fd`.
pub(crate) fn lflag(fd: libc::c_int) -> libc::tcflag_t {
    // SAFETY: tcgetattr fills a plain termios.
    unsafe {
        let mut termios: libc::termios = std::mem::zeroed();
        assert_eq!(libc::tcgetattr(fd, &mut termios), 0);
        termios.c_lflag
    }
}

// Closes the two descriptors of `pty`.
pub(crate) fn close_pty((master, slave): (libc::c_int, libc::c_int)) {
    // SAFETY: closing descriptors that `pty` made.
    unsafe {
        libc::close(master);
        libc::close(slave);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "another test's name")]
    fn a_name_that_is_another_real_tests_is_refused() {
        // That test would run in the child, say it ran, and pass without this body.
        in_own_process(
            "signals::tests::die_if_caught_does_nothing_when_nothing_was_caught",
            "",
            &[],
            |_| {},
        );
    }

    #[test]
    fn a_descriptor_that_is_closed_on_exec_is_open_in_the_child_that_is_given_it() {
        // A `File` is opened close-on-exec, as a lock file is. Only the test process opens it:
        // the child runs this far too, and would open one of its own at the number that the
        // one handed over lacks if it did not arrive.
        let file = std::env::var_os(CHILD_OF)
            .is_none()
            .then(|| std::fs::File::open(std::env::current_exe().unwrap()).unwrap());
        let fd = file
            .as_ref()
            .map_or(-1, std::os::unix::io::AsRawFd::as_raw_fd);
        let ran = in_own_process(
            concat!(
                "test_support::tests::",
                "a_descriptor_that_is_closed_on_exec_is_open_in_the_child_that_is_given_it"
            ),
            "",
            &[fd],
            |fds| {
                // Open, and the file it was: the number alone could be some other file's by now.
                let open = std::fs::read_link(format!("/proc/self/fd/{}", fds[0]))
                    .is_ok_and(|path| Some(path) == std::env::current_exe().ok());
                // SAFETY: _exit only ends this process.
                unsafe { libc::_exit(if open { 0 } else { 3 }) };
            },
        )
        .unwrap();
        assert_eq!(ran.ended, Ended::Exit(0));
    }
}
