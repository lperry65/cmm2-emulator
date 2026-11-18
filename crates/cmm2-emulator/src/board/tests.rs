use super::*;
use crate::test_support::{files_in, scratch_dir, spawning};
use std::collections::HashMap;

fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
    let vars: HashMap<&str, &str> = pairs.iter().copied().collect();
    move |name| vars.get(name).map(OsString::from)
}

// A firmware image the loader accepts, with `mark` in its last byte to tell two of them apart.
fn image(mark: u8) -> Vec<u8> {
    let mut bytes = vec![0; 0x200];
    bytes[..4].copy_from_slice(&0x2000_8000u32.to_le_bytes());
    bytes[4..8].copy_from_slice(&0x0800_0101u32.to_le_bytes());
    bytes[0x1FF] = mark;
    bytes
}

fn firmware_file(dir: &Path, mark: u8) -> String {
    let path = dir.join(format!("source-{mark}.bin"));
    std::fs::write(&path, image(mark)).unwrap();
    path.display().to_string()
}

fn burned(dir: &Path) -> Board {
    let board = Board::switch_on(dir.join("board"), true).unwrap();
    burn(&board, &firmware_file(dir, 1)).unwrap();
    board
}

// Looks at the image and burns it, as a burning run does.
fn burn(board: &Board, source: &str) -> Result<(), RunError> {
    board.burn(&Firmware::load(source)?)
}

fn text(err: impl fmt::Display) -> String {
    err.to_string()
}

#[test]
fn the_environment_variable_names_the_directory_itself() {
    let dir = board_dir(env(&[
        ("CMM2_EMULATOR_HOME", "/tmp/mine"),
        ("XDG_DATA_HOME", "/x"),
        ("HOME", "/h"),
    ]));
    assert_eq!(dir.unwrap(), Path::new("/tmp/mine"));
}

#[test]
fn a_relative_directory_is_made_absolute() {
    let dir = board_dir(env(&[("CMM2_EMULATOR_HOME", "here")])).unwrap();
    assert!(dir.is_absolute());
    assert!(dir.ends_with("here"));
}

#[test]
fn xdg_data_home_is_honoured_when_it_is_absolute() {
    let dir = board_dir(env(&[("XDG_DATA_HOME", "/data"), ("HOME", "/h")]));
    assert_eq!(dir.unwrap(), Path::new("/data/cmm2-emulator"));
}

#[test]
fn a_relative_or_empty_xdg_data_home_is_ignored() {
    for xdg in ["data", ""] {
        let dir = board_dir(env(&[("XDG_DATA_HOME", xdg), ("HOME", "/h")]));
        assert_eq!(
            dir.unwrap(),
            Path::new("/h/.local/share/cmm2-emulator"),
            "{xdg:?}"
        );
    }
}

#[test]
fn an_empty_home_variable_is_an_error_and_does_not_fall_back_to_the_usual_directory() {
    let said = text(board_dir(env(&[("CMM2_EMULATOR_HOME", ""), ("HOME", "/h")])).unwrap_err());
    assert!(
        said.contains("CMM2_EMULATOR_HOME") && said.contains("empty"),
        "{said}"
    );
}

#[test]
fn with_nowhere_to_look_it_says_what_to_set() {
    let err = board_dir(env(&[])).unwrap_err();
    assert!(text(&err).contains("Set CMM2_EMULATOR_HOME"), "{err}");
}

#[test]
fn nothing_burned_says_how_to_burn_and_where_the_board_is() {
    let scratch = scratch_dir("board-nothing");
    for dir in [scratch.join("none"), scratch.to_path_buf()] {
        let err = Board::switch_on(dir.clone(), false).err().unwrap();
        let said = text(&err);
        assert!(said.contains("cmm2-emulator --burn <file>"), "{said}");
        assert!(said.contains(&dir.display().to_string()), "{said}");
    }
    // Finding out does not make the directory.
    assert!(!scratch.join("none").exists());
}

#[test]
fn burning_copies_the_image_into_the_board() {
    let scratch = scratch_dir("board-flash");
    let board = burned(&scratch);
    assert_eq!(std::fs::read(board.firmware_path()).unwrap(), image(1));
}

