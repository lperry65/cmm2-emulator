use super::*;
use crate::test_support::{HOSTILE_NAME, HOSTILE_SHOWN, in_own_process, mode, root, scratch};
use std::fs::File;
use std::os::unix::fs::symlink;

#[test]
fn a_flash_link_is_judged_by_the_directory_of_the_file_it_points_to() {
    if root() {
        return;
    }
    let dir = scratch("flash-link-dir");
    std::fs::create_dir(dir.join("locked")).unwrap();
    symlink("locked/board.flash", dir.join("link.flash")).unwrap();
    mode(&dir.join("locked"), 0o555);

    let why = why_not_saveable(dir.join("link.flash").to_str().unwrap());

    mode(&dir.join("locked"), 0o755);
    assert_eq!(why.as_deref(), Some("its directory is not writable"));
}

#[test]
fn a_link_to_a_regular_file_is_one() {
    let dir = scratch("regular-link");
    std::fs::write(dir.join("real"), b"x").unwrap();
    symlink("real", dir.join("link")).unwrap();
    assert!(check_regular(dir.join("link").to_str().unwrap()).is_ok());
    assert!(check_regular(dir.join("missing").to_str().unwrap()).is_ok());
}

#[test]
fn a_file_in_a_sticky_directory_is_only_replaced_by_its_owner_or_the_directorys() {
    let sticky = 0o1777;
    // (directory mode, directory owner, file owner, user): refused only when sticky and
    // the user is neither owner and not privileged.
    assert!(sticky_refuses(sticky, 0, 1000, 1001, false));
    assert!(!sticky_refuses(sticky, 0, 1000, 1000, false));
    assert!(!sticky_refuses(sticky, 1001, 1000, 1001, false));
    // It is CAP_FOWNER that lets a user through, and not their number: root without it is
    // refused, and a user with it is not.
    assert!(sticky_refuses(sticky, 1, 1000, 0, false));
    assert!(!sticky_refuses(sticky, 0, 1000, 1001, true));
    assert!(!sticky_refuses(0o777, 0, 1000, 1001, false));
}

#[test]
fn an_ordinary_file_is_not_immutable_and_an_unreadable_flag_is_not_a_reason() {
    let dir = scratch("immutable");
    std::fs::write(dir.join("a"), b"x").unwrap();
    assert!(!is_immutable(&File::open(dir.join("a")).unwrap()));
    assert_eq!(FS_IMMUTABLE_OR_APPEND, 0x30);
    // A file system that does not know the ioctl leaves the flags 0.
    assert!(!is_immutable(&File::open("/dev/null").unwrap()));
}

#[test]
fn searching_a_directory_and_writing_to_it_are_said_apart() {
    if root() {
        return;
    }
    let dir = scratch("search-write");
    std::fs::create_dir(dir.join("d")).unwrap();
    let path = dir.join("d/flash");
    let path = path.to_str().unwrap();

    mode(&dir.join("d"), 0o666);
    assert_eq!(
        why_not_saveable(path).as_deref(),
        Some("a directory on its path cannot be searched")
    );
    mode(&dir.join("d"), 0o555);
    assert_eq!(
        why_not_saveable(path).as_deref(),
        Some("its directory is not writable")
    );
    // A directory above it that cannot be searched hides the whole path.
    mode(&dir.join("d"), 0o755);
    mode(&dir, 0o666);
    assert_eq!(
        why_not_saveable(path).as_deref(),
        Some("a directory on its path cannot be searched")
    );
    mode(&dir, 0o755);
    assert_eq!(why_not_saveable(path), None);
}

#[test]
fn an_ordinary_directory_is_not_append_only() {
    // chattr +a on a directory needs root, so the refusal itself is not tried; what is
    // here is that a directory that is not flagged is not taken for one.
    let dir = scratch("dir-flags");
    assert!(!is_immutable(&File::open(&dir).unwrap()));
    assert_eq!(why_not_saveable(dir.join("flash").to_str().unwrap()), None);
}

