//! What the tests of the board share: a board directory of a test's own, files of its own, and
//! runs of the emulator that are never left going.

#![allow(dead_code)]

use super::*;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Output, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// A board directory that is this test's own, removed at the end, a failed test too. The
// directory is not made: the emulator makes it when it burns.
pub struct Home(pub PathBuf);

impl Home {
    pub fn new(name: &str) -> Home {
        let home = temp_path(name, "home");
        let _ = std::fs::remove_dir_all(&home);
        Home(home)
    }
    pub fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// A file of the test's in /tmp, removed at the end, a failed test too.
pub struct Tmp(pub PathBuf);

impl Tmp {
    pub fn program(name: &str, code: &[u16]) -> Tmp {
        Tmp(temp_program(name, code))
    }
    pub fn card(name: &str) -> (Tmp, Vec<u8>) {
        let (path, image) = sd_card(name);
        (Tmp(path), image)
    }
    pub fn file(name: &str, extension: &str) -> Tmp {
        let path = temp_path(name, extension);
        let _ = std::fs::remove_file(&path);
        Tmp(path)
    }
    pub fn arg(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        // An empty directory, for the one that is a mount point.
        let _ = std::fs::remove_dir(&self.0);
    }
}

pub fn on(home: &Home) -> std::process::Command {
    let mut command = emulator();
    command
        .env(HOME_VARIABLE, &home.0)
        .env("SDL_VIDEODRIVER", "dummy");
    command
}

// How long a run of these tests may take before it is a failure and not a wait.
pub const DEADLINE: Duration = Duration::from_secs(120);

// A run of the emulator that is killed, by its handle, when this is dropped, so that a test that
// fails, or times out, never leaves one going. Its standard output and error are read as it
// goes, and `said` is told when `until` has come out of the one and `until_err` out of the other.
pub struct Run {
    child: Option<Guard>,
    out: Arc<Mutex<Vec<u8>>>,
    err: Arc<Mutex<Vec<u8>>>,
    said: mpsc::Receiver<()>,
    readers: Vec<std::thread::JoinHandle<()>>,
}

pub fn watch(
    mut stream: impl Read + Send + 'static,
    pattern: &[u8],
    into: Arc<Mutex<Vec<u8>>>,
    said: mpsc::Sender<()>,
) -> std::thread::JoinHandle<()> {
    let pattern = pattern.to_vec();
    std::thread::spawn(move || {
        let mut byte = [0];
        while stream.read(&mut byte).is_ok_and(|n| n == 1) {
            let mut text = into.lock().unwrap();
            text.push(byte[0]);
            if !pattern.is_empty() && text.ends_with(&pattern) {
                let _ = said.send(());
            }
        }
    })
}

impl Run {
    pub fn start(home: &Home, args: &[&str], until: &[u8], until_err: &[u8]) -> Run {
        Run::spawn(on(home).args(args), until, until_err)
    }

    pub fn spawn(command: &mut std::process::Command, until: &[u8], until_err: &[u8]) -> Run {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn_guarded();
        let (sender, said) = mpsc::channel();
        let out = Arc::new(Mutex::new(Vec::new()));
        let err = Arc::new(Mutex::new(Vec::new()));
        let readers = vec![
            watch(
                child.stdout.take().unwrap(),
                until,
                out.clone(),
                sender.clone(),
            ),
            watch(child.stderr.take().unwrap(), until_err, err.clone(), sender),
        ];
        Run {
            child: Some(child),
            out,
            err,
            said,
            readers,
        }
    }

    // Waits for what `start` was told to wait for.
    pub fn wait_said(&self) {
        let said = self.said.recv_timeout(Duration::from_secs(30));
        assert!(said.is_ok(), "the run never said it was going");
    }

    pub fn quit_key(&mut self) {
        let stdin = self.child.as_mut().unwrap().stdin.as_mut().unwrap();
        let _ = stdin.write_all(b"\x1D");
    }

    pub fn signal(&self, signal: libc::c_int) {
        let pid = self.child.as_ref().unwrap().id() as libc::pid_t;
        // SAFETY: kill with the id of the child this test started and has not yet waited for.
        unsafe { libc::kill(pid, signal) };
    }

