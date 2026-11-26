//! --flash: the saved flash and clock, and the files and directories it refuses.

mod common;

use common::*;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::process::Stdio;

#[test]
fn the_flash_file_keeps_the_flash_between_runs() {
    let flash = flash_path("flash-keeps");
    let _ = std::fs::remove_file(&flash);
    let path = flash.to_str().unwrap();
    let program = count_in_flash(SPIN);

    let first = run_program(
        "flash-keeps-1",
        &program,
        &["--run", "100", "--flash", path],
    );
    assert!(first.status.success(), "{}", stderr(&first));
    let saved = std::fs::read(&flash).unwrap();
    assert_eq!(saved.len(), FLASH_FILE_SIZE);
    assert_eq!(options_word(&saved), 0);

    let second = run_program(
        "flash-keeps-2",
        &program,
        &["--run", "100", "--flash", path],
    );
    assert!(second.status.success(), "{}", stderr(&second));
    assert_eq!(options_word(&std::fs::read(&flash).unwrap()), 1);
    std::fs::remove_file(&flash).unwrap();
}

#[test]
fn the_flash_file_is_written_by_a_console_run_too() {
    let flash = flash_path("flash-console");
    let _ = std::fs::remove_file(&flash);
    let path = flash.to_str().unwrap();

    let output = run_program(
        "flash-console",
        &count_in_flash(SPIN),
        &["--console", "--run", "100", "--flash", path],
    );

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(options_word(&std::fs::read(&flash).unwrap()), 0);
    std::fs::remove_file(&flash).unwrap();
}

#[test]
fn the_flash_file_is_written_when_the_run_stops_at_an_address() {
    let flash = flash_path("flash-stop-at");
    let _ = std::fs::remove_file(&flash);
    let path = flash.to_str().unwrap();

    let output = run_program(
        "flash-stop-at",
        &count_in_flash(SPIN),
        &["--stop-at", "0x08000108", "--flash", path],
    );

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(options_word(&std::fs::read(&flash).unwrap()), 0);
    std::fs::remove_file(&flash).unwrap();
}

#[test]
fn the_flash_file_is_written_when_the_guest_faults() {
    for (name, live) in [
        ("flash-fault-batch", &["--run", "100"][..]),
        ("flash-fault-live", &["--console"][..]),
    ] {
        let flash = flash_path(name);
        let _ = std::fs::remove_file(&flash);
        let mut args = vec!["--flash", flash.to_str().unwrap()];
        args.extend_from_slice(live);

        let output = run_program(name, &count_in_flash(0xDE00), &args);

        assert_eq!(output.status.code(), Some(1), "{name}");
        assert_eq!(options_word(&std::fs::read(&flash).unwrap()), 0, "{name}");
        std::fs::remove_file(&flash).unwrap();
    }
}

