use super::*;
use crate::test_support::{mode, root, scratch};
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
