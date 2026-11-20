//! The card in the board's socket: remembered, replaced, taken out, and gone.

mod common;

use common::board::*;
use common::*;

#[test]
fn the_card_stays_in_the_socket_until_another_is_given_or_it_is_taken_out() {
    let home = Home::new("board-card");
    let (card, image) = Tmp::card("board-card");
    let (other, _) = Tmp::card("board-card-other");
    let program = Tmp::program("board-card", &write_card(SPIN));
    // Burned and the card put in together.
    let put_in = quit_after(
        &home,
        &["--burn", program.arg(), "--console", "--sd", card.arg()],
        b"done\n",
    );
    assert!(put_in.status.success(), "{}", stderr(&put_in));
    assert_eq!(
        std::fs::read(&card.0).unwrap(),
        written(&image),
        "the guest wrote the card"
    );
    assert_eq!(
        std::fs::read_to_string(home.file("sd.path")).unwrap(),
        card.arg()
    );

    // No --sd: the card that was left in the socket is still there, and written to.
    std::fs::write(&card.0, &image).unwrap();
    assert!(
        quit_after(&home, &["--console"], b"done\n")
            .status
            .success()
    );
    assert_eq!(std::fs::read(&card.0).unwrap(), written(&image));

    // Another card replaces it.
    let swap = quit_after(&home, &["--console", "--sd", other.arg()], b"done\n");
    assert!(swap.status.success());
    assert_eq!(std::fs::read(&other.0).unwrap(), written(&image));
    assert_eq!(
        std::fs::read_to_string(home.file("sd.path")).unwrap(),
        other.arg()
    );
    std::fs::write(&card.0, &image).unwrap();
    std::fs::write(&other.0, &image).unwrap();
    assert!(
        quit_after(&home, &["--console"], b"done\n")
            .status
            .success()
    );
    assert_eq!(
        std::fs::read(&card.0).unwrap(),
        image,
        "the first card is out"
    );
    assert_eq!(std::fs::read(&other.0).unwrap(), written(&image));

    // --no-sd takes it out: the file is left alone and the board remembers none.
    std::fs::write(&other.0, &image).unwrap();
    let out = quit_run(&home, &["--console", "--no-sd"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!home.file("sd.path").exists());
    assert_eq!(std::fs::read(&other.0).unwrap(), image);
    let out = quit_run(&home, &["--console"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(std::fs::read(&other.0).unwrap(), image);
}

#[test]
fn a_card_file_that_has_gone_is_a_clear_error_that_says_how_to_go_on() {
    let home = Home::new("board-card-gone");
    burn_counter(&home, "board-card-gone");
    let (card, _) = Tmp::card("board-card-gone");
    assert!(
        quit_run(&home, &["--console", "--sd", card.arg()])
            .status
            .success()
    );
    std::fs::remove_file(&card.0).unwrap();

    let output = quit_run(&home, &["--console"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let said = stderr(&output);
    assert!(said.contains(card.arg()), "{said}");
    assert!(said.contains("No such file"), "{said}");
    assert!(said.contains("Give another with --sd <file>"), "{said}");
    assert!(said.contains("take it out with --no-sd"), "{said}");
    // The two ways out work.
    assert!(quit_run(&home, &["--console", "--no-sd"]).status.success());
    assert!(quit_run(&home, &["--console"]).status.success());
}

#[test]
fn a_hand_written_card_record_with_a_line_end_works() {
    let home = Home::new("board-card-newline");
    burn_counter(&home, "board-card-newline");
    let (card, _) = Tmp::card("board-card-newline");
    std::fs::write(home.file("sd.path"), format!("{}\n", card.arg())).unwrap();

    let output = quit_run(&home, &["--console"]);

    assert!(output.status.success(), "{}", stderr(&output));
}

#[test]
fn a_burn_is_done_even_if_the_start_then_fails_on_a_card_that_has_gone() {
    // The firmware is burned before the machine is built, and docs/usage.md says so.
    let home = Home::new("board-burn-then-fail");
    burn_counter(&home, "board-burn-then-fail");
    let (card, _) = Tmp::card("board-burn-then-fail");
    assert!(
        quit_run(&home, &["--console", "--sd", card.arg()])
            .status
            .success()
    );
    std::fs::remove_file(&card.0).unwrap();
    let echo = Tmp::program("board-burn-then-fail-echo", &ECHO);

    let output = quit_run(&home, &["--burn", echo.arg(), "--console"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        std::fs::read(home.file("firmware.bin")).unwrap(),
        std::fs::read(&echo.0).unwrap()
    );
}

#[test]
fn a_card_that_cannot_be_read_is_not_remembered() {
    let home = Home::new("board-card-small");
    burn_counter(&home, "board-card-small");
    let small = Tmp::file("board-card-small", "img");
    std::fs::write(&small.0, [1, 2, 3]).unwrap();

    let output = quit_run(&home, &["--console", "--sd", small.arg()]);

    assert_eq!(output.status.code(), Some(1));
    assert!(!home.file("sd.path").exists());
}

#[test]
fn a_card_record_that_is_a_directory_is_told_how_to_go_on_and_both_ways_out_work() {
    let home = Home::new("board-record-dir");
    burn_counter(&home, "board-record-dir");
    let (card, _) = Tmp::card("board-record-dir");
    std::fs::create_dir(home.file("sd.path")).unwrap();

    let plain = quit_run(&home, &["--console"]);
    assert_eq!(plain.status.code(), Some(1));
    assert!(stderr(&plain).contains("--no-sd"), "{}", stderr(&plain));

    assert!(quit_run(&home, &["--console", "--no-sd"]).status.success());
    assert!(!home.file("sd.path").exists());
    std::fs::create_dir(home.file("sd.path")).unwrap();
    assert!(
        quit_run(&home, &["--console", "--sd", card.arg()])
            .status
            .success()
    );
    assert_eq!(
        std::fs::read_to_string(home.file("sd.path")).unwrap(),
        card.arg()
    );
}
