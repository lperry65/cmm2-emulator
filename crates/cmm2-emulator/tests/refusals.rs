//! Refusals that only a unit test had ever said, driven here end-to-end through the real
//! binary: the message has to come out of a run, not out of the error built by hand.
//!
//! Three are here. Each is pinned by three things at once, through `assert_refusal`: the exit
//! status, the one line said on standard error, and two counts of raw bytes in it (no `ESC`,
//! one line feed). Each also runs the same command with its fault taken away and asserts the
//! run is not refused, so the refusal cannot be something the test would pass anyway. Where the
//! message carries a name, the name is also run hostile, holding a colour change and a line
//! feed, and its escaped rendering is pinned.
//!
//! Four refusals are not here, because nothing on this machine can be made to fail the way they
//! need. What is missing is the fault, not the staging of a file, and I would rather say so than
//! write a test that passes for another reason:
//!
//! - The board that cannot take its lock (`BoardError::NoLock`) needs the lock file to open and
//!   `flock` to refuse it afterwards. Everything this user can open takes a lock: of every entry
//!   of `/dev`, `/dev/pts` and `/dev/shm` that an open for writing accepted, and of the 927
//!   world-readable regular files of `/proc` and `/sys` I tried, whose open for writing is refused
//!   and so are the branch that opens the lock to read instead, every one was locked. What is left
//!   is a file system with no lock support (an NFS or FUSE mount), which needs privileges I do not
//!   have, and a descriptor closed under the emulator's feet, which no file can cause.
//! - The migration that cannot make its hard link (`BackupNeedsLink`) needs `link` to answer
//!   `EPERM` or `EOPNOTSUPP`, which on this machine only a file system without hard links (a FAT
//!   one) does. I cannot mount one, and the hard-link guard does not help: `protected_hardlinks`
//!   is `1` here and the file being linked is the emulator's own, which the guard leaves alone.
//! - The migration that loses its race every time (`BackupRaceLost`) needs a backup name that is
//!   free when it is looked at and taken when the link is made, 99 times in a row. Both questions
//!   are asked about one name in one directory, so any file system that is not being written
//!   underneath answers them the same way; a second writer would supply the difference, and a
//!   retry loop around a real race is not a test.
//! - Settings that do not read back (`NotReadBack`) need the encoder to write a file its own
//!   decoder refuses. Read from the code, the two cannot disagree: `Cpu::save_settings` walks
//!   each region of the profile in 32-byte rows, so every entry is aligned, whole, inside one
//!   region and in order; it names the image it holds and that image's own SHA-256, which is
//!   what the hash gate compares against; and the checksum covers the bytes just written. So no
//!   `--flash` file, guest or file system reaches this arm in any way I could find, and it is a
//!   fault of the encoder alone. That is read from `cmm2-core`, not proven by a run.

mod common;

use common::board::*;
use common::*;
use std::process::Stdio;

// A name holding a colour change and a line feed, and the same name as a message must render it:
// each control character written out where it stands, one line, no raw escape byte. Nothing else
// in it needs escaping, so no backslash is doubled.
const HOSTILE: &str = "evil\n\u{1b}[31mRED\u{1b}[0m";
const SHOWN: &str = r"evil\n\x1B[31mRED\x1B[0m";

// Burns a program onto a board of a test's own, so that a later run of it starts from a firmware
// that is already there.
fn burn(home: &Home, name: &str) -> Tmp {
    let program = Tmp::program(name, &THREE);
    let burn = quit_run(home, &["--burn", program.arg(), "--console"]);
    assert!(burn.status.success(), "{}", stderr(&burn));
    program
}

// Whether the board's directory holds a temporary file of a write that failed.
fn temps_in(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".cmm2."))
        .collect()
}