#[test]
fn the_board_keeps_its_copy_when_the_source_changes() {
    let scratch = scratch_dir("board-copy");
    let source = firmware_file(&scratch, 1);
    let board = Board::switch_on(scratch.join("board"), true).unwrap();
    burn(&board, &source).unwrap();
    std::fs::write(&source, image(9)).unwrap();
    assert_eq!(std::fs::read(board.firmware_path()).unwrap(), image(1));
}

#[test]
fn a_second_firmware_replaces_the_first() {
    let scratch = scratch_dir("board-replace");
    let board = burned(&scratch);
    burn(&board, &firmware_file(&scratch, 2)).unwrap();
    assert_eq!(std::fs::read(board.firmware_path()).unwrap(), image(2));
}

#[test]
fn burning_leaves_no_temporary_file() {
    let scratch = scratch_dir("board-temp");
    let board = burned(&scratch);
    drop(board);
    let mut names = files_in(&scratch.join("board"));
    names.sort();
    assert_eq!(names, ["board.lock", "firmware.bin"]);
}

#[test]
fn an_image_that_reaches_the_user_flash_is_refused_and_the_old_firmware_stays() {
    let scratch = scratch_dir("board-large");
    let board = burned(&scratch);
    let large = scratch.join("large.bin");
    let mut bytes = image(3);
    bytes.resize(0x12_0000 + 1, 0);
    std::fs::write(&large, bytes).unwrap();

    let err = burn(&board, large.to_str().unwrap()).unwrap_err();

    assert!(text(&err).contains("0x08120000"), "{err}");
    assert_eq!(std::fs::read(board.firmware_path()).unwrap(), image(1));
}

#[test]
fn an_image_that_is_not_firmware_is_refused_and_the_old_firmware_stays() {
    let scratch = scratch_dir("board-bad");
    let board = burned(&scratch);
    let bad = scratch.join("bad.bin");
    std::fs::write(&bad, [1, 2, 3]).unwrap();
    assert!(burn(&board, bad.to_str().unwrap()).is_err());
    assert!(burn(&board, "/nonexistent/f.bin").is_err());
    assert_eq!(std::fs::read(board.firmware_path()).unwrap(), image(1));
}

#[test]
fn the_settings_are_kept_across_a_re_burn() {
    let scratch = scratch_dir("board-settings");
    let board = burned(&scratch);
    std::fs::write(board.settings_path(), b"my settings").unwrap();
    burn(&board, &firmware_file(&scratch, 2)).unwrap();
    assert_eq!(
        std::fs::read(board.settings_path()).unwrap(),
        b"my settings"
    );
    assert!(board.settings_path().ends_with("board/settings.flash"));
}

#[test]
fn a_burned_board_switches_on_again_without_the_source() {
    // The lock is let go of and taken again here, so no process may be started meanwhile.
    let _alone = spawning();
    let scratch = scratch_dir("board-again");
    drop(burned(&scratch));
    let again = Board::switch_on(scratch.join("board"), false).unwrap();
    assert_eq!(std::fs::read(again.firmware_path()).unwrap(), image(1));
}

#[test]
fn a_board_that_is_on_cannot_be_switched_on_again() {
    // The lock is let go of and taken again here, so no process may be started meanwhile.
    let _alone = spawning();
    let scratch = scratch_dir("board-lock");
    let board = burned(&scratch);
    for burning in [false, true] {
        let err = Board::switch_on(scratch.join("board"), burning)
            .err()
            .unwrap();
        let said = text(&err);
        assert!(said.contains("already switched on"), "{said}");
        assert!(
            said.contains(&scratch.join("board").display().to_string()),
            "{said}"
        );
    }
    drop(board);
    assert!(Board::switch_on(scratch.join("board"), false).is_ok());
}

