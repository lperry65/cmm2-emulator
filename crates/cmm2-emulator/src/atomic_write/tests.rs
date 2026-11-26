use super::*;
use crate::test_support::{
    HOSTILE_NAME, HOSTILE_SHOWN, age, files_in, in_own_process, mode, root, scratch, spawning,
};
use std::fs::File;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

#[test]
fn a_file_is_written_whole_and_replaced_whole() {
    let dir = scratch("atomic-plain");
    let file = dir.join("a.img");
    write_atomically(file.to_str().unwrap(), b"first").unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"first");
    write_atomically(file.to_str().unwrap(), b"second, longer").unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"second, longer");
    // The temporary file does not stay.
    assert_eq!(files_in(&dir), ["a.img"]);
}

#[test]
fn a_symlink_is_written_through_and_stays_a_symlink() {
    let dir = scratch("atomic-link");
    let real = dir.join("real.img");
    let link = dir.join("link.img");
    std::fs::write(&real, b"old").unwrap();
    symlink("real.img", &link).unwrap();

    write_atomically(link.to_str().unwrap(), b"new").unwrap();

    assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
    assert_eq!(std::fs::read(&real).unwrap(), b"new");
    assert_eq!(files_in(&dir), ["link.img", "real.img"]);
}

#[test]
fn a_symlink_to_another_directory_is_written_there() {
    let dir = scratch("atomic-link-dir");
    let elsewhere = dir.join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("real.img"), b"old").unwrap();
    symlink("elsewhere/real.img", dir.join("link.img")).unwrap();

    write_atomically(dir.join("link.img").to_str().unwrap(), b"new").unwrap();

    assert_eq!(std::fs::read(elsewhere.join("real.img")).unwrap(), b"new");
    // The temporary file was made beside the real file, and is gone from both.
    assert_eq!(files_in(&elsewhere), ["real.img"]);
    assert_eq!(files_in(&dir), ["elsewhere", "link.img"]);
}

#[test]
fn the_permissions_of_the_old_file_are_kept() {
    let dir = scratch("atomic-mode");
    let file = dir.join("a.img");
    std::fs::write(&file, b"old").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();

    write_atomically(file.to_str().unwrap(), b"new").unwrap();

    assert_eq!(std::fs::metadata(&file).unwrap().mode() & 0o777, 0o640);
}

#[test]
fn the_temporary_file_is_never_taken_over_and_a_stale_one_does_not_block_the_save() {
    let dir = scratch("atomic-unique");
    let file = dir.join("a.img");
    // Somebody else's file with the name another process would use, and one that an
    // earlier crash left under this very process id (a container starts at the same one).
    let other = dir.join("a.img.1.1.tmp");
    std::fs::write(&other, b"theirs").unwrap();
    let stale = dir.join(format!("a.img.{}.tmp", std::process::id()));
    std::fs::write(&stale, b"stale").unwrap();

    write_atomically(file.to_str().unwrap(), b"new").unwrap();
    write_atomically(file.to_str().unwrap(), b"newer").unwrap();

    assert_eq!(std::fs::read(&file).unwrap(), b"newer");
    assert_eq!(std::fs::read(&other).unwrap(), b"theirs");
    assert_eq!(std::fs::read(&stale).unwrap(), b"stale");
}

#[test]
fn a_dangling_symlink_is_written_through_and_the_target_is_made() {
    let dir = scratch("atomic-dangling");
    let link = dir.join("link.img");
    symlink("real.img", &link).unwrap();

    write_atomically(link.to_str().unwrap(), b"first").unwrap();

    assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
    assert_eq!(std::fs::read(dir.join("real.img")).unwrap(), b"first");
    assert_eq!(files_in(&dir), ["link.img", "real.img"]);
}

#[test]
fn a_chain_of_links_to_a_target_that_is_not_there_is_followed_to_its_end() {
    let dir = scratch("atomic-chain");
    std::fs::create_dir(dir.join("sub")).unwrap();
    symlink("sub/second", dir.join("first")).unwrap();
    // Relative to the directory of the link that holds it, which is sub.
    symlink("../elsewhere.img", dir.join("sub/second")).unwrap();

    write_atomically(dir.join("first").to_str().unwrap(), b"data").unwrap();

    assert_eq!(std::fs::read(dir.join("elsewhere.img")).unwrap(), b"data");
    assert!(
        std::fs::symlink_metadata(dir.join("first"))
            .unwrap()
            .is_symlink()
    );
    assert!(
        std::fs::symlink_metadata(dir.join("sub/second"))
            .unwrap()
            .is_symlink()
    );
}

#[test]
fn a_loop_of_links_is_an_error() {
    let dir = scratch("atomic-loop");
    symlink("b", dir.join("a")).unwrap();
    symlink("a", dir.join("b")).unwrap();

    let err = write_atomically(dir.join("a").to_str().unwrap(), b"x").unwrap_err();

    assert_eq!(err.raw_os_error(), Some(libc::ELOOP));
    assert_eq!(files_in(&dir), ["a", "b"]);
}

