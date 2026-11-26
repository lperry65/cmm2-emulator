//! `--burn`: the firmware burned onto the board once, and what a plain start does after.

mod common;

use common::board::*;
use common::*;
use std::path::PathBuf;

#[test]
fn a_plain_start_with_nothing_burned_says_how_to_burn_and_where_the_board_is() {
    let home = Home::new("board-nothing");
    for args in [&[][..], &["--console", "--run", "10"]] {
        let output = quit_run(&home, args);

        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty());
        assert_eq!(
            stderr(&output),
            format!(
                "cmm2-emulator: no firmware is burned onto the board in {}. Burn it once with: \
                 cmm2-emulator --burn <file>\n",
                home.0.display()
            ),
            "{args:?}"
        );
        assert!(!home.0.exists(), "finding out made the directory");
    }
}

// The board's directory reaches the refusal through the board's own error, which the run hands
// over whole. A directory named with an escape and a line feed is still one line, broken nowhere
// and nothing a terminal acts on, and the name is still there to read.
#[test]
fn a_board_directory_named_with_control_characters_is_refused_in_one_unbroken_line() {
    let home = Home(temp_path("board-escape-names", "dir").join("evil\n\x1b[31mRED\x1b[0m"));

    let output = quit_run(&home, &["--console", "--run", "10"]);
    let err = stderr(&output);

    assert_eq!(output.status.code(), Some(1), "{err}");
    assert!(output.stdout.is_empty());
    assert_eq!(err.matches('\n').count(), 1, "{err:?}");
    assert_eq!(err.lines().count(), 1, "{err:?}");
    assert!(
        !err.contains('\x1b'),
        "the name recoloured the line: {err:?}"
    );
    assert!(err.starts_with("cmm2-emulator: "), "{err:?}");
    assert!(
        err.contains(r"evil\n\x1B[31mRED\x1B[0m. Burn it once with"),
        "the name is not said: {err}"
    );
    assert!(!home.0.exists(), "finding out made the directory");
}

#[test]
fn burning_once_then_a_plain_start_boots_the_board_from_its_own_copy() {
    let home = Home::new("board-burn");
    let program = Tmp::program("board-burn", &count_in_flash(SPIN));
    let first = quit_run(&home, &["--burn", program.arg(), "--console"]);
    assert!(first.status.success(), "{}", stderr(&first));
    assert_eq!(
        stderr(&first),
        format!(
            "{}{QUIT_LINE}",
            burned_line(std::fs::read(&program.0).unwrap().len())
        )
    );
    assert_eq!(counter(&home), 0);
    // The source is gone, and the board does not need it.
    let image = std::fs::read(&program.0).unwrap();
    drop(program);

    let second = quit_run(&home, &["--console"]);

    assert!(second.status.success(), "{}", stderr(&second));
    assert_eq!(stderr(&second), QUIT_LINE);
    assert_eq!(
        counter(&home),
        1,
        "the settings were loaded and saved again"
    );
    assert_eq!(std::fs::read(home.file("firmware.bin")).unwrap(), image);
    let third = quit_run(&home, &["--console"]);
    assert!(third.status.success());
    assert_eq!(counter(&home), 2);
}

#[test]
fn burn_goes_with_the_other_board_options() {
    let home = Home::new("board-burn-run");
    let program = Tmp::program("board-burn-run", &count_in_flash(SPIN));

    let output = quit_run(
        &home,
        &["--burn", program.arg(), "--console", "--run", "100"],
    );

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(counter(&home), 0);
    assert_eq!(
        std::fs::read(home.file("firmware.bin")).unwrap(),
        std::fs::read(&program.0).unwrap()
    );
}

#[test]
fn a_second_burn_replaces_the_first_and_the_settings_are_kept() {
    let home = Home::new("board-reburn");
    burn_counter(&home, "board-reburn-1");
    let echo = Tmp::program("board-reburn-echo", &ECHO);
    let burned = quit_after(&home, &["--burn", echo.arg(), "--console"], b"ready\r\n");
    assert!(burned.status.success(), "{}", stderr(&burned));
    assert_eq!(
        counter(&home),
        0,
        "the settings were not touched by the burn"
    );

    // The board now runs the echo program, whatever it was before.
    let again = quit_after(&home, &["--console"], b"ready\r\n");
    assert!(again.status.success(), "{}", stderr(&again));
    assert_eq!(counter(&home), 0);
    burn_counter(&home, "board-reburn-3");
    assert_eq!(
        counter(&home),
        1,
        "the settings of the first burn are still there"
    );
}

