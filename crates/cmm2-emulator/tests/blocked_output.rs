//! A run whose output is blocked (a pipe, a terminal, a stalled reader) still ends on a signal.

mod common;

use common::*;
use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::process::Stdio;
use std::time::{Duration, Instant};

// Waits until the emulator is blocked, which an unpaced run, always at work, is not: its state in
// /proc is sleeping for several looks in a row. That is it waiting in a write to a pipe that
// is full.
#[cfg(target_os = "linux")]
fn wait_until_blocked(child: &std::process::Child) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut sleeping = 0;
    while sleeping < 10 {
        let stat = std::fs::read_to_string(format!("/proc/{}/stat", child.id())).unwrap();
        // The state is the field after the command, which is in brackets and may hold anything.
        let state = stat.rsplit_once(") ").unwrap().1.chars().next();
        sleeping = if state == Some('S') { sleeping + 1 } else { 0 };
        assert!(Instant::now() < deadline, "the emulator never blocked");
        std::thread::sleep(Duration::from_millis(20));
    }
}

// Sets UE and TE in USART1_CR1 and then sends 'x' for ever. The address of USART1 is the
// literal at +16.
const SPAM: [u16; 10] = [
    0x4803, 0x2109, 0x6001, 0x2278, 0x6282, 0xE7FD, 0x0000, 0x0000, 0x1000, 0x4001,
];

// A pipe filled to the last byte, so that the next write to it blocks with nothing written.
fn full_pipe() -> (std::io::PipeReader, std::io::PipeWriter) {
    use std::os::fd::AsRawFd;
    let (reader, mut writer) = std::io::pipe().unwrap();
    let fd = writer.as_raw_fd();
    // SAFETY: fcntl on a descriptor this function owns; the flag is shared with the other end
    // of the dup that the child gets, so it is put back before it goes.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        let mut chunk = 4096;
        while chunk > 0 {
            match writer.write(&vec![b'.'; chunk]) {
                Ok(_) => {}
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => chunk /= 2,
                Err(err) => panic!("{err}"),
            }
        }
        libc::fcntl(fd, libc::F_SETFL, flags);
    }
    (reader, writer)
}

// Writes to AIRCR (the literals at +8 and +12) to reset the system, over and over: each reset is
// a line on standard error.
const RESET_FOR_EVER: [u16; 8] = [
    0x4801, 0x4902, 0x6001, 0xE7FD, 0xED0C, 0xE000, 0x0004, 0x05FA,
];

#[cfg(target_os = "linux")]
#[test]
fn a_run_blocked_writing_to_a_pipe_nobody_reads_ends_on_a_signal() {
    let flash = flash_path("signal-blocked");
    let _ = std::fs::remove_file(&flash);
    let path = temp_program("signal-blocked", &SPAM);
    // Never read from: the pipe is full, and the emulator blocks in its first write.
    let (_reader, writer) = full_pipe();
    let mut child = emulator()
        .args(["--firmware", path.to_str().unwrap()])
        .args(["--console", "--unpaced", "--flash"])
        .arg(&flash)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(writer))
        .spawn_guarded();
    wait_until_blocked(&child);

    // SAFETY: kill only sends the signal to the child, which is still running.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let status = wait_for(&mut child);

    assert_eq!(status.signal(), Some(libc::SIGTERM));
    assert_eq!(std::fs::read(&flash).unwrap().len(), FLASH_FILE_SIZE);
    std::fs::remove_file(&flash).unwrap();
    std::fs::remove_file(&path).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn a_batch_run_blocked_writing_its_trace_to_a_pipe_nobody_reads_ends_on_a_signal() {
    let flash = flash_path("signal-trace-blocked");
    let _ = std::fs::remove_file(&flash);
    let path = temp_program("signal-trace-blocked", &SPAM);
    // The trace of the peripherals goes to stdout, which is full and never read.
    let (_reader, writer) = full_pipe();
    let mut child = emulator()
        .args(["--firmware", path.to_str().unwrap()])
        .args(["--trace-mmio", "--stop-at", "0x08000FF0", "--flash"])
        .arg(&flash)
        .stdout(Stdio::from(writer))
        .spawn_guarded();
    wait_until_blocked(&child);

    // SAFETY: kill only sends the signal to the child, which is still running.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let status = wait_for(&mut child);

    assert_eq!(status.signal(), Some(libc::SIGTERM));
    assert_eq!(std::fs::read(&flash).unwrap().len(), FLASH_FILE_SIZE);
    std::fs::remove_file(&flash).unwrap();
    std::fs::remove_file(&path).unwrap();
}