#[test]
fn a_failed_write_leaves_the_old_file_and_no_temporary_file() {
    let dir = scratch("atomic-fail");
    // The rename onto a directory fails, after the temporary file is made.
    let target = dir.join("a");
    std::fs::create_dir(&target).unwrap();
    std::fs::create_dir(target.join("inside")).unwrap();

    assert!(write_atomically(target.to_str().unwrap(), b"x").is_err());

    assert_eq!(files_in(&dir), ["a"]);
}

#[test]
fn a_name_with_no_directory_is_written_in_the_current_one() {
    let name = format!("cmm2-main-{}-relative.img", std::process::id());
    write_atomically(&name, b"here").unwrap();
    assert_eq!(std::fs::read(&name).unwrap(), b"here");
    std::fs::remove_file(&name).unwrap();
}

#[test]
fn an_old_temporary_file_that_nobody_holds_is_removed_and_the_others_are_kept() {
    let dir = scratch("stale-temps");
    let held = dir.join(".cmm2.4242.9.tmp");
    let unheld = dir.join(".cmm2.4242.1.tmp");
    let fresh = dir.join(".cmm2.4242.2.tmp");
    // Names that only look like ours, or are not a process's at all.
    let others = [
        ".cmm2.x.tmp",
        ".cmm2.+999999.+1.tmp",
        ".cmm2.-9999999.1.tmp",
        ".cmm2.0.1.tmp",
        ".cmm2.12.tmp",
        ".cmm2.12.3.tmp.keep",
        "a.img.1.1.tmp",
    ];
    for file in [&held, &unheld, &fresh] {
        std::fs::write(file, b"x").unwrap();
    }
    for name in others {
        std::fs::write(dir.join(name), b"x").unwrap();
        age(&dir.join(name), 3600);
    }
    std::fs::write(dir.join("a.img"), b"card").unwrap();
    for file in [&held, &unheld] {
        age(file, 3600);
    }
    // A process that another test starts while this has the lock would hold it until the
    // process execs, and the lock would not be let go of when it is dropped below. So no
    // process is started meanwhile.
    let _alone = spawning();
    // A save that is going on: the file is locked as `create_locked` does, and old.
    let lock = File::open(&held).unwrap();
    assert!(try_lock(&lock));

    remove_stale_temps(dir.join("a.img").to_str().unwrap());

    assert!(!unheld.exists(), "old and nobody's");
    assert!(held.exists(), "held by a save that is going on");
    assert!(fresh.exists(), "too new to tell");
    for name in others {
        assert!(dir.join(name).exists(), "{name}");
    }
    // Once the holder lets go (or is gone), the next start removes it.
    drop(lock);
    remove_stale_temps(dir.join("a.img").to_str().unwrap());
    assert!(!held.exists());
}

#[test]
fn what_is_not_a_regular_file_is_never_opened_by_the_cleanup() {
    let dir = scratch("stale-odd");
    // A FIFO would block an open for reading until someone writes, and a link to a device
    // would be opened as the device. Both are named like a temporary file and are old.
    let fifo = dir.join(".cmm2.4242.1.tmp");
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: mkfifo with a NUL-terminated name that lives through the call.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o644) }, 0);
    age(&fifo, 3600);
    let link = dir.join(".cmm2.4242.2.tmp");
    symlink("/dev/null", &link).unwrap();
    std::fs::write(dir.join("a.img"), b"card").unwrap();

    // This returns, and so does not wait on the FIFO.
    remove_stale_temps(dir.join("a.img").to_str().unwrap());

    assert!(fifo.exists());
    assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
    // The opening itself refuses a link and a FIFO, if one is swapped in after the look.
    assert!(open_to_lock(&link).is_none());
    assert!(open_to_lock(&fifo).is_none());
}

#[test]
fn a_left_over_file_that_cannot_be_read_is_removed_through_a_write_open() {
    if root() {
        return;
    }
    let dir = scratch("stale-wo");
    let write_only = dir.join(".cmm2.4242.1.tmp");
    let nothing = dir.join(".cmm2.4242.2.tmp");
    std::fs::write(&write_only, b"x").unwrap();
    std::fs::write(&nothing, b"x").unwrap();
    age(&write_only, 3600);
    age(&nothing, 3600);
    mode(&write_only, 0o200);
    mode(&nothing, 0o000);
    std::fs::write(dir.join("a.img"), b"card").unwrap();

    remove_stale_temps(dir.join("a.img").to_str().unwrap());

    // Write only can be locked and so is removed; with no right at all it cannot be, and
    // is left (docs/usage.md has the command to remove it).
    assert!(!write_only.exists());
    assert!(nothing.exists());
}

