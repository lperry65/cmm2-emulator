use super::*;

use crate::args::RunOptions;
use crate::run_and_save::{run_and_save, run_and_save_saying};
use crate::test_support::{PANICS, idle_cpu, scratch};
use cmm2_core::SdSlot;
use std::io;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

#[test]
fn a_panic_message_that_was_held_back_is_said_after_the_files_are_saved() {
    // A writer that notes whether the flash file was there when the message came.
    struct Probe {
        file: PathBuf,
        seen: Option<(bool, String)>,
        all: String,
    }
    impl Write for Probe {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let text = String::from_utf8_lossy(bytes).into_owned();
            self.all.push_str(&text);
            self.seen = Some((self.file.exists(), text));
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let _turn = PANICS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // The real hook is in, as for the tests of it, so it holds this test's own panic too.
    with_the_hook();
    let dir = scratch("late-panic");
    let file = dir.join("board.flash");
    let options = RunOptions {
        flash: Some(file.to_str().unwrap().to_string()),
        ..RunOptions::default()
    };
    let mut probe = Probe {
        file: file.clone(),
        seen: None,
        all: String::new(),
    };
    let mut cpu = idle_cpu();
    // Two messages that were already held when the run panicked.
    hold_panic("the message\n".to_string());
    hold_panic("and another\n".to_string());

    let panicked = catch_unwind(AssertUnwindSafe(|| {
        run_and_save_saying(
            &mut cpu,
            &SdSlot::new(),
            &options,
            |_| -> () { panic!("a bug in the emulator") },
            &mut probe,
        )
    }));

    assert!(panicked.is_err());
    // Said in the order they came, after the save, the run's own panic (held by the hook)
    // last; and not held any more.
    assert!(
        probe.all.starts_with("the message\nand another\n"),
        "{}",
        probe.all
    );
    assert!(probe.all.contains("a bug in the emulator"), "{}", probe.all);
    let last = probe.seen.as_ref().unwrap();
    assert!(last.0, "the file was there when the last was said");
    assert!(last.1.contains("a bug in the emulator"), "{last:?}");
    assert!(HELD_PANICS.lock().unwrap().is_empty());
    assert!(!HOLDING.load(std::sync::atomic::Ordering::Acquire));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_panic_during_a_run_is_held_whatever_standard_error_is_and_only_so_many_are_kept() {
    let _turn = PANICS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // The message is not written when it happens, so what standard error is (a pipe with
    // room for a page, or a nearly full one) does not decide whether the save is waited for.
    for n in 0..HELD_PANICS_MAX + 4 {
        hold_panic(format!("panic {n}\n"));
    }
    let mut said = Vec::new();
    say_late_panic(&mut said);
    let said = String::from_utf8(said).unwrap();
    // The first ones, oldest first, and no more than the bound.
    assert_eq!(said.lines().count(), HELD_PANICS_MAX);
    assert!(said.starts_with("panic 0\npanic 1\n"));
    assert!(!said.contains(&format!("panic {HELD_PANICS_MAX}\n")));
    let mut again = Vec::new();
    say_late_panic(&mut again);
    assert!(again.is_empty());
}

#[test]
fn a_panic_is_described_in_the_words_of_the_default_message() {
    let payload: Box<dyn std::any::Any + Send> = Box::new("it broke");
    assert_eq!(
        describe_panic(Some("main"), Some(("src/x.rs", 3, 9)), &*payload),
        "thread 'main' panicked at src/x.rs:3:9:\nit broke\n"
    );
    let payload: Box<dyn std::any::Any + Send> = Box::new(String::from("formatted 7"));
    assert_eq!(
        describe_panic(None, None, &*payload),
        "thread '<unnamed>' panicked:\nformatted 7\n"
    );
}

// Puts the panic hook in, once, for the tests that look at it; it only acts during a run.
fn with_the_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(install_panic_hook);
    set_main_thread();
}

#[test]
fn the_installed_hook_holds_a_panic_of_the_run_and_it_is_said_after_the_save() {
    let _turn = PANICS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    with_the_hook();
    struct Probe {
        file: PathBuf,
        said: Vec<(bool, String)>,
    }
    impl Write for Probe {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.said.push((
                self.file.exists(),
                String::from_utf8_lossy(bytes).into_owned(),
            ));
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let dir = scratch("hook-panic");
    let file = dir.join("board.flash");
    let options = RunOptions {
        flash: Some(file.to_str().unwrap().to_string()),
        ..RunOptions::default()
    };
    let mut probe = Probe {
        file: file.clone(),
        said: Vec::new(),
    };
    let mut cpu = idle_cpu();

    let panicked = catch_unwind(AssertUnwindSafe(|| {
        run_and_save_saying(
            &mut cpu,
            &SdSlot::new(),
            &options,
            |_| -> () { panic!("a bug in the run") },
            &mut probe,
        )
    }));

    assert!(panicked.is_err());
    // Held, and not written when it happened: it is said once, after the file is there.
    assert_eq!(probe.said.len(), 1, "{:?}", probe.said);
    assert!(probe.said[0].0, "the file was written first");
    assert!(probe.said[0].1.contains("panicked at"), "{:?}", probe.said);
    assert!(
        probe.said[0].1.contains("a bug in the run"),
        "{:?}",
        probe.said
    );
    assert!(HELD_PANICS.lock().unwrap().is_empty());
    // A panic after the run is the default hook's again, and the thread can hold again.
    assert!(!PANIC_SEEN.with(std::cell::Cell::get));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_panic_puts_the_terminal_back_on_the_main_thread_and_not_on_another() {
    let _turn = PANICS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    with_the_hook();
    // A terminal as it is made, cooked: what raw mode saves and a panic puts back.
    let (mut master, mut slave) = (0, 0);
    // SAFETY: openpty fills in two descriptors, which are closed below.
    unsafe {
        assert_eq!(
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null()
            ),
            0
        );
    }
    let lflag = || {
        // SAFETY: tcgetattr fills a plain termios.
        unsafe {
            let mut termios: libc::termios = std::mem::zeroed();
            libc::tcgetattr(slave, &mut termios);
            termios.c_lflag
        }
    };
    let panic_in_a_run = || {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            run_and_save_saying(
                &mut idle_cpu(),
                &SdSlot::new(),
                &RunOptions::default(),
                |_| -> () { panic!("a bug in the run") },
                &mut io::sink(),
            )
        }));
    };
    let raw = terminal::RawMode::enable_on(slave).unwrap();
    let in_raw_mode = lflag();

    // Another thread is the main one: this panic leaves the terminal as it is.
    *MAIN_THREAD.lock().unwrap() = Some(
        std::thread::spawn(|| std::thread::current().id())
            .join()
            .unwrap(),
    );
    panic_in_a_run();
    assert_eq!(
        lflag(),
        in_raw_mode,
        "restored from a thread that is not the main one"
    );

    // This thread is the main one: the terminal is put back before the message.
    set_main_thread();
    panic_in_a_run();
    assert_ne!(lflag(), in_raw_mode, "not restored from the main thread");
    std::mem::forget(raw);
    // SAFETY: closing the two descriptors made above.
    unsafe {
        libc::close(master);
        libc::close(slave);
    }
}