    // Waits for the run to end by itself, as far as the deadline, and returns what it said.
    pub fn finish(mut self) -> Output {
        let mut child = self.child.take().unwrap().release();
        drop(child.stdin.take());
        let status = wait_for_end(&mut child);
        // The pipes close with the process, and the readers have what they held.
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
        let take = |from: &Arc<Mutex<Vec<u8>>>| from.lock().unwrap().clone();
        Output {
            status,
            stdout: take(&self.out),
            stderr: take(&self.err),
        }
    }
}

pub fn wait_for_end(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the run did not end");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// A run that ends by itself: with its quit key already typed for a live run on the console, so
// one that is on the board and has the console ends at its first slice.
pub fn quit_run(home: &Home, args: &[&str]) -> Output {
    let mut run = Run::start(home, args, b"", b"");
    run.quit_key();
    run.finish()
}

// A run that ends by itself (a limit, say), with nothing typed to it.
pub fn run_to_end(home: &Home, args: &[&str]) -> Output {
    Run::start(home, args, b"", b"").finish()
}

// A live run that is ended with the quit key once it has said `until`.
pub fn quit_after(home: &Home, args: &[&str], until: &[u8]) -> Output {
    let mut run = Run::start(home, args, until, b"");
    run.wait_said();
    run.quit_key();
    run.finish()
}

// A run with no console that goes on until a signal: waits for `until` on its standard error
// (which says a window is up), ends it with SIGTERM, and says whether it was seen.
// The board's plain start (its default window) with SDL given no display and `driver` (none if
// None) as `SDL_VIDEODRIVER`: whether a window came up (`no picture` on standard error), the run
// being ended with SIGTERM, which is also how a run that was refused ends up not being waited for.
pub fn headless_start(home: &Home, driver: Option<&str>) -> (bool, Output) {
    let mut command = on(home);
    command
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("XDG_RUNTIME_DIR");
    match driver {
        Some(driver) => command.env("SDL_VIDEODRIVER", driver),
        None => command.env_remove("SDL_VIDEODRIVER"),
    };
    let run = Run::spawn(&mut command, b"", b"no picture");
    let seen = run.said.recv_timeout(Duration::from_secs(30)).is_ok();
    run.signal(libc::SIGTERM);
    (seen, run.finish())
}

pub fn window_up(home: &Home, args: &[&str], envs: &[(&str, &str)]) -> bool {
    let mut command = on(home);
    command.args(args);
    for (name, value) in envs {
        command.env(name, value);
    }
    let run = Run::spawn(&mut command, b"", b"no picture");
    let seen = run.said.recv_timeout(Duration::from_secs(30)).is_ok();
    run.signal(libc::SIGTERM);
    let output = run.finish();
    assert!(
        output.status.code().is_none(),
        "ended by the signal: {output:?}"
    );
    seen
}

pub const QUIT_LINE: &str = "cmm2-emulator: quit key (Ctrl-]) pressed\n";

// What a burning run says of what it burned, the program being `size` bytes with the initial SP
// and reset handler that `temp_program` gives.
pub fn burned_line(size: usize) -> String {
    format!(
        "cmm2-emulator: burned {size} bytes (initial SP 0x20008000, reset handler 0x08000101)\n"
    )
}

pub fn counter(home: &Home) -> u32 {
    options_word(&std::fs::read(home.file("settings.flash")).unwrap())
}

// Burns a program that counts its boots in the saved options, and starts it once.
pub fn burn_counter(home: &Home, name: &str) {
    let program = Tmp::program(name, &count_in_flash(SPIN));
    let output = quit_run(home, &["--burn", program.arg(), "--console"]);
    assert!(output.status.success(), "{}", stderr(&output));
}

// A run of the board with SDL given no display, and `driver` as its video driver (none if None).
pub fn no_display(home: &Home, driver: Option<&str>, args: &[&str]) -> Output {
    let mut command = on(home);
    command
        .args(args)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("XDG_RUNTIME_DIR");
    match driver {
        Some(driver) => command.env("SDL_VIDEODRIVER", driver),
        None => command.env_remove("SDL_VIDEODRIVER"),
    };
    Run::spawn(&mut command, b"", b"").finish()
}

// Runs `script` in a private mount namespace with a tmpfs of `size` on its first argument; the
// arguments after it are the script's. None, with a note, where the machine does not allow it.
pub fn in_tmpfs(size: &str, script: &str, args: &[&str]) -> Option<Output> {
    let script = format!("mount -t tmpfs -o size={size} tmpfs \"$1\" || exit 90\n{script}");
    // In a PID namespace that is killed with its first process, so that nothing a script
    // started outlives it.
    let output = std::process::Command::new("unshare")
        // The script sets the board's directory itself; the other places it could be looked for
        // are as impossible as they are for `emulator()`.
        .env("XDG_DATA_HOME", NO_BOARD)
        .env("HOME", NO_BOARD)
        .env("USERPROFILE", NO_BOARD)
        .args([
            "-rm",
            "--pid",
            "--fork",
            "--kill-child",
            "bash",
            "-c",
            &script,
            "bash",
        ])
        .args(args)
        .output();
    match output {
        Ok(output) if output.status.code() == Some(90) || stderr(&output).contains("unshare:") => {
            eprintln!("skipped: no tmpfs in a namespace here: {}", stderr(&output));
            None
        }
        Ok(output) => Some(output),
        Err(err) => {
            eprintln!("skipped: no unshare here: {err}");
            None
        }
    }
}