#[test]
fn a_firmware_that_will_not_load_is_refused_and_the_old_one_stays() {
    let home = Home::new("board-bad-firmware");
    burn_counter(&home, "board-bad-firmware");
    let before = std::fs::read(home.file("firmware.bin")).unwrap();
    let bad = Tmp::file("board-bad-firmware", "bin");
    std::fs::write(&bad.0, [1, 2, 3]).unwrap();

    let output = quit_run(&home, &["--burn", bad.arg(), "--console"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(std::fs::read(home.file("firmware.bin")).unwrap(), before);
}

#[test]
fn a_burn_says_what_it_burned() {
    let home = Home::new("board-burn-says");
    let program = Tmp::program("board-burn-says", &THREE);
    let output = quit_run(&home, &["--burn", program.arg(), "--console"]);
    let size = std::fs::read(&program.0).unwrap().len();
    assert!(
        stderr(&output).starts_with(&burned_line(size)),
        "{}",
        stderr(&output)
    );
}

#[test]
fn an_image_that_is_refused_makes_no_directory_and_no_lock_file() {
    let home = Home::new("board-refused");
    let bad = Tmp::file("board-refused", "bin");
    let empty = Tmp::file("board-refused-empty", "bin");
    std::fs::write(&empty.0, []).unwrap();
    std::fs::write(&bad.0, [1, 2, 3]).unwrap();

    for file in [&bad, &empty] {
        let output = quit_run(&home, &["--burn", file.arg(), "--console"]);
        assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
        assert!(!home.0.exists(), "{}", file.arg());
    }
}

// A burn reads the board's own layout variable, and the refusal of a bad value names the variable
// it read. This is the one message that says that name at all; docs/settings-format.md says what
// the variable is for.
#[test]
fn a_burn_with_a_bad_board_layout_name_is_refused_and_names_that_variable() {
    let home = Home::new("board-bad-layout-name");
    let program = Tmp::program("board-bad-layout-name", &THREE);
    let mut command = on(&home);
    let output = command
        .env(BOARD_LAYOUT_VARIABLE, "8")
        .args(["--burn", program.arg(), "--console", "--run", "1"])
        .output()
        .expect("failed to run cmm2-emulator");

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(
        stderr(&output),
        "cmm2-emulator: CMM2_BOARD_LAYOUT_FOR_TESTS is \"8\", not b5 or rc6\n"
    );
    assert!(!home.0.exists(), "a refused burn makes no board directory");
}

// The script of the full-disk tests: burn a small image, make `firmware.bin` as the mode says,
// fill the file system, try to burn a big one, and say what is left.
const FULL_DISK: &str = r#"
M="$1"; E="$2"; SMALL="$3"; BIG="$4"; MODE="$5"
export CMM2_EMULATOR_HOME="$M/board"
python3 -c 'import sys; sys.stdout.write(chr(29))' | "$E" --burn "$SMALL" --console >/dev/null 2>&1 || exit 91
if [ "$MODE" = link ]; then
  cp "$SMALL" "$M/vendor.bin"
  rm "$M/board/firmware.bin"
  ln -s "$M/vendor.bin" "$M/board/firmware.bin"
fi
dd if=/dev/zero of="$M/filler" bs=4k 2>/dev/null
"$E" --burn "$BIG" --console </dev/null >/dev/null
echo "STATUS=$?"
cmp -s "$M/board/firmware.bin" "$SMALL" && echo KEPT
[ -L "$M/board/firmware.bin" ] && echo LINK
[ "$MODE" = link ] && cmp -s "$M/vendor.bin" "$SMALL" && echo VENDOR
ls -A "$M/board"
"#;

#[test]
fn a_burn_that_cannot_finish_on_a_full_disk_leaves_the_old_firmware_in_place() {
    let small = Tmp::program("board-full-small", &THREE);
    let big = Tmp::file("board-full-big", "bin");
    let mut bytes = vec![0u8; 0x1_0000];
    bytes[..4].copy_from_slice(&0x2000_8000u32.to_le_bytes());
    bytes[4..8].copy_from_slice(&0x0800_0101u32.to_le_bytes());
    std::fs::write(&big.0, bytes).unwrap();
    let mount = Tmp::file("board-full-mount", "d");
    std::fs::create_dir(&mount.0).unwrap();
    let program = emulator_path();

    for mode in ["plain", "link"] {
        let args = [mount.arg(), &program, small.arg(), big.arg(), mode];
        let Some(output) = in_tmpfs("3m", FULL_DISK, &args) else {
            break;
        };
        let said = stdout(&output);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{mode}: {said} {}",
            stderr(&output)
        );
        assert!(
            said.contains("STATUS=1"),
            "{mode}: the burn did not fail: {said}"
        );
        assert!(
            said.contains("KEPT"),
            "{mode}: the old firmware is gone: {said}"
        );
        assert_eq!(said.contains("LINK"), mode == "link", "{mode}: {said}");
        assert_eq!(said.contains("VENDOR"), mode == "link", "{mode}: {said}");
        assert!(
            !said.contains(".tmp"),
            "{mode}: a temporary file is left: {said}"
        );
    }
}

// The real firmware: burned once, set up once, and then a plain start comes up at the prompt.
fn real_firmware() -> String {
    std::env::var("CMM2_FIRMWARE")
        .expect("CMM2_FIRMWARE must be set to the path of CMM2_MMBasic_7.00.00b5.bin")
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_first_boot_is_remembered_and_the_next_plain_start_reaches_the_prompt() {
    let home = Home::new("board-real");
    // Burned from a copy that is gone afterwards: the board has its own.
    let copy = Tmp::file("board-real", "bin");
    std::fs::copy(real_firmware(), &copy.0).unwrap();

    // The first run burns and starts the board. The first boot asks its questions (about 450
    // million instructions to answer them all and reach the prompt of the second boot), and the
    // answers are saved in the board.
    let first = run_to_end(
        &home,
        &[
            "--burn",
            copy.arg(),
            "--console",
            "--run",
            "700000000",
            "--on",
            "Keyboard type",
            "1",
            "--on",
            "HH:MM:SS",
            "12:00:00\\r",
            "--on",
            "DD/MM/YY",
            "12/11/26\\r",
        ],
    );
    drop(copy);
    let first_text = stdout(&first);
    assert!(first.status.success(), "{}", stderr(&first));
    assert!(first_text.contains("Keyboard type"), "{first_text}");
    let settings = std::fs::read(home.file("settings.flash")).unwrap();
    // A board of a firmware I know writes the v2 file (docs/settings-format.md): a header,
    // and an entry for every 32-byte row that differs from the burned image. The options
    // sector is at 0x081E0000, past the image's end, so a byte in it is not FF only because
    // the firmware wrote it: the same fact the raw dump showed, now inside an entry.
    let entries = v2_entries(&settings);
    let (at, value) = entries
        .iter()
        .find(|(at, value)| *at <= 0x081E_0000 && 0x081E_0000 < *at + value.len() as u32)
        .expect("no entry covers the options sector");
    assert!(
        value[(0x081E_0000 - at) as usize..]
            .iter()
            .any(|&byte| byte != 0xFF),
        "the options sector is still erased"
    );

    // A plain start: no firmware named, no questions, and the prompt answers.
    let second = run_to_end(
        &home,
        &[
            "--console",
            "--run",
            "700000000",
            "--on",
            "Allardice\r\n\r\n>",
            "PRINT 6*7\\r",
        ],
    );
    let second_text = stdout(&second);
    assert!(second.status.success(), "{}", stderr(&second));
    assert!(!second_text.contains("Keyboard type"), "{second_text}");
    assert!(second_text.contains("PRINT 6*7\r\n 42"), "{second_text}");
}

// The migration of a v1 file (issue 82): the board of a known firmware takes the old file on
// its first start, keeps the old bytes aside, and reads v2 from then on.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn a_v1_settings_file_migrates_on_the_first_board_start_and_keeps_a_backup() {
    let home = Home::new("board-migrate");
    let copy = Tmp::file("board-migrate", "bin");
    std::fs::copy(real_firmware(), &copy.0).unwrap();
    let burn = quit_run(&home, &["--burn", copy.arg(), "--console", "--run", "1000"]);
    assert!(burn.status.success(), "{}", stderr(&burn));
    drop(copy);
    // Seed the v1 file the migration reads: the regions erased and the clock of nothing,
    // the plainest content a v1 file can hold.
    std::fs::remove_file(home.file("settings.flash")).unwrap();
    let mut seed = vec![0xFFu8; FLASH_FILE_SIZE];
    seed[FLASH_FILE_SIZE - 8..].fill(0);
    std::fs::write(home.file("settings.flash"), &seed).unwrap();

    let first = run_to_end(&home, &["--console", "--run", "100000000"]);
    assert!(first.status.success(), "{}", stderr(&first));
    assert_eq!(
        std::fs::read(home.file("settings.flash.v1")).unwrap(),
        seed,
        "the backup holds the old bytes, byte for byte"
    );
    let entries = v2_entries(&std::fs::read(home.file("settings.flash")).unwrap());

    let second = run_to_end(&home, &["--console", "--run", "100000000"]);
    assert!(second.status.success(), "{}", stderr(&second));
    assert_eq!(
        v2_entries(&std::fs::read(home.file("settings.flash")).unwrap()),
        entries,
        "run 2 read and saved the same entries; only the clock tail may move"
    );
    assert_eq!(
        std::fs::read(home.file("settings.flash.v1")).unwrap(),
        seed,
        "the migration of run 2 did not overwrite the backup with its own file"
    );
}

// m2: the board's file is v2 after the first save and still v2 after the second.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_b5_settings_are_the_v2_format_after_two_saves() {
    let home = Home::new("board-v2-b5");
    let copy = Tmp::file("board-v2-b5", "bin");
    std::fs::copy(real_firmware(), &copy.0).unwrap();
    let first = run_to_end(
        &home,
        &[
            "--burn",
            copy.arg(),
            "--console",
            "--run",
            "700000000",
            "--on",
            "Keyboard type",
            "1",
            "--on",
            "HH:MM:SS",
            "12:00:00\\r",
            "--on",
            "DD/MM/YY",
            "12/11/26\\r",
        ],
    );
    assert!(first.status.success(), "{}", stderr(&first));
    assert!(stdout(&first).contains("Keyboard type"), "no first boot");
    // The magic and version 2: v2_entries asserts both, or panics.
    v2_entries(&std::fs::read(home.file("settings.flash")).unwrap());
    drop(copy);

    let second = run_to_end(
        &home,
        &[
            "--console",
            "--run",
            "700000000",
            "--on",
            "Allardice\r\n\r\n>",
            "PRINT 6*7\\r",
        ],
    );
    assert!(second.status.success(), "{}", stderr(&second));
    assert!(
        stdout(&second).contains("PRINT 6*7\r\n 42"),
        "the second start did not reach the prompt"
    );
    v2_entries(&std::fs::read(home.file("settings.flash")).unwrap());
}