#[test]
fn the_temporary_file_of_a_save_is_locked_while_it_is_open() {
    let dir = scratch("locked-temp");
    let _alone = spawning();
    let temp = dir.join(".cmm2.4242.1.tmp");
    let file = create_locked(&temp, None).unwrap();

    // Nothing else can lock it, and it is the file that is made new, never one taken over.
    assert!(!try_lock(&File::open(&temp).unwrap()));
    assert!(create_locked(&temp, None).is_err());

    drop(file);
    assert!(try_lock(&File::open(&temp).unwrap()));
}

#[test]
fn only_digits_make_a_temporary_files_name() {
    assert!(is_temp_name(".cmm2.12.345.tmp"));
    for name in [
        ".cmm2.12.tmp",
        ".cmm2.a.345.tmp",
        "cmm2.12.345.tmp",
        ".cmm2.12.345",
        ".cmm2.+12.345.tmp",
        ".cmm2.12.+345.tmp",
        ".cmm2.-12.345.tmp",
        ".cmm2.0.1.tmp",
        ".cmm2.00.1.tmp",
        ".cmm2..1.tmp",
        ".cmm2.1..tmp",
        ".cmm2.99999999999999999999.1.tmp",
    ] {
        assert!(!is_temp_name(name), "{name}");
    }
}

#[test]
fn a_new_file_is_made_and_a_name_that_is_taken_is_never_replaced() {
    let dir = scratch("atomic-create-new");
    let target = dir.join("a");
    let path = target.to_str().unwrap();

    create_new_atomically(path, b"first", 0o600).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"first");
    assert_eq!(files_in(&dir), ["a"], "no temporary file is left");

    let err = create_new_atomically(path, b"second", 0o600).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&target).unwrap(), b"first");
    assert_eq!(
        files_in(&dir),
        ["a"],
        "a refusal leaves no temporary file either"
    );

    // A link at the name is a name that is taken, and what it leads to is left alone.
    let victim = dir.join("victim");
    std::fs::write(&victim, b"theirs").unwrap();
    symlink(&victim, dir.join("link")).unwrap();
    let err = create_new_atomically(dir.join("link").to_str().unwrap(), b"x", 0o600).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&victim).unwrap(), b"theirs");
}

// The error of a hard link that a file system refuses is told apart from the rest. Linux says
// EPERM for the link of a directory, as it does for a file system with no hard links.
#[test]
fn a_hard_link_that_is_refused_is_unsupported_and_a_name_that_is_taken_is_not() {
    let dir = scratch("atomic-link-refused");
    std::fs::create_dir(dir.join("a-directory")).unwrap();
    std::fs::write(dir.join("taken"), b"x").unwrap();
    std::fs::write(dir.join("plain"), b"y").unwrap();

    let refused = link_to(&dir.join("a-directory"), &dir.join("new")).unwrap_err();
    let taken = link_to(&dir.join("plain"), &dir.join("taken")).unwrap_err();

    assert_eq!(refused.kind(), io::ErrorKind::Unsupported);
    assert_eq!(taken.kind(), io::ErrorKind::AlreadyExists);
}

// A file that is in its place while its directory could not be synced is said, and the name is the
// user's to give: one line, and nothing in it a terminal acts on. A process of its own, because
// what is said goes to standard error, which a test process cannot take from itself.
#[test]
fn a_directory_that_could_not_be_synced_is_said_with_its_name_escaped() {
    if root() {
        // As root a mode bit stops nothing, so the directory syncs and nothing is said.
        return;
    }
    let ran = in_own_process(
        "atomic_write::tests::a_directory_that_could_not_be_synced_is_said_with_its_name_escaped",
        "",
        &[],
        |_| {
            let dir = scratch("atomic-said");
            let inside = dir.join(HOSTILE_NAME);
            std::fs::create_dir(&inside).unwrap();
            let file = inside.join("settings.flash");
            // Writing and renaming need no read, so the save goes through and only the sync of the
            // directory fails.
            mode(&inside, 0o300);
            write_atomically(file.to_str().unwrap(), b"the settings").unwrap();
            mode(&inside, 0o700);
            assert_eq!(std::fs::read(&file).unwrap(), b"the settings");
            // The child ends with `_exit`, so its own directory is removed here.
            std::fs::remove_dir_all(&*dir).unwrap();
        },
    );
    let Some(ran) = ran else { return };
    assert_eq!(
        ran.ended,
        crate::test_support::Ended::Exit(0),
        "{}",
        ran.stderr
    );
    let said = ran.stderr;
    assert!(
        !said.contains('\u{1b}'),
        "the name recoloured the child's error: {said:?}"
    );
    let warned: Vec<&str> = said
        .lines()
        .filter(|line| line.contains("its directory could not be synced"))
        .collect();
    assert_eq!(warned.len(), 1, "one line about it, and no more: {said}");
    let line = warned[0];
    assert!(line.starts_with("cmm2-emulator: "), "{line:?}");
    assert!(
        line.ends_with(
            ": written, but its directory could not be synced (Permission denied (os error 13))",
        ),
        "{line:?}"
    );
    assert!(line.contains(HOSTILE_SHOWN), "the name is not said: {line}");
}
