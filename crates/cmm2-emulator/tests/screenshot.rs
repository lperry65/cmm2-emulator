//! --screenshot.

mod common;

use common::*;

#[test]
fn screenshot_writes_the_screen_as_a_ppm_file() {
    let image = temp_path("screenshot", "ppm");
    let output = run_program(
        "screenshot",
        &SCREEN,
        &["--run", "40", "--screenshot", image.to_str().unwrap()],
    );

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        std::fs::read(&image).unwrap(),
        b"P6\n1 1\n255\n\xFF\x00\x00"
    );
    std::fs::remove_file(&image).unwrap();
}

#[test]
fn screenshot_of_a_machine_with_no_screen_fails() {
    let image = temp_path("no-screen", "ppm");
    let output = run_program(
        "no-screen",
        &THREE,
        &["--run", "4", "--screenshot", image.to_str().unwrap()],
    );

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "cmm2-emulator: no screenshot: the LTDC is not enabled\n"
    );
    assert!(!image.exists());
}

#[test]
fn screenshot_that_cannot_be_written_fails() {
    let output = run_program(
        "bad-screenshot",
        &SCREEN,
        &["--run", "40", "--screenshot", "/nonexistent/s.ppm"],
    );

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("cmm2-emulator: /nonexistent/s.ppm: "),
        "{stderr}"
    );
}