// The board's record of the card is written like its firmware, through a temporary file in the
// board's own directory. A directory that cannot be written refuses it before the guest starts.
// The board's settings file is pointed outside the directory, or the run would be refused for
// that file first and never reach the record.
#[cfg(target_os = "linux")]
#[test]
fn a_card_record_that_cannot_be_written_is_refused() {
    if root() {
        return;
    }
    let home = Home::new("refusal-card");
    let _cleanup = Cleanup(vec![home.0.clone()]);
    let _program = burn(&home, "refusal-card");
    let card = Tmp::card("refusal-card");
    let flash = Tmp::file("refusal-card", "flash");

    // The control: the same command with the directory writable puts the card in the socket.
    let ok = quit_run(
        &home,
        &["--sd", card.0.arg(), "--flash", flash.arg(), "--console"],
    );
    assert!(ok.status.success(), "{}", stderr(&ok));
    let remembered = std::fs::read(home.file("sd.path")).unwrap();
    assert_eq!(remembered, card.0.arg().as_bytes());

    mode(&home.0, 0o500);
    let output = quit_run(
        &home,
        &["--sd", card.0.arg(), "--flash", flash.arg(), "--console"],
    );
    mode(&home.0, 0o700);

    assert_refusal(
        &output,
        1,
        &format!(
            "{}/sd.path: its directory is not writable, so the board could not remember the card",
            home.0.display()
        ),
    );
    // The record is the one the control wrote, and the temporary file it made is not left behind.
    assert_eq!(std::fs::read(home.file("sd.path")).unwrap(), remembered);
    assert_eq!(temps_in(&home.0), Vec::<String>::new());
}

// The same refusal with a board directory named to break the line it is said on and recolour it.
#[cfg(target_os = "linux")]
#[test]
fn a_card_record_that_cannot_be_written_says_a_hostile_directory_escaped() {
    if root() {
        return;
    }
    let home = Home::new(HOSTILE);
    let _cleanup = Cleanup(vec![home.0.clone()]);
    let _program = burn(&home, "refusal-card-hostile");
    let card = Tmp::card("refusal-card-hostile");
    let flash = Tmp::file("refusal-card-hostile", "flash");

    mode(&home.0, 0o500);
    let output = quit_run(
        &home,
        &["--sd", card.0.arg(), "--flash", flash.arg(), "--console"],
    );
    mode(&home.0, 0o700);

    // The hostile part of the path, and only that, is what the message escapes.
    let said_dir = home.0.display().to_string().replace(HOSTILE, SHOWN);
    assert_refusal(
        &output,
        1,
        &format!(
            "{said_dir}/sd.path: its directory is not writable, so the board could not remember \
             the card"
        ),
    );
}

// The console writes the guest's output as it goes. Given a descriptor that takes no write, which
// is what standard output redirected *from* a file is, it says the refusal once and ends the run.
// The program is one that writes as soon as it boots, so the write is reached.
#[test]
fn a_console_that_refuses_its_output_is_refused() {
    let program = temp_program("refusal-console", &ECHO);
    let _cleanup = Cleanup(vec![program.clone()]);
    let args = [
        "--firmware",
        program.to_str().unwrap(),
        "--console",
        "--run",
        "20000",
    ];

    // The control: with a pipe for its output the run prints what the guest wrote and ends clean.
    let ok = run(&args);
    assert!(ok.status.success(), "{}", stderr(&ok));
    assert!(stdout(&ok).contains("ready"), "{}", stdout(&ok));

    let read_only = std::fs::File::open("/dev/null").unwrap();
    let output = emulator()
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(read_only))
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to run cmm2-emulator")
        .wait_with_output()
        .expect("the run did not end");

    assert_refusal(
        &output,
        1,
        "writing the console output: Bad file descriptor (os error 9)",
    );
}

// A window that SDL will not open, because the video driver it was told to use does not exist.
// What SDL says about the driver is its own words, which the run escapes like any other name.
#[test]
fn a_window_that_will_not_open_is_refused() {
    let program = temp_program("refusal-window", &THREE);
    let _cleanup = Cleanup(vec![program.clone()]);
    let args = [
        "--firmware",
        program.to_str().unwrap(),
        "--window",
        "--run",
        "100",
    ];

    // The control: the dummy driver opens a window, and the run ends at its limit.
    let ok = emulator()
        .args(args)
        .env("SDL_VIDEODRIVER", "dummy")
        .output()
        .expect("failed to run cmm2-emulator");
    assert!(ok.status.success(), "{}", stderr(&ok));

    let missing = emulator()
        .args(args)
        .env("SDL_VIDEODRIVER", "nosuchdriver")
        .output()
        .expect("failed to run cmm2-emulator");
    assert_refusal(
        &missing,
        1,
        "window: nosuchdriver not available (drop --window to run without a window)",
    );

    let hostile = emulator()
        .args(args)
        .env("SDL_VIDEODRIVER", HOSTILE)
        .output()
        .expect("failed to run cmm2-emulator");
    assert_refusal(
        &hostile,
        1,
        &format!("window: {SHOWN} not available (drop --window to run without a window)"),
    );
}
