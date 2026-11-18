//! Keeps the tests off the owner's own board: the emulator is only ever started through
//! `common::emulator()`, which points `CMM2_EMULATOR_HOME`, `XDG_DATA_HOME`, `HOME` and
//! `USERPROFILE` at a directory that cannot be made. The binary's path is a private constant of
//! `common/mod.rs`, so no other file can name it by that. What this checks is the text of the
//! other test files, for the names of the ways of getting the path some other way: the constant,
//! cargo's own variable (even split in two or built with `concat!`), `current_exe`,
//! `/proc/self/exe`, `read_link`, and `get_program` (which copies the program of a command
//! without the environment that goes with it; `common::emulator_path()` is the one place that
//! does, and it is counted). It is a text check and can be got round by someone who means to:
//! what it cannot see is a path made up by hand, or a shell command that finds the binary by
//! some other road. That is for review. What does not depend on it is the environment that
//! `emulator()` sets, which no spelling of the other variables can undo without naming them.

mod common;

use std::path::{Path, PathBuf};

// The names in `text`, as the identifiers a Rust file has in it (so `CMM2_EMULATOR_HOME` is one
// name and not `EMULATOR` with something round it, and spaces and line breaks do not matter).
fn names(text: &str) -> Vec<&str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|name| !name.is_empty())
        .collect()
}

// Whether a file, other than common/mod.rs, gets at the emulator binary by itself.
fn gets_the_binary(text: &str) -> bool {
    // Without blanks, so that a name or a path split over lines or spaces is still seen.
    let packed: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    packed.contains("/proc/self/exe")
        || packed.contains("env!(concat!")
        || names(text).iter().any(|name| {
            // The halves of cargo's variable are names of their own when it is split.
            *name == "EMULATOR"
                || name.starts_with("CARGO_BIN")
                || name.starts_with("BIN_EXE")
                || name.starts_with("_EXE")
                || ["current_exe", "get_program", "read_link"].contains(name)
        })
}

fn files(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn no_test_file_names_the_emulator_binary_but_the_common_helper() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut all = Vec::new();
    files(&dir, &mut all);
    assert!(all.len() > 10, "found only {all:?}");
    // This file holds the names as examples to look for, and `common/mod.rs` is looked at below.
    let common = dir.join("common").join("mod.rs");
    let this = dir.join("guard.rs");
    let found: Vec<_> = all
        .iter()
        .filter(|path| **path != common && **path != this)
        .filter(|path| gets_the_binary(&std::fs::read_to_string(path).unwrap()))
        .collect();
    assert!(
        found.is_empty(),
        "start the emulator through common::emulator(): {found:?}"
    );
}

#[test]
fn the_helper_file_uses_the_binary_in_the_helper_and_one_other_place_that_sets_the_board() {
    // `emulator()` and `run_with_read_only_mount`, which sets the board's directory itself: the
    // constant's own line is a third. A new use has to be looked at by whoever adds it.
    let text =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/common/mod.rs"))
            .unwrap();
    let uses = names(&text)
        .iter()
        .filter(|name| **name == "EMULATOR")
        .count();
    let programs = names(&text)
        .iter()
        .filter(|name| **name == "get_program")
        .count();
    assert_eq!(
        programs, 1,
        "get_program is named {programs} times in common/mod.rs, not the 1 of `emulator_path`: \
         a program that is copied from `emulator()` leaves its environment behind, so a new use \
         has to set the board variables to the impossible board itself"
    );
    assert_eq!(
        uses, 3,
        "EMULATOR is named {uses} times in common/mod.rs, not the 3 that are looked at: if you \
         added a use on purpose, check that it sets CMM2_EMULATOR_HOME (and the other board \
         variables) to the impossible board, as `emulator()` does, and change this number"
    );
}

#[test]
fn the_check_sees_every_way_of_naming_the_binary_that_i_could_think_of() {
    for text in [
        "Command::new(common::EMULATOR)",
        "Command::new( common::EMULATOR )",
        "Command::new(\n    common::EMULATOR,\n)",
        "let exe = common::EMULATOR; Command::new(exe)",
        "let exe = EMULATOR;",
        "env!(\"CARGO_BIN_EXE_cmm2-emulator\")",
        "env!(concat!(\"CARGO_\", \"BIN_EXE_cmm2-emulator\"))",
        "std::env::current_exe().unwrap().parent()",
        "Command::new(env!(concat!(\"CARGO_BIN\", \"_EXE_cmm2-emulator\")))",
        "Command::new(env!(\n    concat!(\"CARGO\", \"_BIN_EXE_x\"),\n))",
        "Command::new(common::emulator().get_program())",
        "let program = emulator().get_program();",
        "std::fs::read_link(\"/proc/self/exe\")",
        "std::fs::read_link( \"/proc/self/ exe\" )",
        "std::fs::read_link(path).unwrap().parent().unwrap()",
        "std::fs::File::open(\"/proc/self/exe\")",
        "Command::new(env!(concat!(\"CARGO_BI\", \"N_EXE_x\")))",
        "option_env!(\"CARGO_BIN_EXE_cmm2-emulator\")",
        "std::env::var(\"CARGO_BIN_EXE_cmm2-emulator\")",
    ] {
        assert!(gets_the_binary(text), "{text}");
    }
    for text in [
        "common::emulator().args([\"--version\"])",
        "env(HOME_VARIABLE, &home)",
        "\"CMM2_EMULATOR_HOME\"",
        "// an emulator",
    ] {
        assert!(!gets_the_binary(text), "{text}");
    }
}

#[test]
fn the_guard_kills_and_reaps_its_child_when_it_is_dropped() {
    // With its output closed, so that a guard that fails to kill it cannot hold the test run up.
    let child = std::process::Command::new("sleep")
        .arg("60")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id() as libc::pid_t;
    drop(common::Guard::new(child));
    // Reaped: the process is not there, not even as a zombie that was never waited for.
    // SAFETY: signal 0 only asks whether the process exists.
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    assert!(!alive, "the child is still there");
}

#[test]
fn the_helper_sets_every_place_the_board_could_be_looked_for_to_one_that_cannot_exist() {
    let command = common::emulator();
    for name in [
        common::HOME_VARIABLE,
        "XDG_DATA_HOME",
        "HOME",
        "USERPROFILE",
    ] {
        let value = command
            .get_envs()
            .find(|(var, _)| *var == name)
            .and_then(|(_, value)| value);
        assert_eq!(
            value,
            Some(std::ffi::OsStr::new(common::NO_BOARD)),
            "{name}"
        );
    }
    assert!(std::fs::create_dir_all(common::NO_BOARD).is_err());
}

#[test]
fn the_helper_points_at_a_board_that_cannot_exist() {
    let command = common::emulator();
    let home = command
        .get_envs()
        .find(|(name, _)| *name == common::HOME_VARIABLE)
        .and_then(|(_, value)| value);
    assert_eq!(home, Some(std::ffi::OsStr::new(common::NO_BOARD)));
    assert!(std::fs::create_dir_all(common::NO_BOARD).is_err());
}