#[test]
fn a_stale_temporary_file_of_a_cut_short_burn_is_cleared_on_switch_on() {
    // The lock is let go of and taken again here, so no process may be started meanwhile.
    let _alone = spawning();
    let scratch = scratch_dir("board-stale");
    drop(burned(&scratch));
    let stale = scratch.join("board/.cmm2.1.2.tmp");
    std::fs::write(&stale, b"x").unwrap();
    crate::test_support::age(&stale, 3600);
    let _board = Board::switch_on(scratch.join("board"), false).unwrap();
    assert!(!stale.exists());
}

#[test]
fn no_card_is_remembered_at_first() {
    let scratch = scratch_dir("board-no-card");
    let board = burned(&scratch);
    assert_eq!(board.remembered_card().unwrap(), None);
}

#[test]
fn the_card_is_remembered_by_its_absolute_path() {
    let scratch = scratch_dir("board-card");
    let board = burned(&scratch);
    board.remember_card("/cards/one.img").unwrap();
    assert_eq!(
        board.remembered_card().unwrap().as_deref(),
        Some("/cards/one.img")
    );
    board.remember_card("relative.img").unwrap();
    let kept = board.remembered_card().unwrap().unwrap();
    assert!(
        Path::new(&kept).is_absolute() && kept.ends_with("/relative.img"),
        "{kept}"
    );
}

#[test]
fn a_new_card_replaces_the_remembered_one() {
    let scratch = scratch_dir("board-card-replace");
    let board = burned(&scratch);
    board.remember_card("/cards/one.img").unwrap();
    board.remember_card("/cards/two.img").unwrap();
    assert_eq!(
        board.remembered_card().unwrap().as_deref(),
        Some("/cards/two.img")
    );
}

#[test]
fn the_card_is_remembered_by_the_next_switch_on() {
    // The lock is let go of and taken again here, so no process may be started meanwhile.
    let _alone = spawning();
    let scratch = scratch_dir("board-card-next");
    let board = burned(&scratch);
    board.remember_card("/cards/one.img").unwrap();
    drop(board);
    let board = Board::switch_on(scratch.join("board"), false).unwrap();
    assert_eq!(
        board.remembered_card().unwrap().as_deref(),
        Some("/cards/one.img")
    );
}

#[test]
fn taking_the_card_out_forgets_it_and_nothing_to_take_out_is_fine() {
    let scratch = scratch_dir("board-card-out");
    let board = burned(&scratch);
    board.forget_card().unwrap();
    board.remember_card("/cards/one.img").unwrap();
    board.forget_card().unwrap();
    assert_eq!(board.remembered_card().unwrap(), None);
}

#[test]
fn a_record_that_is_not_a_path_is_refused_with_what_to_do() {
    let scratch = scratch_dir("board-card-bad");
    let board = burned(&scratch);
    for bad in ["", "relative.img"] {
        std::fs::write(scratch.join("board/sd.path"), bad).unwrap();
        let said = text(board.remembered_card().unwrap_err());
        assert!(
            said.contains("--sd <file>") && said.contains("--no-sd"),
            "{said}"
        );
    }
}

#[test]
fn a_card_that_has_gone_is_told_how_to_take_it_out_or_give_another() {
    let gone = RunError::Sd {
        path: "/cards/gone.img".into(),
        source: io::Error::from(io::ErrorKind::NotFound),
    };
    let said = text(about_the_remembered_card(gone));
    assert!(said.contains("/cards/gone.img"), "{said}");
    assert!(said.contains("Give another with --sd <file>"), "{said}");
    assert!(said.contains("take it out with --no-sd"), "{said}");
}

#[test]
fn an_error_that_is_not_about_the_card_is_left_as_it_is() {
    let err = about_the_remembered_card(RunError::Fault("boom".into()));
    assert_eq!(text(err), "boom");
}

#[test]
fn a_hand_written_record_may_end_in_a_line_end() {
    let scratch = scratch_dir("board-card-newline");
    let board = burned(&scratch);
    for (text, want) in [
        ("/cards/one.img\n", "/cards/one.img"),
        ("/cards/one.img\r\n", "/cards/one.img"),
        ("/cards/one.img", "/cards/one.img"),
    ] {
        std::fs::write(scratch.join("board/sd.path"), text).unwrap();
        assert_eq!(
            board.remembered_card().unwrap().as_deref(),
            Some(want),
            "{text:?}"
        );
    }
}

