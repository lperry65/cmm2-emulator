//! Runs of the board and of scratch machines: which uses the board, and which files each has.

mod common;

use common::board::*;
use common::*;
use std::path::PathBuf;
use std::process::Output;

#[test]
fn burn_and_firmware_together_are_a_usage_error_and_touch_nothing() {
    let home = Home::new("board-burn-firmware");
    let program = Tmp::program("board-burn-firmware", &THREE);
    let output = quit_run(
        &home,
        &[
            "--burn",
            program.arg(),
            "--firmware",
            program.arg(),
            "--run",
            "9",
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).starts_with("cmm2-emulator: --burn puts the firmware"));
    assert!(!home.0.exists());
}

#[test]
fn firmware_is_a_scratch_machine_whatever_else_is_on_the_line() {
    let home = Home::new("board-scratch");
    let program = Tmp::program("board-scratch", &count_in_flash(SPIN));
    let flash = Tmp::file("board-scratch", "flash");
    let (card, _) = Tmp::card("board-scratch");

    // No board yet: none is made, and none is needed.
    for args in [
        &["--run", "100"][..],
        &["--stop-at", "0x08000108"],
        &["--console"],
        &["--run", "100", "--flash", flash.arg()],
        &["--run", "100", "--sd", card.arg()],
        &["--run", "100", "--rng-seed", "1", "--console"],
    ] {
        let output = quit_run(&home, &[&["--firmware", program.arg()][..], args].concat());
        assert!(output.status.success(), "{args:?}: {}", stderr(&output));
        assert!(!home.0.exists(), "{args:?}");
    }
    assert_eq!(options_word(&std::fs::read(&flash.0).unwrap()), 0);

    // `--firmware` alone prints the vector table, and touches nothing.
    let vectors = quit_run(&home, &["--firmware", program.arg()]);
    assert!(vectors.status.success(), "{}", stderr(&vectors));
    assert_eq!(
        stdout(&vectors),
        "initial SP 0x20008000\nreset handler 0x08000101\n"
    );
    assert!(!home.0.exists());
}

#[test]
fn a_board_that_is_there_is_not_changed_by_a_scratch_run() {
    let home = Home::new("board-scratch-there");
    burn_counter(&home, "board-scratch-there");
    let (card, _) = Tmp::card("board-scratch-there");
    let before = std::fs::read(home.file("settings.flash")).unwrap();
    let firmware = std::fs::read(home.file("firmware.bin")).unwrap();
    let other = Tmp::program("board-scratch-other", &ECHO);

    for args in [&["--run", "100"][..], &["--run", "100", "--sd", card.arg()]] {
        let output = quit_run(&home, &[&["--firmware", other.arg()][..], args].concat());
        assert!(output.status.success(), "{}", stderr(&output));
    }

    assert_eq!(std::fs::read(home.file("settings.flash")).unwrap(), before);
    assert_eq!(std::fs::read(home.file("firmware.bin")).unwrap(), firmware);
    assert!(!home.file("sd.path").exists());
}

#[test]
fn a_board_run_takes_a_limit_like_any_other() {
    let home = Home::new("board-run-on-board");
    burn_counter(&home, "board-run-on-board");

    let output = quit_run(&home, &["--run", "100"]);

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(counter(&home), 1);
}

#[test]
fn flash_names_another_settings_file_and_the_boards_own_is_left() {
    let home = Home::new("board-other-flash");
    burn_counter(&home, "board-other-flash");
    let other = Tmp::file("board-other-flash", "flash");
    for _ in 0..2 {
        let output = quit_run(&home, &["--console", "--flash", other.arg()]);
        assert!(output.status.success(), "{}", stderr(&output));
    }

    assert_eq!(options_word(&std::fs::read(&other.0).unwrap()), 1);
    assert_eq!(counter(&home), 0);
}

#[test]
fn sd_and_no_sd_together_are_a_usage_error() {
    let home = Home::new("board-sd-and-no-sd");
    let output = quit_run(&home, &["--sd", "a.img", "--no-sd"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).starts_with("cmm2-emulator: --sd puts a card in the socket"));
    assert!(!home.0.exists());
}

#[test]
fn the_board_is_found_through_xdg_data_home_when_there_is_no_override() {
    struct Data(PathBuf);
    impl Drop for Data {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let data = Data(temp_path("board-xdg", "data"));
    let _ = std::fs::remove_dir_all(&data.0);
    let program = Tmp::program("board-xdg", &THREE);
    let run = |args: &[&str]| -> Output {
        let mut command = emulator();
        command
            .env_remove(HOME_VARIABLE)
            .env("XDG_DATA_HOME", &data.0)
            .env("HOME", "/dev/null/no-home")
            .args(args);
        let mut run = Run::spawn(&mut command, b"", b"");
        run.quit_key();
        run.finish()
    };

    let burn = run(&["--burn", program.arg(), "--console"]);
    let plain = run(&["--console"]);

    assert!(burn.status.success(), "{}", stderr(&burn));
    assert!(plain.status.success(), "{}", stderr(&plain));
    assert!(data.0.join("cmm2-emulator/firmware.bin").is_file());
}

#[test]
fn a_board_directory_that_cannot_be_written_switches_on_but_the_run_cannot_save_its_settings() {
    if root() {
        return;
    }
    let home = Home::new("board-read-only");
    burn_counter(&home, "board-read-only");
    let _writable_again = Cleanup(vec![home.0.clone()]);
    mode(&home.0, 0o500);

    // The board's own settings are in the directory, so the run is refused at the start, as a
    // `--flash` file in an unwritable directory is.
    let refused = quit_run(&home, &["--run", "10"]);
    assert_eq!(refused.status.code(), Some(1), "{}", stderr(&refused));
    let said = stderr(&refused);
    assert!(said.contains("settings.flash"), "{said}");
    assert!(said.contains("its directory is not writable"), "{said}");
    // The lock was taken (the directory is not the problem), and a settings file elsewhere runs.
    let other = Tmp::file("board-read-only", "flash");
    let elsewhere = quit_run(&home, &["--run", "10", "--flash", other.arg()]);
    assert!(elsewhere.status.success(), "{}", stderr(&elsewhere));
}

#[test]
fn an_empty_board_variable_is_an_error_and_not_the_usual_board() {
    let mut command = emulator();
    command.env(HOME_VARIABLE, "").args(["--run", "10"]);
    let output = Run::spawn(&mut command, b"", b"").finish();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("CMM2_EMULATOR_HOME is set but empty"),
        "{}",
        stderr(&output)
    );
}
