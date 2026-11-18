//! The board's window: the default of a plain start, and a window that cannot open.

mod common;

use common::board::*;
use common::*;

#[test]
fn a_plain_start_opens_the_window_and_console_alone_does_not() {
    let home = Home::new("board-window");
    let program = Tmp::program("board-window", &THREE);
    // The console alone says nothing of a window (the dummy video driver says there is no
    // picture, once, as soon as a window is up).
    let burn = quit_run(&home, &["--burn", program.arg(), "--console"]);
    assert!(stderr(&burn).ends_with(QUIT_LINE), "{}", stderr(&burn));

    assert!(window_up(&home, &[], &[]), "no window came up");
}

#[test]
fn burn_alone_burns_and_opens_the_window() {
    let home = Home::new("board-burn-window");
    let program = Tmp::program("board-burn-window", &THREE);

    assert!(
        window_up(&home, &["--burn", program.arg()], &[]),
        "no window came up"
    );

    assert_eq!(
        std::fs::read(home.file("firmware.bin")).unwrap(),
        std::fs::read(&program.0).unwrap()
    );
}

#[test]
fn a_window_that_cannot_open_says_to_use_the_console_and_ends() {
    let home = Home::new("board-no-display");
    let program = Tmp::program("board-no-display", &THREE);
    let burn = quit_run(&home, &["--burn", program.arg(), "--console"]);
    assert!(burn.status.success());

    // The plain start's window with no display: SDL falls back to a driver that shows nothing
    // (and that is refused), or fails; either way it ends, with the advice.
    let plain = no_display(&home, None, &[]);
    assert_eq!(plain.status.code(), Some(1), "{}", stderr(&plain));
    assert!(
        stderr(&plain).contains("use --console to run without a window"),
        "{}",
        stderr(&plain)
    );
    // A driver that was asked for is SDL's to open or not: x11 has no display here.
    let x11 = no_display(&home, Some("x11"), &[]);
    assert_eq!(x11.status.code(), Some(1), "{}", stderr(&x11));
    assert!(
        stderr(&x11).contains("x11 not available"),
        "{}",
        stderr(&x11)
    );
    assert!(stderr(&x11).contains("use --console"), "{}", stderr(&x11));
    // A `--window` that was typed has to be dropped, with `--console` or without it: `--console`
    // would keep the window.
    for args in [
        &["--window", "--run", "1000"][..],
        &["--console", "--window"],
    ] {
        let typed = no_display(&home, Some("x11"), args);
        assert_eq!(typed.status.code(), Some(1), "{args:?}: {}", stderr(&typed));
        let said = stderr(&typed);
        assert!(
            said.contains("(drop --window to run without a window)"),
            "{args:?}: {said}"
        );
        assert!(!said.contains("use --console"), "{args:?}: {said}");
    }
}

#[test]
fn a_window_that_was_asked_for_or_a_driver_that_was_chosen_opens_on_offscreen_as_it_did() {
    let home = Home::new("board-offscreen");
    let program = Tmp::program("board-offscreen", &THREE);
    assert!(
        quit_run(&home, &["--burn", program.arg(), "--console"])
            .status
            .success()
    );
    // `--window` is asked for, and `SDL_VIDEODRIVER` is chosen: neither is refused.
    for (driver, args) in [
        (Some("offscreen"), &["--run", "1000"][..]),
        (Some("offscreen"), &["--window", "--run", "1000"]),
        (None, &["--window", "--run", "1000"]),
    ] {
        let output = no_display(&home, driver, args);
        assert!(
            output.status.success(),
            "{driver:?} {args:?}: {}",
            stderr(&output)
        );
    }
    // A scratch run is as it was.
    for driver in [None, Some("offscreen")] {
        let mut command = emulator();
        command
            .args(["--firmware", program.arg(), "--window", "--run", "1000"])
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("XDG_RUNTIME_DIR");
        match driver {
            Some(driver) => command.env("SDL_VIDEODRIVER", driver),
            None => command.env_remove("SDL_VIDEODRIVER"),
        };
        let output = Run::spawn(&mut command, b"", b"").finish();
        assert!(output.status.success(), "{driver:?}: {}", stderr(&output));
    }
}

#[test]
fn the_choice_of_a_video_driver_is_honoured_by_the_boards_default_window() {
    let home = Home::new("board-chosen-driver");
    let program = Tmp::program("board-chosen-driver", &THREE);
    assert!(
        quit_run(&home, &["--burn", program.arg(), "--console"])
            .status
            .success()
    );

    // Chosen with SDL_VIDEODRIVER: the window opens on offscreen, and the run goes on until it
    // is ended.
    let (seen, chosen) = headless_start(&home, Some("offscreen"));
    assert!(seen, "no window came up: {}", stderr(&chosen));
    assert!(chosen.status.code().is_none(), "{chosen:?}");
    // Not chosen: SDL falls back by itself, and that is refused (or fails), and ends at once.
    let (seen, fell_back) = headless_start(&home, None);
    assert!(!seen);
    assert_eq!(fell_back.status.code(), Some(1), "{}", stderr(&fell_back));
}