#[test]
#[ignore = "needs the RC6 firmware: CMM2_FIRMWARE_RC6=<path> cargo test --release -- --ignored"]
fn real_rc6_settings_are_the_v2_format_after_two_saves() {
    let home = Home::new("board-v2-rc6");
    let copy = Tmp::file("board-v2-rc6", "bin");
    std::fs::copy(rc6_firmware(), &copy.0).unwrap();
    let first = run_to_end(
        &home,
        &[
            "--burn",
            copy.arg(),
            "--console",
            "--run",
            "700000000",
            "--on",
            "Keyboard type",
            "1",
            "--on",
            "HH:MM:SS",
            "12:00:00\\r",
            "--on",
            "DD/MM/YY",
            "12/11/26\\r",
        ],
    );
    assert!(first.status.success(), "{}", stderr(&first));
    v2_entries(&std::fs::read(home.file("settings.flash")).unwrap());
    drop(copy);

    let second = run_to_end(
        &home,
        &[
            "--console",
            "--run",
            "700000000",
            "--on",
            "Allardice\r\n\r\n>",
            "PRINT 6*7\\r",
        ],
    );
    assert!(second.status.success(), "{}", stderr(&second));
    v2_entries(&std::fs::read(home.file("settings.flash")).unwrap());
}

// The hash gate: burn one release over the settings of another, and the board names both
// firmwares by hash and leaves the file alone.
#[test]
#[ignore = "needs both firmwares: CMM2_FIRMWARE=<path> CMM2_FIRMWARE_RC6=<path> cargo test \
            --release -- --ignored"]