#[test]
fn a_record_that_is_not_utf8_gets_the_advice_too() {
    let scratch = scratch_dir("board-card-bytes");
    let board = burned(&scratch);
    std::fs::write(scratch.join("board/sd.path"), [b'/', 0xFF, 0xFE]).unwrap();
    let said = text(board.remembered_card().unwrap_err());
    assert!(
        said.contains("--sd <file>") && said.contains("--no-sd"),
        "{said}"
    );
    assert!(!said.contains("stream did not contain"), "{said}");
}

#[test]
fn a_link_in_the_place_of_the_firmware_is_replaced_and_what_it_points_to_is_not_written() {
    let scratch = scratch_dir("board-link");
    let board = burned(&scratch);
    let vendor = scratch.join("vendor.bin");
    std::fs::write(&vendor, b"vendor image").unwrap();
    std::fs::remove_file(board.firmware_path()).unwrap();
    std::os::unix::fs::symlink(&vendor, board.firmware_path()).unwrap();

    burn(&board, &firmware_file(&scratch, 2)).unwrap();

    assert_eq!(std::fs::read(&vendor).unwrap(), b"vendor image");
    assert!(
        !std::fs::symlink_metadata(board.firmware_path())
            .unwrap()
            .is_symlink()
    );
    assert_eq!(std::fs::read(board.firmware_path()).unwrap(), image(2));
}

#[test]
fn a_link_in_the_place_of_the_card_record_is_replaced_too() {
    let scratch = scratch_dir("board-link-card");
    let board = burned(&scratch);
    let other = scratch.join("other");
    std::fs::write(&other, b"keep").unwrap();
    std::os::unix::fs::symlink(&other, scratch.join("board/sd.path")).unwrap();
    board.remember_card("/cards/one.img").unwrap();
    assert_eq!(std::fs::read(&other).unwrap(), b"keep");
    assert_eq!(
        board.remembered_card().unwrap().as_deref(),
        Some("/cards/one.img")
    );
}

#[test]
fn a_directory_whose_name_is_not_utf8_is_refused_up_front() {
    use std::os::unix::ffi::OsStringExt;
    let name = OsString::from_vec(b"/tmp/\xFFboard".to_vec());
    let vars = move |var: &str| (var == "CMM2_EMULATOR_HOME").then(|| name.clone());
    let said = text(board_dir(vars).unwrap_err());
    assert!(
        said.contains("not valid UTF-8") && said.contains("CMM2_EMULATOR_HOME"),
        "{said}"
    );
}

#[test]
fn the_home_directory_is_joined_by_components() {
    let dir = board_dir(env(&[("USERPROFILE", "C:\\Users\\lee")])).unwrap();
    let parts: Vec<_> = dir.components().map(|c| c.as_os_str().to_owned()).collect();
    let n = parts.len();
    assert_eq!(&parts[n - 3..], [".local", "share", "cmm2-emulator"]);
}

#[test]
fn the_no_home_message_names_every_variable_that_was_looked_at() {
    let said = text(board_dir(env(&[("XDG_DATA_HOME", "relative")])).unwrap_err());
    for name in [
        "CMM2_EMULATOR_HOME",
        "XDG_DATA_HOME",
        "absolute",
        "HOME",
        "USERPROFILE",
    ] {
        assert!(said.contains(name), "{name}: {said}");
    }
}

#[test]
fn the_directory_is_private_to_the_user_and_so_are_the_parents_made_with_it() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = scratch_dir("board-mode");
    let dir = scratch.join("share/cmm2-emulator");
    drop(Board::switch_on(dir.clone(), true).unwrap());
    for made in [dir.clone(), dir.parent().unwrap().to_path_buf()] {
        let mode = std::fs::metadata(&made).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{}", made.display());
    }
}

