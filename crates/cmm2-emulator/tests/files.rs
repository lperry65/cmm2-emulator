//! What --flash and --sd may name, and the temporary files a save leaves.

mod common;

use common::*;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

#[test]
fn what_is_not_a_regular_file_is_refused_for_both_files_and_is_never_opened() {
    let dir = temp_path("not-regular", "d");
    std::fs::create_dir(&dir).unwrap();
    let fifo = dir.join("pipe");
    make_fifo(&fifo);

    // The FIFO would hang the read for ever, so a refusal that returns is the test.
    for path in [fifo.as_path(), dir.as_path(), Path::new("/dev/null")] {
        for option in ["--sd", "--flash"] {
            let output = run_program(
                "not-regular",
                &THREE,
                &["--run", "10", option, path.to_str().unwrap()],
            );
            assert_eq!(output.status.code(), Some(1), "{option} {}", path.display());
            assert!(output.stdout.is_empty());
            assert_eq!(
                stderr(&output),
                format!("cmm2-emulator: {}: not a regular file\n", path.display()),
                "{option}"
            );
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_name_that_ends_in_a_slash_or_dots_is_refused_at_the_start_for_both_files() {
    let dir = temp_path("slash", "d");
    std::fs::create_dir(&dir).unwrap();
    for tail in ["flash/", "flash/.", "flash/..", "."] {
        for option in ["--flash", "--sd"] {
            let path = format!("{}/{tail}", dir.display());
            let output = run_program("slash", &THREE, &["--run", "10", option, &path]);
            assert_eq!(output.status.code(), Some(1), "{option} {path}");
            assert_eq!(
                stderr(&output),
                format!(
                    "cmm2-emulator: {path}: not a file name (it is empty, or ends in a slash, or in . or ..)\n"
                ),
                "{option}"
            );
        }
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_file_name_as_long_as_there_can_be_is_saved_and_a_longer_one_is_refused_at_the_start() {
    let dir = temp_path("long-name", "d");
    std::fs::create_dir(&dir).unwrap();
    let longest = dir.join("f".repeat(255));
    let output = run_program(
        "long-name-1",
        &THREE,
        &["--run", "10", "--flash", longest.to_str().unwrap()],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    // The card too: its name plus a suffix would not have fitted.
    let card = dir.join("c".repeat(255));
    std::fs::write(&card, card_image()).unwrap();
    let output = run_program(
        "long-name-2",
        &write_card(SPIN),
        &["--run", "1000000", "--sd", card.to_str().unwrap()],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(std::fs::read(&card).unwrap()[512..1024], [0x5A; 512]);
    // Nothing else is left beside them.
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);

    let too_long = dir.join("f".repeat(256));
    let output = run_program(
        "long-name-3",
        &THREE,
        &["--run", "10", "--flash", too_long.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!(
            "cmm2-emulator: {}: a name on its path is longer than the file system allows, so the run could not save its flash and clock\n",
            too_long.display()
        )
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_link_to_a_name_that_is_not_a_file_name_is_refused_at_the_start() {
    let dir = temp_path("link-slash", "d");
    std::fs::create_dir(&dir).unwrap();
    let link = dir.join("lnk");
    std::os::unix::fs::symlink("newfile/", &link).unwrap();

    let flash = run_program(
        "link-slash-1",
        &THREE,
        &["--run", "10", "--flash", link.to_str().unwrap()],
    );
    assert_eq!(flash.status.code(), Some(1));
    assert_eq!(
        stderr(&flash),
        format!(
            "cmm2-emulator: {}: it leads to {}/newfile/, which cannot be the name of a file, so the run could not save its flash and clock\n",
            link.display(),
            dir.display()
        )
    );
    let card = run_program(
        "link-slash-2",
        &THREE,
        &["--run", "10", "--sd", link.to_str().unwrap()],
    );
    assert_eq!(card.status.code(), Some(1));
    assert!(stderr(&card).starts_with(&format!("cmm2-emulator: {}: ", link.display())));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_directory_that_cannot_be_read_does_not_fail_a_save_that_has_been_made() {
    if root() {
        return;
    }
    // Write and search but not read: the file can be made and renamed in, and the directory
    // cannot be opened to be synced, nor to look at its attributes. Both are said, as warnings.
    let dir = temp_path("dir-unreadable", "d");
    std::fs::create_dir(&dir).unwrap();
    let flash = dir.join("board.flash");
    mode(&dir, 0o300);

    let output = run_program(
        "dir-unreadable",
        &THREE,
        &["--run", "10", "--flash", flash.to_str().unwrap()],
    );

    mode(&dir, 0o755);
    assert!(output.status.success(), "{}", stderr(&output));
    let lines: Vec<String> = stderr(&output).lines().map(str::to_string).collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].contains("its directory cannot be read"),
        "{lines:?}"
    );
    assert!(
        lines[1].contains("written, but its directory could not be synced"),
        "{lines:?}"
    );
    assert_eq!(std::fs::read(&flash).unwrap().len(), FLASH_FILE_SIZE);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_temporary_file_nobody_holds_is_removed_at_the_start_and_one_that_is_held_is_kept() {
    let dir = temp_path("stale-temp", "d");
    std::fs::create_dir(&dir).unwrap();
    let flash = dir.join("board.flash");
    let old = std::time::SystemTime::now() - Duration::from_secs(3600);
    let left = dir.join(".cmm2.4242.1.tmp");
    let held = dir.join(".cmm2.4242.2.tmp");
    for file in [&left, &held] {
        std::fs::write(file, b"left behind").unwrap();
        std::fs::File::options()
            .write(true)
            .open(file)
            .unwrap()
            .set_modified(old)
            .unwrap();
    }
    // Held by a save that is going on, as this test's own lock stands for.
    let holder = std::fs::File::open(&held).unwrap();
    // SAFETY: flock on a descriptor that is open.
    assert_eq!(
        unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&holder), libc::LOCK_EX) },
        0
    );

    let output = run_program(
        "stale-temp",
        &THREE,
        &["--run", "10", "--flash", flash.to_str().unwrap()],
    );

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!left.exists());
    assert_eq!(std::fs::read(&held).unwrap(), b"left behind");
    drop(holder);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_old_fifo_named_like_a_temporary_file_does_not_hang_the_start() {
    let dir = temp_path("stale-fifo", "d");
    std::fs::create_dir(&dir).unwrap();
    let fifo = dir.join(".cmm2.1.1.tmp");
    make_fifo(&fifo);
    // Two hours old, by its path: opening a FIFO would wait for a writer.
    let two_hours_ago = std::time::SystemTime::now() - Duration::from_secs(7200);
    let secs = two_hours_ago
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let times = [
        libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_OMIT,
        },
        libc::timespec {
            tv_sec: secs as libc::time_t,
            tv_nsec: 0,
        },
    ];
    let name = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: utimensat with a NUL-terminated name and two timespecs that live through the call.
    assert_eq!(
        unsafe { libc::utimensat(libc::AT_FDCWD, name.as_ptr(), times.as_ptr(), 0) },
        0
    );
    let flash = dir.join("board.flash");

    let mut child = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .args([
            "--firmware",
            temp_program("stale-fifo", &THREE).to_str().unwrap(),
        ])
        .args(["--run", "10", "--flash", flash.to_str().unwrap()])
        .spawn()
        .expect("failed to run cmm2-emulator");
    let status = wait_for(&mut child);

    assert!(status.success());
    assert!(fifo.exists());
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_file(temp_path("stale-fifo", "bin")).unwrap();
}