#[test]
fn the_flash_file_is_written_when_the_quit_key_ends_the_run() {
    let flash = flash_path("flash-quit");
    let _ = std::fs::remove_file(&flash);
    let path = temp_program("flash-quit", &count_in_flash(SPIN));
    let mut child = emulator()
        .args(["--firmware", path.to_str().unwrap(), "--console", "--flash"])
        .arg(&flash)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn_guarded();
    child.stdin.as_mut().unwrap().write_all(b"\x1D").unwrap();
    let status = child.wait().unwrap();

    assert!(status.success());
    assert_eq!(options_word(&std::fs::read(&flash).unwrap()), 0);
    std::fs::remove_file(&flash).unwrap();
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn saving_the_flash_leaves_no_temporary_file() {
    // The temporary file is made beside the flash file, so that directory shows it.
    let dir = temp_path("flash-temp", "d");
    std::fs::create_dir(&dir).unwrap();
    let flash = dir.join("board.flash");

    run_program(
        "flash-temp",
        &THREE,
        &["--run", "10", "--flash", flash.to_str().unwrap()],
    );

    assert!(flash.exists());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_flash_file_of_the_wrong_size_is_an_error_and_is_left_alone() {
    let flash = flash_path("flash-size");
    std::fs::write(&flash, [1, 2, 3]).unwrap();

    let output = run_program(
        "flash-size",
        &THREE,
        &["--run", "10", "--flash", flash.to_str().unwrap()],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        stderr(&output),
        format!(
            "cmm2-emulator: {}: the saved flash and clock for firmware 7.00.00b5 are \
             {FLASH_FILE_SIZE} bytes, or {} with the DS3231's state ({} from the first version \
             that had it), or those sizes less 32 bytes from before the firmware's own block was \
             kept, not 3\n",
            flash.display(),
            FLASH_FILE_SIZE + 10,
            FLASH_FILE_SIZE + 8
        )
    );
    assert_eq!(std::fs::read(&flash).unwrap(), [1, 2, 3]);
    std::fs::remove_file(&flash).unwrap();
}

#[test]
fn a_flash_file_that_cannot_be_read_is_an_error() {
    let output = run_program(
        "flash-unreadable",
        &THREE,
        &[
            "--run",
            "10",
            "--flash",
            std::env::temp_dir().to_str().unwrap(),
        ],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).starts_with("cmm2-emulator: "),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_flash_file_in_a_directory_that_is_not_there_is_an_error_at_the_start() {
    let missing = std::env::temp_dir().join("cmm2-cli-no-such-directory/f.flash");

    let output = run_program(
        "flash-unwritable",
        &THREE,
        &["--run", "10", "--flash", missing.to_str().unwrap()],
    );

    assert_eq!(output.status.code(), Some(1));
    // Found at the start, and not when the run is over and its session is lost.
    assert!(output.stdout.is_empty());
    assert_eq!(
        stderr(&output),
        format!(
            "cmm2-emulator: {}: its directory {} does not exist, so the run could not save its flash and clock\n",
            missing.display(),
            missing.parent().unwrap().display()
        )
    );
}

#[test]
fn a_read_only_flash_file_in_a_writable_directory_is_saved_as_the_save_replaces_it() {
    let flash = flash_path("flash-readonly");
    let _ = std::fs::remove_file(&flash);
    run_program(
        "flash-readonly-1",
        &THREE,
        &["--run", "10", "--flash", flash.to_str().unwrap()],
    );
    mode(&flash, 0o444);
    let before = inode(&flash);

    let output = run_program(
        "flash-readonly-2",
        &THREE,
        &["--run", "10", "--flash", flash.to_str().unwrap()],
    );

    assert!(output.status.success(), "{}", stderr(&output));
    assert_ne!(inode(&flash), before, "the file was replaced");
    assert_eq!(std::fs::metadata(&flash).unwrap().mode() & 0o777, 0o444);
    mode(&flash, 0o644);
    std::fs::remove_file(&flash).unwrap();
}

#[test]
fn a_flash_file_in_a_directory_that_cannot_be_written_is_an_error_at_the_start() {
    if root() {
        return;
    }
    let dir = temp_path("flash-dir-ro", "d");
    std::fs::create_dir(&dir).unwrap();
    let flash = dir.join("board.flash");
    run_program(
        "flash-dir-ro-1",
        &THREE,
        &["--run", "10", "--flash", flash.to_str().unwrap()],
    );
    mode(&dir, 0o555);

    let output = run_program(
        "flash-dir-ro-2",
        &THREE,
        &["--run", "10", "--flash", flash.to_str().unwrap()],
    );

    mode(&dir, 0o755);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        stderr(&output),
        format!(
            "cmm2-emulator: {}: its directory is not writable, so the run could not save its flash and clock\n",
            flash.display()
        )
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_flash_file_on_a_read_only_file_system_is_an_error_at_the_start() {
    let dir = temp_path("flash-ro-mount", "d");
    std::fs::create_dir(&dir).unwrap();
    let flash = dir.join("board.flash");
    run_program(
        "flash-ro-mount-1",
        &THREE,
        &["--run", "10", "--flash", flash.to_str().unwrap()],
    );
    mode(&flash, 0o664);
    let firmware = temp_program("flash-ro-mount-2", &THREE);

    let output = run_with_read_only_mount(
        &dir,
        &firmware,
        &["--run", "10", "--flash", flash.to_str().unwrap()],
    );

    if let Some(output) = output {
        assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
        assert_eq!(
            stderr(&output),
            format!(
                "cmm2-emulator: {}: its file system is read-only, so the run could not save its flash and clock\n",
                flash.display()
            )
        );
    }
    std::fs::remove_file(&firmware).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_first_run_through_a_dangling_link_makes_the_file_it_points_to() {
    let dir = temp_path("flash-dangling", "d");
    std::fs::create_dir(&dir).unwrap();
    let link = dir.join("link.flash");
    std::os::unix::fs::symlink("real.flash", &link).unwrap();

    let output = run_program(
        "flash-dangling",
        &THREE,
        &["--run", "10", "--flash", link.to_str().unwrap()],
    );

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
    assert_eq!(
        std::fs::read(dir.join("real.flash")).unwrap().len(),
        FLASH_FILE_SIZE
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

// The emulator as a user has it, with no layout named for an image I do not know.
fn without_a_layout(name: &str, args: &[&str]) -> std::process::Output {
    let program = temp_program(name, &THREE);
    let output = emulator()
        .env_remove(LAYOUT_VARIABLE)
        .args(["--firmware", program.to_str().unwrap()])
        .args(args)
        .output()
        .expect("failed to run cmm2-emulator");
    std::fs::remove_file(&program).unwrap();
    output
}

#[test]
fn a_firmware_with_no_settings_layout_is_refused_with_flash_and_the_file_is_left_alone() {
    let flash = flash_path("flash-no-layout");
    let _ = std::fs::remove_file(&flash);
    let path = flash.to_str().unwrap();

    // No file yet, and one that is there.
    for existing in [None, Some(&[1u8, 2, 3][..])] {
        if let Some(bytes) = existing {
            std::fs::write(&flash, bytes).unwrap();
        }
        let output = without_a_layout("flash-no-layout", &["--run", "10", "--flash", path]);

        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert_eq!(
            stderr(&output),
            format!(
                "cmm2-emulator: {path}: this firmware has no settings layout yet, so it cannot \
                 save or load settings (see issue 82)\n"
            )
        );
    }
    assert_eq!(std::fs::read(&flash).unwrap(), [1, 2, 3]);
    std::fs::remove_file(&flash).unwrap();
}

#[test]
fn a_firmware_with_no_settings_layout_still_runs_without_a_flash_file() {
    let output = without_a_layout("scratch-no-layout", &["--run", "10"]);
    assert!(output.status.success(), "{}", stderr(&output));
}

#[test]
fn the_layout_variable_names_a_layout_for_an_unknown_firmware_and_a_bad_name_is_an_error() {
    let flash = flash_path("flash-layout-rc6");
    let _ = std::fs::remove_file(&flash);
    let program = temp_program("flash-layout-rc6", &THREE);
    let run = |layout: &str| {
        emulator()
            .env(LAYOUT_VARIABLE, layout)
            .args(["--firmware", program.to_str().unwrap(), "--run", "10"])
            .args(["--flash", flash.to_str().unwrap()])
            .output()
            .expect("failed to run cmm2-emulator")
    };

    let rc6 = run("rc6");
    assert!(rc6.status.success(), "{}", stderr(&rc6));
    // 32 + 4096 + 655,360, then the clock.
    assert_eq!(std::fs::read(&flash).unwrap().len(), 659_496);
    let bad = run("8");
    assert_eq!(bad.status.code(), Some(1));
    assert_eq!(
        stderr(&bad),
        "cmm2-emulator: CMM2_SETTINGS_LAYOUT is \"8\", not b5 or rc6\n"
    );
    std::fs::remove_file(&program).unwrap();
    std::fs::remove_file(&flash).unwrap();
}

#[test]
fn a_bad_layout_variable_does_not_stop_a_scratch_run_that_has_no_flash_file() {
    let program = temp_program("scratch-bad-layout", &THREE);
    let output = emulator()
        .env(LAYOUT_VARIABLE, "8")
        .args(["--firmware", program.to_str().unwrap(), "--run", "10"])
        .output()
        .expect("failed to run cmm2-emulator");
    std::fs::remove_file(&program).unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
}

// A settings file may be called anything the file system takes, control characters included. The
// refusal that names it is still one line and nothing a terminal acts on: an escape in the name
// must not recolour it, and a newline in it must not break it or forge a line of its own. The size
// refusal is driven here because it says the name three times: the file, the copy of the old
// settings beside it, and the file to delete.
#[test]
fn a_settings_file_named_with_control_characters_is_refused_in_one_unbroken_line() {
    const LARGEST_PLUS_ONE: u64 = 1_146_991;
    let dir = temp_path("flash-escape-names", "dir");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).unwrap();
    let name = "evil\n\x1b[31mRED\x1b[0m.flash";
    let flash = dir.join(name);
    std::fs::File::create(&flash)
        .unwrap()
        .set_len(LARGEST_PLUS_ONE)
        .unwrap();
    std::fs::write(dir.join(format!("{name}.v1")), "the old settings").unwrap();
    let program = temp_program("flash-escape-names", &THREE);

    let output = run(&[
        "--firmware",
        program.to_str().unwrap(),
        "--run",
        "10",
        "--flash",
        flash.to_str().unwrap(),
    ]);
    std::fs::remove_file(&program).unwrap();

    let err = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{err}");
    // One line, ended by one line feed, with no escape in it for a terminal to act on.
    assert_eq!(err.matches('\n').count(), 1, "{err:?}");
    assert_eq!(err.lines().count(), 1, "{err:?}");
    assert!(
        !err.contains('\x1b'),
        "the name recoloured the line: {err:?}"
    );
    assert!(err.starts_with("cmm2-emulator: "), "{err:?}");
    // The clause that offers to delete the file is a command, so its name is quoted: the line
    // ends with the closing quote before the full stop.
    assert!(err.ends_with(".flash'.\n"), "{err:?}");
    // The name is still said, in its visible form, as the file, as its copy, and as what to delete.
    assert_eq!(
        err.matches(r"evil\n\x1B[31mRED\x1B[0m").count(),
        3,
        "the three places the name is said: {err}"
    );
    assert!(err.contains(".v1"), "the copy beside it is listed: {err}");
    assert_eq!(
        std::fs::metadata(&flash).unwrap().len(),
        LARGEST_PLUS_ONE,
        "the refusal wrote nothing"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