fn a_board_burned_with_another_release_names_both_hashes_and_refuses_the_settings() {
    let home = Home::new("board-mismatch");
    let b5 = Tmp::file("board-mismatch-b5", "bin");
    std::fs::copy(real_firmware(), &b5.0).unwrap();
    let burn = quit_run(
        &home,
        &["--burn", b5.arg(), "--console", "--run", "5000000"],
    );
    assert!(burn.status.success(), "{}", stderr(&burn));
    drop(b5);
    let saved = std::fs::read(home.file("settings.flash")).unwrap();
    v2_entries(&saved);

    let rc6 = Tmp::file("board-mismatch-rc6", "bin");
    std::fs::copy(rc6_firmware(), &rc6.0).unwrap();
    let mut command = on(&home);
    let output = command
        .args(["--burn", rc6.arg(), "--console", "--run", "10"])
        .output()
        .expect("failed to run cmm2-emulator");
    drop(rc6);

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let said = stderr(&output);
    assert!(said.contains("saved for a different firmware"), "{said}");
    assert!(said.contains("7.00.00b5"), "{said}");
    assert!(said.contains("7.00.00RC6"), "{said}");
    // The two hashes the settings name: the local images' own first 12 hex.
    assert!(said.contains("sha256 6dbbdd03d2dd"), "{said}");
    assert!(said.contains("sha256 f4aa9c6038b4"), "{said}");
    assert!(
        said.contains("Burn the firmware they were made for"),
        "{said}"
    );
    assert_eq!(
        std::fs::read(home.file("settings.flash")).unwrap(),
        saved,
        "the refusal left the settings file byte-identical"
    );
}