// What a run does with a trace that goes to a pipe or a FIFO that nobody reads from: the first
// signal ends it, and it saves.
#[cfg(target_os = "linux")]
#[test]
fn a_run_blocked_writing_its_trace_file_to_a_stalled_reader_ends_on_a_signal() {
    use std::os::unix::fs::OpenOptionsExt;
    // /dev/stdout is the full pipe the run is given as its stdout, and a FIFO is one with a
    // reader that holds it open and never reads. Each is a file the trace is written to.
    for (name, fifo) in [("trace-stdout", false), ("trace-fifo", true)] {
        let flash = flash_path(name);
        let _ = std::fs::remove_file(&flash);
        let path = temp_program(name, &THREE);
        let (_reader, writer) = full_pipe();
        let trace_fifo = temp_path(name, "fifo");
        let mut _held = None;
        let trace = if fifo {
            make_fifo(&trace_fifo);
            // A reader that never reads, opened first so that the run's open does not wait.
            _held = Some(
                std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&trace_fifo)
                    .unwrap(),
            );
            trace_fifo.to_str().unwrap().to_string()
        } else {
            "/dev/stdout".to_string()
        };
        let mut child = emulator()
            .args(["--firmware", path.to_str().unwrap(), "--run", "100000000"])
            .args(["--trace", &trace, "--flash"])
            .arg(&flash)
            .stdout(Stdio::from(writer))
            .spawn_guarded();
        wait_until_blocked(&child);

        // SAFETY: kill only sends the signal to the child, which is still running.
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
        let status = wait_for(&mut child);

        assert_eq!(status.signal(), Some(libc::SIGTERM), "{name}");
        assert_eq!(
            std::fs::read(&flash).unwrap().len(),
            FLASH_FILE_SIZE,
            "{name}"
        );
        std::fs::remove_file(&flash).unwrap();
        std::fs::remove_file(&path).unwrap();
        if fifo {
            std::fs::remove_file(&trace_fifo).unwrap();
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_run_blocked_writing_to_a_standard_error_nobody_reads_ends_on_a_signal() {
    let flash = flash_path("signal-stderr");
    let _ = std::fs::remove_file(&flash);
    let path = temp_program("signal-stderr", &RESET_FOR_EVER);
    // The pipe is full and never read, and the guest is a line of it for each reset.
    let (_reader, writer) = full_pipe();
    let mut child = emulator()
        .args(["--firmware", path.to_str().unwrap()])
        .args(["--console", "--unpaced", "--flash"])
        .arg(&flash)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(writer))
        .spawn_guarded();
    wait_until_blocked(&child);

    // SAFETY: kill only sends the signal to the child, which is still running.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let status = wait_for(&mut child);

    assert_eq!(status.signal(), Some(libc::SIGTERM));
    assert_eq!(std::fs::read(&flash).unwrap().len(), FLASH_FILE_SIZE);
    std::fs::remove_file(&flash).unwrap();
    std::fs::remove_file(&path).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn a_run_blocked_writing_to_a_terminal_nobody_reads_ends_on_a_signal() {
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
    let flash = flash_path("signal-tty");
    let _ = std::fs::remove_file(&flash);
    let path = temp_program("signal-tty", &SPAM);
    // SAFETY: dup of a descriptor this test owns; the new one is handed to the child.
    let tty = unsafe { OwnedFd::from_raw_fd(libc::dup(slave)) };
    // Nothing reads the other end, so the terminal fills and the run waits in a write.
    let mut child = emulator()
        .args(["--firmware", path.to_str().unwrap()])
        .args(["--console", "--unpaced", "--flash"])
        .arg(&flash)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(tty))
        .spawn_guarded();
    wait_until_blocked(&child);

    // SAFETY: kill only sends the signal to the child, which is still running.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let status = wait_for(&mut child);

    assert_eq!(status.signal(), Some(libc::SIGTERM));
    assert_eq!(std::fs::read(&flash).unwrap().len(), FLASH_FILE_SIZE);
    // SAFETY: closing the two descriptors made above.
    unsafe {
        libc::close(master);
        libc::close(slave);
    }
    std::fs::remove_file(&flash).unwrap();
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn output_to_the_master_of_a_terminal_arrives() {
    use std::os::fd::{FromRawFd, OwnedFd};
    let (mut master, mut slave) = (0, 0);
    // SAFETY: openpty fills in two descriptors; the slave is put in raw mode through a termios
    // that was got from it.
    unsafe {
        assert_eq!(
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null()
            ),
            0
        );
        let mut termios: libc::termios = std::mem::zeroed();
        libc::tcgetattr(slave, &mut termios);
        libc::cfmakeraw(&mut termios);
        libc::tcsetattr(slave, libc::TCSANOW, &termios);
    }
    // SAFETY: dup of a descriptor this test owns; the new one is handed to the child.
    let as_stdout = unsafe { OwnedFd::from_raw_fd(libc::dup(master)) };

    // A run that prints its summary to the master, which is not a thing to be reopened: that
    // would make a new pair, and the summary would go nowhere.
    let path = temp_program("master", &THREE);
    let status = emulator()
        .args(["--firmware", path.to_str().unwrap(), "--run", "1000"])
        .stdout(Stdio::from(as_stdout))
        .status()
        .expect("failed to run cmm2-emulator");
    assert!(status.success());

    let mut got = vec![0u8; 4096];
    // What was sent is waiting, or nothing ever will be: a wait of a few seconds says which, and
    // a test that fails does not hang.
    let mut waiting = libc::pollfd {
        fd: slave,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: poll on one live pollfd.
    assert_eq!(
        unsafe { libc::poll(&mut waiting, 1, 5000) },
        1,
        "nothing arrived"
    );
    // SAFETY: reading into a live buffer from the slave, which has data waiting.
    let count = unsafe { libc::read(slave, got.as_mut_ptr().cast(), got.len()) };
    let got = String::from_utf8_lossy(&got[..usize::try_from(count).unwrap()]).into_owned();
    assert!(got.starts_with("stopped: instruction limit\n"), "{got:?}");
    assert!(got.contains("instructions 1000\n"), "{got:?}");
    // SAFETY: closing the two descriptors made above.
    unsafe {
        libc::close(master);
        libc::close(slave);
    }
    std::fs::remove_file(&path).unwrap();
}
