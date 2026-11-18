//! Signals ending a run, and what is written and put back when they do.

mod common;

use common::*;
use std::io::Read;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::Stdio;
use std::time::{Duration, Instant};

// The tests that read /proc look at what the kernel says about the emulator's own process and
// threads, and are for Linux.

// The signals the process has handlers for, from /proc, as a bit for each signal number - 1.
#[cfg(target_os = "linux")]
fn proc_mask(pid: u32, thread: u32, field: &str) -> u64 {
    let status = std::fs::read_to_string(format!("/proc/{pid}/task/{thread}/status")).unwrap();
    let line = status
        .lines()
        .find_map(|line| line.strip_prefix(field))
        .unwrap();
    u64::from_str_radix(line.trim(), 16).unwrap()
}

// Waits until the emulator has its handlers in, which is the first thing it does, so that a
// signal sent after this is the one it is waiting for and not one that ends it.
#[cfg(target_os = "linux")]
fn wait_until_catching(child: &std::process::Child) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while proc_mask(child.id(), child.id(), "SigCgt:") & 1 << (libc::SIGTERM - 1) == 0 {
        assert!(
            Instant::now() < deadline,
            "the emulator never took its handlers"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

// Starts the emulator on `code` with `args`, with SIGHUP ignored as nohup leaves it.
fn spawn_with_sighup_ignored(name: &str, code: &[u16], args: &[&str]) -> Guard {
    let path = temp_program(name, code);
    let mut command = emulator();
    command
        .args(["--firmware", path.to_str().unwrap()])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null());
    // SAFETY: signal with SIG_IGN is async-signal-safe and runs between fork and exec.
    unsafe {
        command.pre_exec(|| {
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
            Ok(())
        });
    }
    command.spawn_guarded()
}

// The local mode flags of the terminal on `fd`.
fn lflag(fd: libc::c_int) -> libc::tcflag_t {
    // SAFETY: tcgetattr fills a plain termios.
    unsafe {
        let mut termios: libc::termios = std::mem::zeroed();
        assert_eq!(libc::tcgetattr(fd, &mut termios), 0);
        termios.c_lflag
    }
}

#[test]
fn a_signal_ends_a_live_run_and_the_files_are_written() {
    for (name, signal) in [
        ("signal-int", libc::SIGINT),
        ("signal-term", libc::SIGTERM),
        ("signal-hup", libc::SIGHUP),
    ] {
        let flash = flash_path(name);
        let _ = std::fs::remove_file(&flash);
        let (card, image) = sd_card(name);
        let before = inode(&card);
        let mut child = start_writing_run(name, &flash, &card);

        // SAFETY: kill only sends the signal to the child, which is still running.
        assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
        let status = wait_for(&mut child);

        // The run ends by the signal itself, once its files are written.
        assert_eq!(status.signal(), Some(signal), "{name}: {status:?}");
        assert_eq!(
            std::fs::read(&flash).unwrap().len(),
            FLASH_FILE_SIZE,
            "{name}"
        );
        assert!(written_back(&card, &image, before), "{name}");
        std::fs::remove_file(&flash).unwrap();
        std::fs::remove_file(&card).unwrap();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_signal_ends_a_batch_run_with_no_limit_and_the_flash_is_written() {
    for (name, signal) in [
        ("signal-batch-term", libc::SIGTERM),
        ("signal-batch-int", libc::SIGINT),
    ] {
        let flash = flash_path(name);
        let _ = std::fs::remove_file(&flash);
        let path = temp_program(name, &count_in_flash(SPIN));
        let trace = temp_path(name, "trace");
        // A stop address the guest never reaches, and no limit: only the signal ends it.
        let mut child = emulator()
            .args([
                "--firmware",
                path.to_str().unwrap(),
                "--stop-at",
                "0x08000FF0",
            ])
            .arg("--flash")
            .arg(&flash)
            .arg("--trace")
            .arg(&trace)
            .stdout(Stdio::piped())
            .spawn_guarded();
        // The trace file has something in it once the run is under way. A signal before that
        // would end the process without a save, as it has nothing to save.
        let deadline = Instant::now() + Duration::from_secs(30);
        while std::fs::metadata(&trace).map_or(0, |meta| meta.len()) == 0 {
            assert!(Instant::now() < deadline, "the run never began");
            std::thread::sleep(Duration::from_millis(10));
        }

        // SAFETY: kill only sends the signal to the child, which is still running.
        assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
        let status = wait_for(&mut child);
        let mut summary = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut summary)
            .unwrap();

        assert_eq!(status.signal(), Some(signal), "{name}: {status:?}");
        assert!(
            summary.starts_with("stopped: interrupted by a signal\n"),
            "{summary}"
        );
        assert_eq!(options_word(&std::fs::read(&flash).unwrap()), 0, "{name}");
        std::fs::remove_file(&flash).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(&trace).unwrap();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_signal_that_was_ignored_when_the_emulator_started_is_still_ignored() {
    let mut child = spawn_with_sighup_ignored("signal-nohup", &THREE, &["--console"]);
    wait_until_catching(&child);
    std::thread::sleep(Duration::from_millis(500));

    // SAFETY: kill only sends the signal to the child, which is still running.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGHUP) }, 0);
    std::thread::sleep(Duration::from_millis(500));
    assert!(child.try_wait().unwrap().is_none(), "SIGHUP ended the run");

    // The others are not ignored, so they still end it.
    // SAFETY: as above.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    assert_eq!(wait_for(&mut child).signal(), Some(libc::SIGTERM));
    std::fs::remove_file(temp_path("signal-nohup", "bin")).unwrap();
}

#[test]
fn two_signals_in_a_row_leave_the_terminal_as_it_was() {
    use std::os::fd::{FromRawFd, OwnedFd};
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
    let cooked = lflag(slave);
    let path = temp_program("signal-pty", &ECHO);
    // SAFETY: dup of a descriptor this test owns; the new one is handed to the child.
    let stdin = unsafe { OwnedFd::from_raw_fd(libc::dup(slave)) };
    let mut child = emulator()
        .args(["--firmware", path.to_str().unwrap(), "--console"])
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::null())
        .spawn_guarded();
    let deadline = Instant::now() + Duration::from_secs(20);
    while lflag(slave) == cooked {
        assert!(Instant::now() < deadline, "the terminal never went raw");
        std::thread::sleep(Duration::from_millis(5));
    }

    for _ in 0..2 {
        // SAFETY: kill only sends the signal to the child, which is still running.
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    }
    let status = wait_for(&mut child);

    assert_eq!(status.signal(), Some(libc::SIGTERM));
    assert_eq!(lflag(slave), cooked, "the terminal is still raw");
    // SAFETY: closing the two descriptors made above.
    unsafe {
        libc::close(master);
        libc::close(slave);
    }
    std::fs::remove_file(&path).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn the_threads_the_run_makes_have_the_signals_blocked_and_the_main_one_takes_them() {
    let flash = flash_path("signal-threads");
    let _ = std::fs::remove_file(&flash);
    let path = temp_program("signal-threads", &ECHO);
    let mut child = emulator()
        .args([
            "--firmware",
            path.to_str().unwrap(),
            "--console",
            "--window",
        ])
        .arg("--flash")
        .arg(&flash)
        .env("SDL_VIDEODRIVER", "dummy")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn_guarded();
    // The guest says "ready" once everything is made and the run has started.
    let mut out = child.stdout.take().unwrap();
    let mut seen = Vec::new();
    let mut byte = [0];
    while !seen.ends_with(b"ready\r\n") && out.read(&mut byte).is_ok_and(|n| n == 1) {
        seen.push(byte[0]);
    }
    let pid = child.id();
    let ending: u64 = 1 << (libc::SIGINT - 1) | 1 << (libc::SIGTERM - 1) | 1 << (libc::SIGHUP - 1);
    let threads: Vec<u32> = std::fs::read_dir(format!("/proc/{pid}/task"))
        .unwrap()
        .map(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_str()
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect();
    assert!(threads.len() > 1, "{threads:?}: the stdin thread is one");
    for thread in threads {
        let blocked = proc_mask(pid, thread, "SigBlk:") & ending;
        let expected = if thread == pid { 0 } else { ending };
        assert_eq!(blocked, expected, "thread {thread}");
    }

    // SAFETY: kill only sends the signal to the child, which is still running.
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGTERM) }, 0);
    let status = wait_for(&mut child);

    assert_eq!(status.signal(), Some(libc::SIGTERM));
    assert_eq!(std::fs::read(&flash).unwrap().len(), FLASH_FILE_SIZE);
    std::fs::remove_file(&flash).unwrap();
    std::fs::remove_file(&path).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn a_signal_ends_a_load_that_is_stuck_at_once() {
    // The firmware is a FIFO with no writer, so the load waits in its open for ever. Nothing
    // has been run, so there is nothing to save: the signal ends the process, by itself.
    for (name, signal) in [("stuck-int", libc::SIGINT), ("stuck-term", libc::SIGTERM)] {
        let fifo = temp_path(name, "fifo");
        make_fifo(&fifo);
        let mut child = emulator()
            .args(["--firmware", fifo.to_str().unwrap(), "--run", "10"])
            .spawn_guarded();
        wait_until_catching(&child);
        std::thread::sleep(Duration::from_millis(200));
        assert!(child.try_wait().unwrap().is_none(), "{name}: not stuck");

        // SAFETY: kill only sends the signal to the child, which is still running.
        assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
        let status = wait_for(&mut child);

        assert_eq!(status.signal(), Some(signal), "{name}");
        std::fs::remove_file(&fifo).unwrap();
    }
}
