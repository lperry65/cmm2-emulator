use super::*;

use crate::args::RunOptions;
use crate::panic_hook::HOLDING;
use crate::test_support::{PANICS, idle_cpu, scratch};
use cmm2_core::B5;
use cmm2_core::SdSlot;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};

#[test]
fn the_files_are_saved_when_the_run_returns() {
    // `run_and_save` sets and clears the process-wide `HOLDING`, so it takes its turn.
    let _turn = PANICS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = scratch("save-return");
    let file = dir.join("board.flash");
    let options = RunOptions {
        flash: Some(file.to_str().unwrap().to_string()),
        ..RunOptions::default()
    };
    let mut cpu = idle_cpu();

    let (answer, saved) = run_and_save(&mut cpu, &SdSlot::new(), &options, |_| 7);

    assert_eq!(answer, 7);
    saved.unwrap();
    assert!(file.exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_files_are_saved_when_the_run_panics_and_the_panic_carries_on() {
    let _turn = PANICS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = scratch("save-panic");
    let file = dir.join("board.flash");
    let options = RunOptions {
        flash: Some(file.to_str().unwrap().to_string()),
        ..RunOptions::default()
    };
    let mut cpu = idle_cpu();

    // The message that was held back goes nowhere that shows.
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        run_and_save_saying(
            &mut cpu,
            &SdSlot::new(),
            &options,
            |_| -> () { panic!("a bug in the emulator") },
            &mut io::sink(),
        )
    }));

    let panic = panicked.expect_err("the panic is not swallowed");
    assert_eq!(panic.downcast_ref::<&str>(), Some(&"a bug in the emulator"));
    assert_eq!(std::fs::read(&file).unwrap().len(), B5.persistent_size());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_run_is_marked_as_holding_panic_messages_while_it_runs_and_not_after() {
    let _turn = PANICS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut cpu = idle_cpu();
    let (during, _) = run_and_save(&mut cpu, &SdSlot::new(), &RunOptions::default(), |_| {
        HOLDING.load(std::sync::atomic::Ordering::Acquire)
    });
    assert!(during);
    assert!(!HOLDING.load(std::sync::atomic::Ordering::Acquire));
}