#[test]
fn burning_a_firmware_with_no_settings_layout_is_refused_before_the_board_is_touched() {
    let home = Home::new("board-no-layout");
    let program = Tmp::program("board-no-layout", &count_in_flash(SPIN));
    let mut command = on(&home);
    let output = command
        .env_remove(LAYOUT_VARIABLE)
        .env_remove(BOARD_LAYOUT_VARIABLE)
        .args(["--burn", program.arg(), "--console", "--run", "10"])
        .output()
        .expect("failed to run cmm2-emulator");

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!(
            "cmm2-emulator: {}: this firmware has no settings layout yet, so it cannot save or \
             load settings (see issue 82)\n",
            program.arg()
        )
    );
    assert!(!home.0.exists(), "the refusal made the directory");
}

#[test]
fn the_settings_layout_variable_never_lets_a_burn_of_an_unknown_firmware_through() {
    let home = Home::new("board-burn-variable");
    let program = Tmp::program("board-burn-variable", &count_in_flash(SPIN));
    let mut command = on(&home);
    let output = command
        .env(LAYOUT_VARIABLE, "b5")
        .env_remove(BOARD_LAYOUT_VARIABLE)
        .args(["--burn", program.arg(), "--console", "--run", "10"])
        .output()
        .expect("failed to run cmm2-emulator");

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("no settings layout"),
        "{}",
        stderr(&output)
    );
    assert!(!home.0.exists(), "the refusal made the directory");
}

