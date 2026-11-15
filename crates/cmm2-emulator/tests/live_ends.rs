//! Every way a live run ends says why, on its own line, once the terminal is back as it was. The
//! runs are on a pseudo-terminal, as that is where raw mode is and where a line can go missing.

#![cfg(unix)]

mod common;

use common::*;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

// How long a test waits for the emulator before it gives up.
const PATIENCE: Duration = Duration::from_secs(20);

// The emulator running `--console` on a pseudo-terminal: its stdin, stdout and stderr are the
// slave, and what it writes is read from the master.
struct Session {
    child: Child,
    master: libc::c_int,
    slave: libc::c_int,
    cooked: libc::tcflag_t,
    seen: Vec<u8>,
    image: PathBuf,
    // The file standard error went to, which is removed with the session.
    stderr_file: Option<PathBuf>,
}

// Where the emulator's standard error goes.
enum Stderr {
    // The pseudo-terminal, as the other two are.
    Terminal,
    // A file of the test's.
    File(PathBuf),
    // The terminal opened as /dev/tty, which has a device number of its own.
    DevTty,
}

fn lflag(fd: libc::c_int) -> libc::tcflag_t {
    // SAFETY: tcgetattr fills a plain termios.
    unsafe {
        let mut termios: libc::termios = std::mem::zeroed();
        assert_eq!(libc::tcgetattr(fd, &mut termios), 0);
        termios.c_lflag
    }
}

fn duplicate(fd: libc::c_int) -> Stdio {
    // SAFETY: dup of a descriptor this test owns; the new one is handed to the child.
    Stdio::from(unsafe { OwnedFd::from_raw_fd(libc::dup(fd)) })
}

impl Session {
    fn start(name: &str, code: &[u16], args: &[&str]) -> Session {
        Session::start_with(name, code, args, true, Stderr::Terminal)
    }

    // As `start`, with the terminal for stdin or `/dev/null` in its place, and standard error
    // where `stderr` says.
    fn start_with(
        name: &str,
        code: &[u16],
        args: &[&str],
        terminal_stdin: bool,
        stderr: Stderr,
    ) -> Session {
        Session::try_start_with(name, code, args, terminal_stdin, stderr)
            .expect("failed to run cmm2-emulator")
    }

    // As `start_with`, with the error of not being able to start it given back.
    fn try_start_with(
        name: &str,
        code: &[u16],
        args: &[&str],
        terminal_stdin: bool,
        stderr: Stderr,
    ) -> std::io::Result<Session> {
        let (mut master, mut slave) = (0, 0);
        // SAFETY: openpty fills in two descriptors.
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
        // SAFETY: fcntl on the master this test owns; reads must not wait.
        unsafe { libc::fcntl(master, libc::F_SETFL, libc::O_NONBLOCK) };
        let cooked = lflag(slave);
        let image = temp_program(name, code);
        let mut command = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"));
        command
            .args(["--firmware", image.to_str().unwrap(), "--console"])
            .args(args)
            .stdin(if terminal_stdin {
                duplicate(slave)
            } else {
                Stdio::null()
            })
            .stdout(duplicate(slave))
            .stderr(match &stderr {
                Stderr::File(path) => Stdio::from(std::fs::File::create(path).unwrap()),
                // The child opens /dev/tty for it in `pre_exec`, below.
                Stderr::Terminal | Stderr::DevTty => duplicate(slave),
            });
        if matches!(stderr, Stderr::DevTty) {
            // SAFETY: only async-signal-safe calls between fork and exec: a new session with the
            // slave (stdin) as its terminal, and /dev/tty, which is that, put on descriptor 2.
            unsafe {
                command.pre_exec(|| {
                    libc::setsid();
                    // A sandbox may refuse it, and then the test is skipped.
                    if libc::ioctl(0, libc::TIOCSCTTY, 0) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    let tty = libc::open(c"/dev/tty".as_ptr(), libc::O_WRONLY);
                    if tty < 0 || libc::dup2(tty, 2) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    libc::close(tty);
                    Ok(())
                });
            }
        }
        let child = command.spawn()?;
        let stderr_file = match stderr {
            Stderr::File(path) => Some(path),
            _ => None,
        };
        Ok(Session {
            stderr_file,
            child,
            master,
            slave,
            cooked,
            seen: Vec::new(),
            image,
        })
    }