#[test]
fn a_directory_that_cannot_be_written_still_switches_on_with_its_lock_file_there() {
    if crate::test_support::root() {
        return;
    }
    let _alone = spawning();
    let scratch = scratch_dir("board-read-only");
    drop(burned(&scratch));
    // The lock file is read-only too (a board that was copied, or made read-only on purpose).
    crate::test_support::mode(&scratch.join("board/board.lock"), 0o444);
    crate::test_support::mode(&scratch.join("board"), 0o555);
    let board = Board::switch_on(scratch.join("board"), false);
    crate::test_support::mode(&scratch.join("board"), 0o755);
    crate::test_support::mode(&scratch.join("board/board.lock"), 0o644);
    assert!(board.is_ok(), "{:?}", board.err());
}

#[test]
fn a_record_may_end_in_several_line_ends_but_not_in_blanks_that_are_the_names() {
    let scratch = scratch_dir("board-card-blank");
    let board = burned(&scratch);
    std::fs::write(scratch.join("board/sd.path"), "/cards/one.img\n\n\r\n").unwrap();
    assert_eq!(
        board.remembered_card().unwrap().as_deref(),
        Some("/cards/one.img")
    );
    // A name that ends in a blank is remembered as it is, and read as it is.
    board.remember_card("/cards/one card ").unwrap();
    assert_eq!(
        board.remembered_card().unwrap().as_deref(),
        Some("/cards/one card ")
    );
}

#[test]
fn a_card_name_that_ends_in_a_line_end_is_refused_when_it_is_given() {
    let scratch = scratch_dir("board-card-eol");
    let board = burned(&scratch);
    for name in ["/cards/one\n", "/cards/one\r"] {
        let said = text(board.remember_card(name).unwrap_err());
        assert!(said.contains("cannot end in a line end"), "{said}");
    }
    assert_eq!(board.remembered_card().unwrap(), None);
}

#[test]
fn a_record_that_is_a_directory_gets_the_advice_and_can_be_replaced_or_removed() {
    let scratch = scratch_dir("board-card-dir");
    let board = burned(&scratch);
    let record = scratch.join("board/sd.path");
    std::fs::create_dir(&record).unwrap();
    let said = text(board.remembered_card().unwrap_err());
    assert!(
        said.contains("--sd <file>") && said.contains("--no-sd"),
        "{said}"
    );

    board.remember_card("/cards/one.img").unwrap();
    assert_eq!(
        board.remembered_card().unwrap().as_deref(),
        Some("/cards/one.img")
    );
    std::fs::remove_file(&record).unwrap();
    std::fs::create_dir(&record).unwrap();
    board.forget_card().unwrap();
    assert!(!record.exists());
}

#[test]
fn a_record_that_is_a_directory_with_something_in_it_is_not_deleted() {
    let scratch = scratch_dir("board-card-full-dir");
    let board = burned(&scratch);
    let record = scratch.join("board/sd.path");
    std::fs::create_dir(&record).unwrap();
    std::fs::write(record.join("mine.txt"), b"keep").unwrap();
    assert!(board.forget_card().is_err());
    assert!(board.remember_card("/cards/one.img").is_err());
    assert_eq!(std::fs::read(record.join("mine.txt")).unwrap(), b"keep");
}

#[test]
fn a_link_to_a_directory_or_a_file_that_cannot_be_read_is_replaced_by_a_burn() {
    let scratch = scratch_dir("board-link-odd");
    let board = burned(&scratch);
    let unreadable = scratch.join("secret.bin");
    std::fs::write(&unreadable, b"secret").unwrap();
    crate::test_support::mode(&unreadable, 0o000);
    for target in [
        scratch.to_path_buf(),
        unreadable.clone(),
        scratch.join("dangling"),
    ] {
        std::fs::remove_file(board.firmware_path()).unwrap();
        std::os::unix::fs::symlink(&target, board.firmware_path()).unwrap();
        burn(&board, &firmware_file(&scratch, 2)).unwrap();
        assert_eq!(std::fs::read(board.firmware_path()).unwrap(), image(2));
    }
    crate::test_support::mode(&unreadable, 0o600);
}

