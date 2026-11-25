//! The settings mode a run gets, as the user meets it: a scratch run and a layout-labelled
//! board both read and write the raw file, and each is told what to do when it finds a file of
//! format v2 instead. The board of a firmware I know, which is the run that writes v2, needs a
//! real image and is in the ignored tests of `tests/board_burn.rs`.

mod common;

use common::board::*;
use common::*;

// A scratch run (--firmware, a layout named by the variable) reads the raw layout. A file the
// board wrote (format v2) is not raw, and the run says so and what to do.
#[test]
fn a_run_that_reads_the_raw_layout_refuses_a_v2_file_and_names_the_way_out() {
    let flash = flash_path("modes-v2-in-raw");
    // Bytes shaped like the head of a v2 file and built to be refused: the magic and version 2
    // over a header's worth of zeros. No emulator would read these as settings - the hash and
    // the checksum are zero - and the test does not ask it to. What matters is only the magic:
    // the raw run stops on it before it looks at anything else.
    let mut bytes = vec![0u8; 48 + 8 + 4];
    bytes[..4].copy_from_slice(b"CMM2");
    bytes[4] = 2;
    std::fs::write(&flash, &bytes).unwrap();

    let path = temp_program("modes-v2-in-raw", &THREE);
    let output = run(&[
        "--firmware",
        path.to_str().unwrap(),
        "--console",
        "--flash",
        flash.to_str().unwrap(),
    ]);
    std::fs::remove_file(&path).unwrap();
    let kept = std::fs::read(&flash).unwrap();
    std::fs::remove_file(&flash).unwrap();

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(
        stderr(&output),
        format!(
            "cmm2-emulator: {}: these settings were saved by the board of this emulator \
             (format v2); this run reads the raw layout. Start the board itself, or point \
             --flash at a raw file.\n",
            flash.display()
        )
    );
    assert_eq!(kept, bytes, "the refusal left the file alone");
}

// A board whose layout came from the test variable, not from a firmware I know, stays on the
// raw file: the counter's settings are still the raw dump, and no magic appears in them.
#[test]
fn a_board_of_a_labelled_layout_still_writes_the_raw_file() {
    let home = Home::new("modes-labelled");
    burn_counter(&home, "modes-labelled");

    let settings = std::fs::read(home.file("settings.flash")).unwrap();
    assert_eq!(
        settings.len(),
        FLASH_FILE_SIZE,
        "the labelled board keeps the raw dump"
    );
    assert_ne!(
        &settings[..4],
        b"CMM2",
        "no v2 magic on a layout-labelled board"
    );
    assert_eq!(counter(&home), 0);
}

// The same file met on the board itself: the board is on the raw layout only because the test
// variable named a layout for a firmware I cannot recognise, so the refusal names the variable.
// Telling this run to start the board is telling it to do the thing that just failed.
#[test]
fn a_labelled_board_is_told_which_variable_put_it_on_the_raw_layout() {
    let home = Home::new("modes-labelled-v2");
    burn_counter(&home, "modes-labelled-v2");
    let settings = home.file("settings.flash");
    let mut bytes = vec![0u8; 48 + 8 + 4];
    bytes[..4].copy_from_slice(b"CMM2");
    bytes[4] = 2;
    std::fs::write(&settings, &bytes).unwrap();

    let mut command = on(&home);
    let output = command
        .args(["--console", "--run", "10"])
        .output()
        .expect("failed to run cmm2-emulator");

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let said = stderr(&output);
    assert!(
        said.contains("these settings were saved by the board of this emulator"),
        "{said}"
    );
    assert!(said.contains(BOARD_LAYOUT_VARIABLE), "{said}");
    assert!(
        said.contains("Unset"),
        "it says what to do about it: {said}"
    );
    assert!(
        !said.contains("Start the board itself"),
        "starting the board is what just happened: {said}"
    );
    assert_eq!(
        std::fs::read(&settings).unwrap(),
        bytes,
        "the refusal left the file alone"
    );
}