    // Takes in whatever the emulator has written so far.
    fn pump(&mut self) {
        let mut buffer = [0u8; 4096];
        loop {
            // SAFETY: the pointer and length are those of a live buffer.
            let count = unsafe { libc::read(self.master, buffer.as_mut_ptr().cast(), 4096) };
            if count <= 0 {
                return;
            }
            self.seen.extend_from_slice(&buffer[..count as usize]);
        }
    }

    fn until(&mut self, what: &str, done: impl Fn(&Session) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.pump();
            if done(self) {
                return;
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!(
                    "never saw {what}, got {:?}",
                    String::from_utf8_lossy(&self.seen)
                );
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    // Waits until the guest has said `text` (and the terminal is raw, which is first).
    fn wait_for_text(&mut self, text: &str) {
        self.until(text, |s| {
            lflag(s.slave) != s.cooked && s.seen.windows(text.len()).any(|w| w == text.as_bytes())
        });
    }

    fn type_bytes(&mut self, bytes: &[u8]) {
        // SAFETY: the pointer and length are those of a live slice.
        let wrote = unsafe { libc::write(self.master, bytes.as_ptr().cast(), bytes.len()) };
        assert_eq!(wrote, bytes.len() as isize);
    }

    // What went to the file standard error was on, once the run has ended.
    fn stderr_file_text(&self) -> String {
        std::fs::read_to_string(self.stderr_file.as_ref().unwrap()).unwrap()
    }

    // Waits for the end, and returns the status and all that was written, with the terminal
    // checked to be as it was.
    fn finish(mut self) -> (std::process::ExitStatus, String) {
        let status = wait_for(&mut self.child);
        self.pump();
        assert_eq!(lflag(self.slave), self.cooked, "the terminal is still raw");
        (status, String::from_utf8_lossy(&self.seen).into_owned())
    }
}

impl Drop for Session {
    // A test that fails part way does not leave its emulator running: it is killed by its handle
    // and reaped, which does nothing if it has ended.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // SAFETY: closing the two descriptors made in `start`.
        unsafe {
            libc::close(self.master);
            libc::close(self.slave);
        }
        let _ = std::fs::remove_file(&self.image);
        if let Some(file) = &self.stderr_file {
            let _ = std::fs::remove_file(file);
        }
    }
}

// Waits a short while for a run to still be going on after what was typed.
fn still_running(session: &mut Session) -> bool {
    std::thread::sleep(Duration::from_millis(300));
    session.child.try_wait().unwrap().is_none()
}

// A pause of some hundreds of instructions, then an undefined instruction.
const LATE_FAULT: [u16; 4] = [0x20FF, 0x3801, 0xD1FD, 0xDE00];

#[test]
fn the_quit_key_says_so_on_a_line_of_its_own_and_exits_with_zero() {
    let mut run = Session::start("ends-quit", &ECHO, &[]);
    run.wait_for_text("ready\r\n");
    run.type_bytes(b"a");
    // The key before the quit key got to the guest, which left the cursor part way along a line.
    run.until("the echo", |s| s.seen.ends_with(b"ready\r\nb"));
    run.type_bytes(&[0x1D]);
    let (status, text) = run.finish();

    assert_eq!(status.code(), Some(0));
    assert_eq!(
        text,
        "ready\r\nb\r\ncmm2-emulator: quit key (Ctrl-]) pressed\r\n"
    );
}

#[test]
fn a_guest_that_echoes_the_quit_key_does_not_quit() {
    let mut run = Session::start("ends-echo-quit", &ECHO, &[]);
    run.wait_for_text("ready\r\n");
    // The guest sends back each byte plus one, so 0x1C comes back as the quit key.
    run.type_bytes(&[0x1C]);
    run.until("the quit key from the guest", |s| {
        s.seen.ends_with(b"ready\r\n\x1D")
    });

    assert!(
        still_running(&mut run),
        "the guest's own 0x1D ended the run"
    );
    assert!(!String::from_utf8_lossy(&run.seen).contains("quit key"));
    run.type_bytes(&[0x1D]);
    let (status, text) = run.finish();

    assert_eq!(status.code(), Some(0));
    assert!(
        text.ends_with("cmm2-emulator: quit key (Ctrl-]) pressed\r\n"),
        "{text:?}"
    );
}

#[test]
fn a_signal_says_so_and_the_run_exits_by_it() {
    let mut run = Session::start("ends-term", &ECHO, &[]);
    run.wait_for_text("ready\r\n");
    // SAFETY: kill sends the signal to the child this test started, which is still running.
    assert_eq!(
        unsafe { libc::kill(run.child.id() as i32, libc::SIGTERM) },
        0
    );
    let (status, text) = run.finish();

    assert_eq!(status.signal(), Some(libc::SIGTERM));
    assert_eq!(text, "ready\r\ncmm2-emulator: stopped by SIGTERM\r\n");
}

#[test]
fn stopping_at_an_address_says_so() {
    let run = Session::start("ends-address", &THREE, &["--stop-at", "0x08000104"]);
    let (status, text) = run.finish();

    assert_eq!(status.code(), Some(0));
    assert_eq!(text, "cmm2-emulator: reached address 0x08000104\r\n");
}

#[test]
fn the_instruction_limit_says_so() {
    let run = Session::start("ends-limit", &THREE, &["--run", "1000"]);
    let (status, text) = run.finish();

    assert_eq!(status.code(), Some(0));
    assert_eq!(text, "cmm2-emulator: instruction limit reached\r\n");
}

#[test]
fn a_fault_after_a_while_is_said_on_a_line_of_its_own_and_exits_with_one() {
    let run = Session::start("ends-fault", &LATE_FAULT, &[]);
    let (status, text) = run.finish();

    assert_eq!(status.code(), Some(1));
    assert!(text.starts_with("cmm2-emulator: fault trap: "), "{text:?}");
    assert!(text.contains("UndefInstr"), "{text:?}");
    // Every line ends in CR LF, with the terminal as it was.
    assert!(text.ends_with("\r\n"), "{text:?}");
    assert_eq!(text.matches('\n').count(), 1, "{text:?}");
}

#[test]
fn what_is_said_after_part_of_a_line_starts_a_new_one() {
    // The rule types 'a' at the prompt and the guest answers 'b' with no newline, and then the
    // limit ends the run.
    let run = Session::start(
        "ends-mid-line",
        &ECHO,
        &["--on", "ready", "a", "--run", "400000"],
    );
    let (status, text) = run.finish();

    assert_eq!(status.code(), Some(0));
    assert_eq!(
        text,
        "ready\r\nb\r\ncmm2-emulator: instruction limit reached\r\n"
    );
}

#[test]
fn a_fault_after_the_guest_wrote_part_of_a_line_starts_a_new_one() {
    // The echo guest with an undefined instruction where it would end the line "ready".
    let mut code = ECHO;
    code[13] = 0xDE00;
    let run = Session::start("ends-fault-mid-line", &code, &[]);
    let (status, text) = run.finish();

    assert_eq!(status.code(), Some(1));
    assert!(
        text.starts_with("ready\r\ncmm2-emulator: fault trap: "),
        "{text:?}"
    );
    assert!(text.ends_with("\r\n"), "{text:?}");
}

#[test]
fn a_reset_said_while_the_terminal_is_raw_does_not_step_down_the_screen() {
    let run = Session::start("ends-reset", &REBOOT, &["--run", "200000"]);
    let (status, text) = run.finish();

    assert_eq!(status.code(), Some(0));
    assert_eq!(
        text.matches("cmm2-emulator: system reset\r\n").count(),
        2,
        "{text:?}"
    );
    // Every line feed has its carriage return, which a raw terminal does not add.
    assert!(!text.replace("\r\n", "").contains('\n'), "{text:?}");
}

#[test]
fn a_fault_and_a_failed_save_are_both_said_on_lines_of_their_own() {
    if root() {
        eprintln!("skipped: as root a read-only directory stops nothing");
        return;
    }
    let dir = temp_path("ends-two-errors", "dir");
    std::fs::create_dir(&dir).unwrap();
    // The echo guest, with an undefined instruction in place of its jump back to the loop.
    let mut code = ECHO;
    code[24] = 0xDE00;
    let flash = dir.join("flash.bin");
    let mut run = Session::start(
        "ends-two-errors",
        &code,
        &["--flash", flash.to_str().unwrap()],
    );
    run.wait_for_text("ready\r\n");
    // The save at the end of the run cannot be made in a directory that has become read-only.
    mode(&dir, 0o555);
    run.type_bytes(b"a");
    let (status, text) = run.finish();
    mode(&dir, 0o755);
    std::fs::remove_dir_all(&dir).unwrap();

    assert_eq!(status.code(), Some(1));
    assert!(
        text.starts_with("ready\r\nb\r\ncmm2-emulator: fault trap: "),
        "{text:?}"
    );
    assert!(
        text.contains("\r\ncmm2-emulator: ") && text.contains("flash.bin"),
        "{text:?}"
    );
    assert!(!text.contains("\r\n\r\n"), "{text:?}");
    assert!(text.ends_with("\r\n"), "{text:?}");
}

#[test]
fn the_new_line_is_made_when_only_output_is_a_terminal() {
    let run = Session::start_with(
        "ends-no-stdin",
        &ECHO,
        &["--on", "ready", "a", "--run", "400000"],
        false,
        Stderr::Terminal,
    );
    let (status, text) = run.finish();

    // The terminal is cooked, so it turns the guest's CR LF into CR CR LF.
    assert_eq!(status.code(), Some(0));
    assert_eq!(
        text,
        "ready\r\r\nb\r\ncmm2-emulator: instruction limit reached\r\n"
    );
}

#[test]
fn lines_to_a_file_keep_their_line_feeds_while_the_terminal_is_raw() {
    let file = temp_path("ends-stderr-file", "txt");
    let mut run = Session::start_with(
        "ends-stderr-file",
        &REBOOT,
        &["--run", "200000"],
        true,
        Stderr::File(file),
    );
    let status = wait_for(&mut run.child);
    let said = run.stderr_file_text();

    assert_eq!(status.code(), Some(0));
    assert_eq!(
        said,
        "cmm2-emulator: system reset\ncmm2-emulator: system reset\n\
         cmm2-emulator: instruction limit reached\n"
    );
}

#[test]
fn lines_to_dev_tty_get_their_carriage_returns_while_the_terminal_is_raw() {
    let Ok(run) = Session::try_start_with(
        "ends-dev-tty",
        &REBOOT,
        &["--run", "200000"],
        true,
        Stderr::DevTty,
    ) else {
        eprintln!("skipped: no controlling terminal can be made here");
        return;
    };
    let (status, text) = run.finish();

    assert_eq!(status.code(), Some(0));
    assert_eq!(
        text.matches("cmm2-emulator: system reset\r\n").count(),
        2,
        "{text:?}"
    );
    assert!(!text.replace("\r\n", "").contains('\n'), "{text:?}");
}

#[test]
fn the_line_follows_where_the_guest_left_the_cursor() {
    // The guest sends back each byte plus one: 0x0C comes back as a CR and 0x09 as a LF.
    let mut run = Session::start("ends-cursor", &ECHO, &[]);
    run.wait_for_text("ready\r\n");
    run.type_bytes(&[0x0C]);
    run.until("the CR", |s| s.seen.ends_with(b"ready\r\n\r"));
    run.type_bytes(&[0x1D]);
    let (_, text) = run.finish();
    // A CR on an empty row leaves nothing to start a new row for.
    assert_eq!(
        text,
        "ready\r\n\rcmm2-emulator: quit key (Ctrl-]) pressed\r\n"
    );

    let mut run = Session::start("ends-cursor-lf", &ECHO, &[]);
    run.wait_for_text("ready\r\n");
    run.type_bytes(b"a\x09");
    run.until("the LF", |s| s.seen.ends_with(b"ready\r\nb\n"));
    run.type_bytes(&[0x1D]);
    let (_, text) = run.finish();
    // A raw terminal's LF leaves the column, so a CR brings the line to the start of the row.
    assert_eq!(
        text,
        "ready\r\nb\n\rcmm2-emulator: quit key (Ctrl-]) pressed\r\n"
    );
}

#[test]
fn a_colour_change_before_the_line_leaves_no_empty_row() {
    // The guest sends back each byte plus one: ESC [ 0 m comes from 0x1A, 'Z', '/' and 'l'.
    let mut run = Session::start("ends-sgr", &ECHO, &[]);
    run.wait_for_text("ready\r\n");
    run.type_bytes(&[0x1A, b'Z', b'/', b'l']);
    run.until("the colour change", |s| {
        s.seen.ends_with(b"ready\r\n\x1b[0m")
    });
    run.type_bytes(&[0x1D]);
    let (_, text) = run.finish();

    assert_eq!(
        text,
        "ready\r\n\x1b[0mcmm2-emulator: quit key (Ctrl-]) pressed\r\n"
    );
}

// Types the bytes whose echo (each plus one) is `echoed`, and waits for them to come back.
fn echo(run: &mut Session, echoed: &[u8]) {
    let typed: Vec<u8> = echoed.iter().map(|byte| byte - 1).collect();
    run.type_bytes(&typed);
    let want = echoed.to_vec();
    run.until("the echo", move |s| s.seen.ends_with(&want));
}

#[test]
fn a_line_said_inside_an_escape_sequence_cancels_it_first() {
    // The guest's output ends with ESC, which the line would turn into ESC c, a full reset.
    let mut run = Session::start("ends-open-escape", &ECHO, &[]);
    run.wait_for_text("ready\r\n");
    echo(&mut run, b"\x1b");
    run.type_bytes(&[0x1D]);
    let (_, text) = run.finish();

    assert_eq!(
        text,
        "ready\r\n\x1b\x18\r\ncmm2-emulator: quit key (Ctrl-]) pressed\r\n"
    );
}

#[test]
fn a_cancelled_title_does_not_take_the_line_in() {
    let mut run = Session::start("ends-cancelled-title", &ECHO, &[]);
    run.wait_for_text("ready\r\n");
    echo(&mut run, b"\x1b]0;t\x18");
    run.type_bytes(&[0x1D]);
    let (_, text) = run.finish();

    // CAN ends the title, so the line is shown, on the row the guest left clean.
    assert_eq!(
        text,
        "ready\r\n\x1b]0;t\x18cmm2-emulator: quit key (Ctrl-]) pressed\r\n"
    );
}

#[test]
fn a_line_feed_to_a_cooked_terminal_needs_no_carriage_return_before_the_line() {
    // The guest sends 'b' and a line feed, which the terminal makes CR LF.
    let run = Session::start_with(
        "ends-cooked-lf",
        &ECHO,
        &["--on", "ready", "a\\x09", "--run", "400000"],
        false,
        Stderr::Terminal,
    );
    let (status, text) = run.finish();

    assert_eq!(status.code(), Some(0));
    assert_eq!(
        text,
        "ready\r\r\nb\r\ncmm2-emulator: instruction limit reached\r\n"
    );
}