// A child forked from a process that has raw mode on gets a copy of what says so. The signal
// tests fork like this while other tests have a terminal raw, and a child that put that terminal
// back would be undoing what is not its own.
#[test]
fn a_forked_child_does_not_put_the_terminal_back_that_its_parent_made_raw() {
    let _turn = PANICS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut master, mut slave) = (0, 0);
    // SAFETY: openpty fills in two descriptors, which are closed below.
    unsafe {
        assert_eq!(
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null()
            ),
            0
        );
    }
    let lflag = || {
        // SAFETY: tcgetattr fills a plain termios.
        unsafe {
            let mut termios: libc::termios = std::mem::zeroed();
            libc::tcgetattr(slave, &mut termios);
            termios.c_lflag
        }
    };
    let raw = terminal::RawMode::enable_on(slave).unwrap();
    let in_raw_mode = lflag();

    // SAFETY: the child only calls `restore` (a lock-free tcsetattr) and `_exit`.
    unsafe {
        let pid = libc::fork();
        if pid == 0 {
            terminal::restore();
            libc::_exit(0);
        }
        let mut status = 0;
        libc::waitpid(pid, &mut status, 0);
    }
    assert_eq!(
        lflag(),
        in_raw_mode,
        "put back by a child that did not make it raw"
    );

    // The process that made it raw still can.
    drop(raw);
    assert_ne!(
        lflag(),
        in_raw_mode,
        "not put back by the process that made it raw"
    );
    // SAFETY: closing the two descriptors made above.
    unsafe {
        libc::close(master);
        libc::close(slave);
    }
}

// A panic in a `Drop` while a panic is being unwound, which aborts the process. The test
// below runs this one in a process of its own (it is a test that does nothing without the
// variable the other sets).
#[test]
fn double_panic_in_a_process_of_its_own() {
    if std::env::var_os("CMM2_TEST_DOUBLE_PANIC").is_none() {
        return;
    }
    struct Bomb;
    impl Drop for Bomb {
        fn drop(&mut self) {
            panic!("the second panic");
        }
    }
    with_the_hook();
    let _ = run_and_save(
        &mut idle_cpu(),
        &SdSlot::new(),
        &RunOptions::default(),
        |_| -> () {
            let _bomb = Bomb;
            panic!("the first panic")
        },
    );
}

#[test]
fn a_double_panic_writes_everything_that_was_held_and_the_new_message_before_the_abort() {
    use std::os::unix::process::ExitStatusExt;
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "panic_hook::tests::double_panic_in_a_process_of_its_own",
            "--nocapture",
        ])
        .env("CMM2_TEST_DOUBLE_PANIC", "1")
        .output()
        .unwrap();

    let said = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.signal(), Some(libc::SIGABRT), "{said}");
    let first = said.find("the first panic").expect(&said);
    let second = said.find("the second panic").expect(&said);
    assert!(first < second, "oldest first: {said}");
}
