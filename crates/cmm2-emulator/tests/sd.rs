//! --sd: the card image, its write-protect switch and the write back.

mod common;

use common::*;
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const WRITE_CARD_END_ADDRESS: &str = "0x08000188";

fn untouched(path: &PathBuf, image: &[u8], inode_before: u64) -> bool {
    let meta = std::fs::metadata(path).unwrap();
    meta.ino() == inode_before && std::fs::read(path).unwrap() == image
}

#[test]
fn the_sd_file_is_written_back_whichever_way_the_run_ends() {
    let cases: [(&str, &[&str]); 3] = [
        ("sd-limit", &["--run", "1000000"]),
        ("sd-console", &["--console", "--run", "1000000"]),
        ("sd-stop-at", &["--stop-at", WRITE_CARD_END_ADDRESS]),
    ];
    for (name, args) in cases {
        let (card, image) = sd_card(name);
        let before = inode(&card);
        let mut all = vec!["--sd", card.to_str().unwrap()];
        all.extend_from_slice(args);

        let output = run_program(name, &write_card(SPIN), &all);

        assert!(output.status.success(), "{name}: {}", stderr(&output));
        assert!(written_back(&card, &image, before), "{name}");
        std::fs::remove_file(&card).unwrap();
    }
}

#[test]
fn the_sd_file_is_written_back_when_the_guest_faults() {
    for (name, args) in [
        ("sd-fault-batch", &["--run", "1000000"][..]),
        ("sd-fault-live", &["--console"][..]),
    ] {
        let (card, image) = sd_card(name);
        let before = inode(&card);
        let mut all = vec!["--sd", card.to_str().unwrap()];
        all.extend_from_slice(args);

        let output = run_program(name, &write_card(0xDE00), &all);

        assert_eq!(output.status.code(), Some(1), "{name}");
        assert!(written_back(&card, &image, before), "{name}");
        std::fs::remove_file(&card).unwrap();
    }
}

#[test]
fn a_card_the_guest_did_not_write_to_leaves_the_sd_file_alone() {
    let cases: [(&str, &[u16], &[&str]); 5] = [
        ("sd-clean-limit", &THREE, &["--run", "10"]),
        ("sd-clean-console", &THREE, &["--console", "--run", "10"]),
        ("sd-clean-stop-at", &THREE, &["--stop-at", "0x08000106"]),
        ("sd-clean-fault", &UNDEFINED, &["--run", "100"]),
        ("sd-clean-fault-live", &UNDEFINED, &["--console"]),
    ];
    for (name, code, args) in cases {
        let (card, image) = sd_card(name);
        let before = inode(&card);
        let mut all = vec!["--sd", card.to_str().unwrap()];
        all.extend_from_slice(args);

        run_program(name, code, &all);

        assert!(untouched(&card, &image, before), "{name}");
        std::fs::remove_file(&card).unwrap();
    }
}

