//! Errors of the run, the trace, the screenshot and the save, all said and in order.

mod common;

use common::*;
use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::process::Stdio;
use std::time::{Duration, Instant};

#[test]
fn a_trace_that_cannot_be_written_does_not_hide_a_fault() {
    // /dev/full takes the open and refuses every write.
    let live = run_program(
        "trace-full-live",
        &UNDEFINED,
        &["--console", "--trace", "/dev/full"],
    );
    let lines: Vec<String> = stderr(&live).lines().map(str::to_string).collect();
    assert_eq!(live.status.code(), Some(1));
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(!lines[0].contains("writing the trace"), "{lines:?}");
    assert!(
        lines[1].starts_with("cmm2-emulator: writing the trace: "),
        "{lines:?}"
    );

    // A batch run says the fault in its summary, and the trace error after it.
    let batch = run_program(
        "trace-full-batch",
        &UNDEFINED,
        &["--run", "100", "--trace", "/dev/full"],
    );
    assert_eq!(batch.status.code(), Some(1));
    assert!(stdout(&batch).contains("stopped: "), "{}", stdout(&batch));
    assert!(
        stderr(&batch).starts_with("cmm2-emulator: writing the trace: "),
        "{}",
        stderr(&batch)
    );
}

#[test]
fn the_errors_of_the_trace_and_the_screenshot_are_both_said() {
    // Neither hides the other: the trace cannot be written, and there is no screen to take a
    // picture of.
    let output = run_program(
        "both-errors",
        &THREE,
        &[
            "--run",
            "1000",
            "--trace",
            "/dev/full",
            "--screenshot",
            "/nonexistent/s.ppm",
        ],
    );

    assert_eq!(output.status.code(), Some(1));
    let lines: Vec<String> = stderr(&output).lines().map(str::to_string).collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].starts_with("cmm2-emulator: writing the trace: "),
        "{lines:?}"
    );
    assert!(
        lines[1].starts_with("cmm2-emulator: no screenshot: "),
        "{lines:?}"
    );
}

// A save that fails only at the end, which the checks at the start cannot see: the directory
// of the flash file is made unwritable once the run is under way, and then a signal ends it.
// With the screenshot that cannot be taken, that is two errors, said in the order they came.
#[cfg(target_os = "linux")]
#[test]
fn a_save_that_fails_at_the_end_is_said_after_the_screenshot() {
    if root() {
        return;
    }
    let dir = temp_path("late-save", "d");
    std::fs::create_dir(&dir).unwrap();
    let flash = dir.join("board.flash");
    let trace = temp_path("late-save", "trace");
    let path = temp_program("late-save", &THREE);
    // Removed when the test ends, one that fails with the directory read-only included.
    let _cleanup = Cleanup(vec![dir.clone(), trace.clone(), path.clone()]);
    let mut child = emulator()
        .args([
            "--firmware",
            path.to_str().unwrap(),
            "--stop-at",
            "0x08000FF0",
        ])
        .arg("--flash")
        .arg(&flash)
        .args(["--screenshot", "/nonexistent/s.ppm"])
        .arg("--trace")
        .arg(&trace)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn_guarded();
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::fs::metadata(&trace).map_or(0, |meta| meta.len()) == 0 {
        assert!(Instant::now() < deadline, "the run never began");
        std::thread::sleep(Duration::from_millis(10));
    }
    mode(&dir, 0o555);

    // SAFETY: kill only sends the signal to the child, which is still running.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let status = wait_for(&mut child);
    let mut said = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut said)
        .unwrap();

    assert_eq!(status.signal(), Some(libc::SIGTERM));
    let lines: Vec<&str> = said.lines().collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].starts_with("cmm2-emulator: no screenshot: "),
        "{lines:?}"
    );
    assert_eq!(
        lines[1],
        format!(
            "cmm2-emulator: {}: Permission denied (os error 13)",
            flash.display()
        )
    );
}