#[test]
fn the_lock_file_is_opened_to_write_when_it_can_be_and_to_read_when_that_is_refused() {
    if crate::test_support::root() {
        return;
    }
    let scratch = scratch_dir("board-open-lock");
    let lock = scratch.join("board.lock");
    // Not there: made, and opened to write.
    assert!(!open_lock(&lock).unwrap().1);
    assert!(lock.exists());
    // There and writable: to write (what NFS needs for an exclusive lock).
    assert!(!open_lock(&lock).unwrap().1);
    // There and not writable: to read.
    crate::test_support::mode(&lock, 0o444);
    assert!(open_lock(&lock).unwrap().1);
    // Not there, in a directory that cannot be written: the error stands.
    let none = scratch.join("none");
    std::fs::create_dir(&none).unwrap();
    crate::test_support::mode(&none, 0o555);
    let err = open_lock(&none.join("board.lock")).unwrap_err();
    crate::test_support::mode(&none, 0o755);
    assert_eq!(err.raw_os_error(), Some(libc::EACCES));
}

#[test]
fn a_lock_file_that_cannot_be_written_is_opened_to_read_for_the_errors_that_say_so() {
    for (errno, falls_back) in [
        (libc::EACCES, true),
        (libc::EPERM, true),
        (libc::EROFS, true),
        (libc::ENOENT, false),
        (libc::ENOSPC, false),
        (libc::EISDIR, false),
    ] {
        let err = io::Error::from_raw_os_error(errno);
        assert_eq!(opens_to_read_instead(&err), falls_back, "{err}");
    }
}

#[test]
fn the_no_lock_message_says_which_way_the_lock_file_was_opened() {
    for (read_only, way) in [(true, "to read"), (false, "to write")] {
        let said = text(BoardError::NoLock {
            path: "/b/board.lock".into(),
            source: io::Error::from_raw_os_error(libc::EBADF),
            read_only,
        });
        assert!(said.contains(&format!("opened {way}")), "{said}");
    }
}

#[test]
fn the_cleanup_of_stale_temporary_files_stays_in_the_board_when_firmware_is_a_link() {
    let _alone = spawning();
    let scratch = scratch_dir("board-stale-link");
    drop(burned(&scratch));
    let vendor = scratch.join("vendor");
    std::fs::create_dir(&vendor).unwrap();
    std::fs::write(vendor.join("fw.bin"), image(1)).unwrap();
    std::fs::remove_file(scratch.join("board/firmware.bin")).unwrap();
    std::os::unix::fs::symlink(vendor.join("fw.bin"), scratch.join("board/firmware.bin")).unwrap();
    let outside = vendor.join(".cmm2.1.2.tmp");
    let inside = scratch.join("board/.cmm2.3.4.tmp");
    for stale in [&outside, &inside] {
        std::fs::write(stale, b"x").unwrap();
        crate::test_support::age(stale, 3600);
    }

    let _board = Board::switch_on(scratch.join("board"), false).unwrap();

    assert!(outside.exists(), "a file outside the board was removed");
    assert!(!inside.exists(), "the board's own stale file was left");
}

#[test]
fn a_plain_start_on_a_directory_with_no_firmware_makes_nothing_in_it() {
    let scratch = scratch_dir("board-nothing-in-it");
    let err = Board::switch_on(scratch.to_path_buf(), false)
        .err()
        .unwrap();
    assert!(text(err).contains("--burn"));
    assert!(files_in(&scratch).is_empty());
}

#[test]
fn a_link_put_in_the_board_directory_does_not_lead_the_cleanup_out_of_it() {
    let _alone = spawning();
    let scratch = scratch_dir("board-stale-clean-link");
    drop(burned(&scratch));
    let elsewhere = scratch.join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let outside = elsewhere.join(".cmm2.1.2.tmp");
    std::fs::write(&outside, b"x").unwrap();
    crate::test_support::age(&outside, 3600);
    // Where a name for the cleanup to resolve used to be.
    std::os::unix::fs::symlink(elsewhere.join("x"), scratch.join("board/.cmm2.clean")).unwrap();

    let _board = Board::switch_on(scratch.join("board"), false).unwrap();

    assert!(
        outside.exists(),
        "the cleanup followed a link out of the board"
    );
}
