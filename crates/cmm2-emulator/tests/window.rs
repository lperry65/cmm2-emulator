//! --window and the live modes that go with it.

mod common;

use common::*;

const LIMIT_LINE: &str = "cmm2-emulator: instruction limit reached\n";

#[test]
fn window_shows_the_screen_and_ends_with_the_run() {
    let output = run_program("window", &SCREEN, &["--window", "--run", "40"]);

    assert!(output.status.success());
    assert_eq!(stderr(&output), LIMIT_LINE);
}

#[test]
fn window_says_once_that_there_is_no_picture() {
    let output = run_program("window-blank", &THREE, &["--window", "--run", "400000"]);

    assert!(output.status.success());
    assert_eq!(
        stderr(&output),
        format!("cmm2-emulator: no picture: the LTDC is not enabled\n{LIMIT_LINE}")
    );
}

#[test]
fn window_goes_with_the_console() {
    let output = run_program(
        "window-console",
        &SCREEN,
        &["--window", "--console", "--run", "40"],
    );

    assert!(output.status.success());
    assert_eq!(stderr(&output), LIMIT_LINE);
}

#[test]
fn window_keys_can_go_to_the_console() {
    let output = run_program(
        "window-console-keys",
        &SCREEN,
        &["--window", "--console-keys", "--run", "40"],
    );

    assert!(output.status.success());
    assert_eq!(stderr(&output), LIMIT_LINE);
}

#[test]
fn unpaced_goes_with_the_live_modes() {
    let output = run_program(
        "window-unpaced",
        &SCREEN,
        &["--window", "--unpaced", "--run", "40"],
    );

    assert!(output.status.success());
    assert_eq!(stderr(&output), LIMIT_LINE);
}
