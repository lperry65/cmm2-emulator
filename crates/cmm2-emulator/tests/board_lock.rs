//! A board can only be switched on once.

mod common;

use common::board::*;
use common::*;

#[test]
fn a_board_that_is_switched_on_cannot_be_switched_on_again() {
    let home = Home::new("board-lock");
    let program = Tmp::program("board-lock", &ECHO);
    let mut first = Run::start(
        &home,
        &["--burn", program.arg(), "--console"],
        b"ready\r\n",
        b"",
    );
    first.wait_said();
    let before = std::fs::read(home.file("firmware.bin")).unwrap();
    let other = Tmp::program("board-lock-other", &THREE);

    for args in [&["--console"][..], &["--burn", other.arg(), "--console"]] {
        let output = quit_run(&home, args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty());
        let said = stderr(&output);
        assert!(said.contains("already switched on"), "{said}");
        assert!(said.contains(&home.0.display().to_string()), "{said}");
    }
    assert_eq!(std::fs::read(home.file("firmware.bin")).unwrap(), before);

    first.quit_key();
    assert!(first.finish().status.success());
    // Switched off, it can be switched on again.
    assert!(quit_run(&home, &["--console"]).status.success());
}