#[test]
fn changes_made_to_the_sd_file_during_a_clean_run_are_kept() {
    // The run is long enough to change the file under it; a card never written to is not
    // written back over it.
    let (card, _) = sd_card("sd-host-edit");
    let path = temp_program("sd-host-edit", &ECHO);
    let mut child = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .args(["--firmware", path.to_str().unwrap(), "--console", "--sd"])
        .arg(&card)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to run cmm2-emulator");
    // The guest says "ready" once the card is in and it is running.
    let mut out = child.stdout.take().unwrap();
    let mut seen = Vec::new();
    let mut byte = [0];
    while !seen.ends_with(b"ready\r\n") && out.read(&mut byte).is_ok_and(|n| n == 1) {
        seen.push(byte[0]);
    }
    std::fs::write(&card, vec![0xEE; 2 * 512]).unwrap();
    child.stdin.as_mut().unwrap().write_all(b"\x1D").unwrap();
    assert!(child.wait().unwrap().success());

    assert_eq!(std::fs::read(&card).unwrap(), vec![0xEE; 2 * 512]);
    std::fs::remove_file(&card).unwrap();
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn saving_the_sd_file_leaves_no_temporary_file() {
    let dir = temp_path("sd-temp", "d");
    std::fs::create_dir(&dir).unwrap();
    let card = dir.join("card.img");
    std::fs::write(&card, card_image()).unwrap();

    run_program(
        "sd-temp",
        &write_card(SPIN),
        &["--run", "1000000", "--sd", card.to_str().unwrap()],
    );

    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_missing_sd_file_is_an_error_and_is_not_made() {
    let card = temp_path("sd-missing", "img");
    let _ = std::fs::remove_file(&card);

    let output = run_program(
        "sd-missing",
        &THREE,
        &["--run", "10", "--sd", card.to_str().unwrap()],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        stderr(&output),
        format!(
            "cmm2-emulator: {}: No such file or directory (os error 2)\n",
            card.display()
        )
    );
    assert!(!card.exists());
}

#[test]
fn an_sd_file_that_is_not_whole_blocks_is_an_error_and_is_left_alone() {
    for (name, bytes) in [("sd-ragged", vec![7; 1000]), ("sd-empty", Vec::new())] {
        let card = temp_path(name, "img");
        std::fs::write(&card, &bytes).unwrap();

        let output = run_program(
            name,
            &THREE,
            &["--run", "10", "--sd", card.to_str().unwrap()],
        );

        assert_eq!(output.status.code(), Some(1), "{name}");
        assert!(output.stdout.is_empty(), "{name}");
        assert_eq!(
            stderr(&output),
            format!(
                "cmm2-emulator: {}: {} bytes is not a whole number of 512-byte blocks\n",
                card.display(),
                bytes.len()
            ),
            "{name}"
        );
        assert_eq!(std::fs::read(&card).unwrap(), bytes, "{name}");
        std::fs::remove_file(&card).unwrap();
    }
}

#[test]
fn the_sd_file_is_written_back_when_the_quit_key_ends_the_run() {
    let flash = flash_path("sd-quit");
    let _ = std::fs::remove_file(&flash);
    let (card, image) = sd_card("sd-quit");
    let before = inode(&card);
    let mut child = start_writing_run("sd-quit", &flash, &card);

    child.stdin.as_mut().unwrap().write_all(b"\x1D").unwrap();
    let status = wait_for(&mut child);

    assert!(status.success());
    assert!(written_back(&card, &image, before));
    std::fs::remove_file(&flash).unwrap();
    std::fs::remove_file(&card).unwrap();
}

#[test]
fn a_read_only_sd_file_is_a_write_protected_card_and_is_not_written_back() {
    if root() {
        return;
    }
    let (card, image) = sd_card("sd-readonly");
    let before = inode(&card);
    std::fs::set_permissions(&card, std::os::unix::fs::PermissionsExt::from_mode(0o444)).unwrap();

    // The guest writes anyway, as one that ignores the switch would.
    let output = run_program(
        "sd-readonly",
        &write_card(SPIN),
        &["--run", "1000000", "--sd", card.to_str().unwrap()],
    );

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output).ends_with("the card is write-protected because the file is read-only\n")
    );
    assert!(untouched(&card, &image, before));
    std::fs::set_permissions(&card, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    std::fs::remove_file(&card).unwrap();
}

#[test]
fn an_sd_file_under_512_kb_is_an_error_and_is_left_alone() {
    let card = temp_path("sd-tiny", "img");
    std::fs::write(&card, vec![7; 4 * 512]).unwrap();

    let output = run_program(
        "sd-tiny",
        &THREE,
        &["--run", "10", "--sd", card.to_str().unwrap()],
    );

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!(
            "cmm2-emulator: {}: 2048 bytes is smaller than the 524288 bytes (512 KB) of the smallest card\n",
            card.display()
        )
    );
    assert_eq!(std::fs::read(&card).unwrap(), vec![7; 4 * 512]);
    std::fs::remove_file(&card).unwrap();
}

#[test]
fn an_sd_file_on_a_read_only_file_system_is_a_write_protected_card_that_is_not_written_back() {
    let dir = temp_path("sd-ro-mount", "d");
    std::fs::create_dir(&dir).unwrap();
    let card = dir.join("card.img");
    std::fs::write(&card, card_image()).unwrap();
    mode(&card, 0o664);
    let firmware = temp_program("sd-ro-mount", &write_card(SPIN));

    // The guest writes anyway, as one that ignores the switch would.
    let output = run_with_read_only_mount(
        &dir,
        &firmware,
        &["--run", "1000000", "--sd", card.to_str().unwrap()],
    );

    if let Some(output) = output {
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(
            stderr(&output),
            format!(
                "cmm2-emulator: {}: the card is write-protected because its file system is read-only\n",
                card.display()
            )
        );
        assert_eq!(std::fs::read(&card).unwrap(), card_image());
    }
    std::fs::remove_file(&firmware).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_sd_file_in_a_directory_that_cannot_be_written_is_a_write_protected_card() {
    if root() {
        return;
    }
    let dir = temp_path("sd-dir-ro", "d");
    std::fs::create_dir(&dir).unwrap();
    let card = dir.join("card.img");
    std::fs::write(&card, card_image()).unwrap();
    mode(&dir, 0o555);

    let output = run_program(
        "sd-dir-ro",
        &write_card(SPIN),
        &["--run", "1000000", "--sd", card.to_str().unwrap()],
    );

    mode(&dir, 0o755);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stderr(&output),
        format!(
            "cmm2-emulator: {}: the card is write-protected because its directory is not writable\n",
            card.display()
        )
    );
    assert_eq!(std::fs::read(&card).unwrap(), card_image());
    std::fs::remove_dir_all(&dir).unwrap();
}