#[test]
fn a_plain_start_with_an_unknown_firmware_burned_names_the_firmware_and_how_to_go_on() {
    let home = Home::new("board-unknown-start");
    let program = Tmp::program("board-unknown-start", &count_in_flash(SPIN));
    // Burned with the layout that the tests name for the board, as a firmware I know would be.
    let first = quit_run(&home, &["--burn", program.arg(), "--console"]);
    assert!(first.status.success(), "{}", stderr(&first));

    // Plain, and with the scratch variable set, which the board never looks at.
    for scratch_layout in [None, Some("b5")] {
        let mut command = on(&home);
        command
            .env_remove(LAYOUT_VARIABLE)
            .env_remove(BOARD_LAYOUT_VARIABLE)
            .args(["--console", "--run", "10"]);
        if let Some(layout) = scratch_layout {
            command.env(LAYOUT_VARIABLE, layout);
        }
        let output = command.output().expect("failed to run cmm2-emulator");

        assert_eq!(output.status.code(), Some(1), "{scratch_layout:?}");
        assert_eq!(
            stderr(&output),
            format!(
                "cmm2-emulator: {}: the firmware burned onto the board has no settings layout \
                 yet (see issue 82), so the board cannot keep its settings. Burn a firmware I \
                 know with: cmm2-emulator --burn <file>\n",
                home.file("firmware.bin").display()
            ),
            "{scratch_layout:?}"
        );
    }
}

// Removes a directory tree that a test made, giving its directories back their rights first, so
// that a failed assertion leaves nothing behind.
struct Unlock(PathBuf);

impl Drop for Unlock {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        for dir in ["L", "T"] {
            let _ =
                std::fs::set_permissions(self.0.join(dir), std::fs::Permissions::from_mode(0o755));
        }
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// The v2 save replaces the name it is given and does not follow a link there, so a settings file
// that is a link is judged by the directory the link is in, and a run that could not save is
// refused at the start. Two cases: a `--flash` link in a directory that cannot be written, and
// the board's own `settings.flash` link in a board directory that cannot be written.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn a_symlinked_settings_file_in_a_directory_that_cannot_be_written_is_refused_at_the_start() {
    use std::os::unix::fs::PermissionsExt;
    if root() {
        eprintln!("skipped: root can write in any directory, so none is refused");
        return;
    }
    let home = Home::new("board-ro-link");
    let copy = Tmp::file("board-ro-link", "bin");
    std::fs::copy(real_firmware(), &copy.0).unwrap();
    let burn = quit_run(&home, &["--burn", copy.arg(), "--console", "--run", "1000"]);
    assert!(burn.status.success(), "{}", stderr(&burn));
    drop(copy);
    let tree = Unlock(temp_path("board-ro-link", "tree"));
    let (links, targets) = (tree.0.join("L"), tree.0.join("T"));
    std::fs::create_dir_all(&links).unwrap();
    std::fs::create_dir_all(&targets).unwrap();
    let v2 = std::fs::read(home.file("settings.flash")).unwrap();
    std::fs::write(targets.join("t.flash"), &v2).unwrap();

    // `--flash` a link in a directory that cannot be written.
    let link = links.join("link.flash");
    std::os::unix::fs::symlink("../T/t.flash", &link).unwrap();
    std::fs::set_permissions(&links, std::fs::Permissions::from_mode(0o555)).unwrap();
    let mut command = on(&home);
    let output = command
        .args([
            "--flash",
            link.to_str().unwrap(),
            "--console",
            "--run",
            "1000",
            "--no-sd",
        ])
        .output()
        .expect("failed to run cmm2-emulator");
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(
        stderr(&output),
        format!(
            "cmm2-emulator: {}: its directory is not writable, so the run could not save its \
             flash and clock\n",
            link.display()
        )
    );
    assert_eq!(
        std::fs::read(targets.join("t.flash")).unwrap(),
        v2,
        "nothing was written"
    );

    // The board's own file, a link, in a board directory that cannot be written.
    let own = home.file("settings.flash");
    std::fs::remove_file(&own).unwrap();
    std::os::unix::fs::symlink(targets.join("t.flash"), &own).unwrap();
    std::fs::set_permissions(&home.0, std::fs::Permissions::from_mode(0o555)).unwrap();
    let mut command = on(&home);
    let output = command
        .args(["--console", "--run", "1000", "--no-sd"])
        .output()
        .expect("failed to run cmm2-emulator");
    std::fs::set_permissions(&home.0, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(
        stderr(&output),
        format!(
            "cmm2-emulator: {}: its directory is not writable, so the run could not save its \
             flash and clock\n",
            own.display()
        )
    );
    assert_eq!(
        std::fs::read(targets.join("t.flash")).unwrap(),
        v2,
        "nothing was written"
    );
}