// A name that a save replaces and does not follow is judged by its own directory, and a link at it
// is judged as the thing the save replaces: by its own owner, not its target's. No second user is
// to be had here, so what is tried is a link of this user's in the sticky /tmp, pointing at a file
// of another user's: the link's owner is the caller, so it is replaceable. (The decision itself,
// for a link owned by someone else, is `sticky_refuses`, tried above with every owner.) Skipped
// where /tmp is not a sticky directory of another user's, or the caller has CAP_FOWNER.
#[test]
fn a_link_in_a_sticky_directory_is_judged_by_its_own_owner_and_not_its_targets() {
    use std::os::unix::fs::MetadataExt;
    let (dir, target) = (Path::new("/tmp"), Path::new("/etc/passwd"));
    // SAFETY: geteuid has no arguments and cannot fail.
    let me = unsafe { libc::geteuid() };
    let (Ok(dir_meta), Ok(target_meta)) = (std::fs::metadata(dir), std::fs::metadata(target))
    else {
        eprintln!("skipped: no /tmp or no /etc/passwd");
        return;
    };
    if dir_meta.mode() & libc::S_ISVTX == 0
        || dir_meta.uid() == me
        || target_meta.uid() == me
        || has_cap_fowner()
    {
        eprintln!("skipped: /tmp is not a sticky directory of another user's, or no other owner");
        return;
    }
    let link = dir.join(format!("cmm2-main-{}-sticky-link", std::process::id()));
    let _ = std::fs::remove_file(&link);
    symlink(target, &link).unwrap();

    let why = why_not_replaceable(link.to_str().unwrap());

    std::fs::remove_file(&link).unwrap();
    assert_eq!(why, None);
}

// The name a link at which was once used as a stand-in for "the file in this directory" is not
// special: a link planted there leads nowhere, because nothing is looked up through it.
#[test]
fn a_link_planted_at_the_probe_name_is_never_followed() {
    if root() {
        return;
    }
    let dir = scratch("probe-link");
    let (open, locked) = (dir.join("open"), dir.join("locked"));
    std::fs::create_dir(&open).unwrap();
    std::fs::create_dir(&locked).unwrap();
    symlink("../locked/x", open.join(".cmm2.probe")).unwrap();
    symlink("elsewhere", open.join("settings.flash")).unwrap();
    mode(&locked, 0o555);

    let why = why_not_replaceable(open.join("settings.flash").to_str().unwrap());

    mode(&locked, 0o755);
    assert_eq!(why, None, "the directory of the name is writable");
}

// What cannot be seen about a directory is said and not refused, and the saying carries the name of
// the file whose directory it could not read. A process of its own, because what is said goes to
// standard error, which a test process cannot take from itself.
#[test]
fn a_directory_that_cannot_be_read_is_said_with_its_name_escaped() {
    if root() {
        // As root a mode bit stops nothing, so the directory reads and nothing is said.
        return;
    }
    let ran = in_own_process(
        "saveable::tests::a_directory_that_cannot_be_read_is_said_with_its_name_escaped",
        "",
        &[],
        |_| {
            let dir = scratch("saveable-said");
            let inside = dir.join(HOSTILE_NAME);
            std::fs::create_dir(&inside).unwrap();
            let file = inside.join("card.flash");
            std::fs::write(&file, "the old settings").unwrap();
            // Searchable and writable, and not readable: everything the save needs is allowed, and
            // what cannot be seen about the directory is left for the save itself to say.
            mode(&inside, 0o300);
            assert_eq!(why_not_saveable(file.to_str().unwrap()), None);
            mode(&inside, 0o700);
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
        .filter(|line| line.contains("its directory cannot be read"))
        .collect();
    assert_eq!(warned.len(), 1, "one line about it, and no more: {said}");
    let line = warned[0];
    assert!(line.starts_with("cmm2-emulator: "), "{line:?}");
    assert!(
        line.contains("its directory cannot be read (Permission denied (os error 13))"),
        "{line:?}"
    );
    assert!(
        line.ends_with("so whether it is append-only is not known"),
        "{line:?}"
    );
    assert!(line.contains(HOSTILE_SHOWN), "the name is not said: {line}");
}
