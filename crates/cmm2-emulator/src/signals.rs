//! Ending a run on SIGINT, SIGTERM or SIGHUP (Ctrl-C when the terminal is not in raw mode, kill,
//! the terminal being closed). The handler only sets a flag and writes a byte to a pipe; the run
//! loop looks at the flag between slices and leaves by its normal way out, which writes the
//! files. Nothing is saved from the handler itself, as almost nothing is safe to do in one.
//!
//! A signal that was ignored when the emulator started (`nohup`, a background job) is left
//! ignored. Until the run begins (`begin_run`) there is nothing to save, so a signal ends the
//! process at once, with the signal's own status: a load that is stuck, on a slow disk or a
//! hung mount, can be interrupted. From then on the signal is a flag. The threads the run makes
//! (the stdin reader, SDL's) are made inside `with_blocked`, so that they have the signals
//! blocked and the main thread is the one that takes them.
//!
//! The pipe is what a write to stdout waits on as well as on the descriptor (see `wake_fd`), so
//! a run that is blocked on a reader that is not reading still sees the signal, with no gap
//! between looking at the flag and starting to wait for it. When the run is done and its files
//! are written, `die_if_caught` puts the default action back and raises the signal again, so
//! that the exit status says what ended the run (128 plus the signal, to a shell). A second
//! signal does that at once, without the save, after putting the terminal back: it is what a
//! second Ctrl-C means.

use std::mem::zeroed;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

static CAUGHT: AtomicBool = AtomicBool::new(false);
/// The run has begun, so a signal is a flag and the files are saved; before that it ends the
/// process.
static RUNNING: AtomicBool = AtomicBool::new(false);
/// The first signal caught, for `die_if_caught`; 0 until one is.
static FIRST: AtomicI32 = AtomicI32::new(0);
/// The ends of the pipe the handler writes to, or -1 before `catch`.
static WAKE_READ: AtomicI32 = AtomicI32::new(-1);
static WAKE_WRITE: AtomicI32 = AtomicI32::new(-1);

const ENDING: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

extern "C" fn on_signal(signal: libc::c_int) {
    // The signal is recorded before the flag is set, so that whoever sees the flag finds it.
    // Nothing is waited for if the run has not begun, or if this is the second one.
    let first = FIRST
        .compare_exchange(0, signal, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok();
    if !first || !RUNNING.load(Ordering::Acquire) {
        // The terminal is put back first, as it may be raw by now.
        crate::terminal::restore();
        die(signal);
    }
    CAUGHT.store(true, Ordering::Release);
    // SAFETY: write is async-signal-safe, and errno is put back as the interrupted code had it.
    unsafe {
        let errno = *libc::__errno_location();
        // A full pipe already says a signal has come, so the result does not matter.
        libc::write(WAKE_WRITE.load(Ordering::Relaxed), [1u8].as_ptr().cast(), 1);
        *libc::__errno_location() = errno;
    }
}

/// Back to the default action for `signal`, and raise it. Both calls are safe in a handler.
fn die(signal: libc::c_int) {
    // SAFETY: signal and raise only take the signal number, and are async-signal-safe.
    unsafe {
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
    }
}

/// Has the run been asked to end by a signal?
pub fn caught() -> bool {
    CAUGHT.load(Ordering::Acquire)
}

/// A descriptor that is readable once a signal has come, for `poll` to wait on beside whatever
/// else it waits for, or -1 (which `poll` skips) before `catch`.
pub fn wake_fd() -> libc::c_int {
    WAKE_READ.load(Ordering::Relaxed)
}

/// Blocks the signals that end a run in this thread while `f` runs, and puts the mask back.
fn with_mask<T>(f: impl FnOnce() -> T) -> T {
    // SAFETY: the sets are plain data filled in by sigemptyset and sigaddset.
    unsafe {
        let mut set: libc::sigset_t = zeroed();
        let mut old: libc::sigset_t = zeroed();
        libc::sigemptyset(&mut set);
        for signal in ENDING {
            libc::sigaddset(&mut set, signal);
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, &mut old);
        let result = f();
        libc::pthread_sigmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
        result
    }
}

/// Runs `f`, which makes threads, with the signals blocked, so that the threads it makes have
/// them blocked and they are taken by the thread that calls this, the main one. One that comes
/// meanwhile waits, and is taken when `f` is done.
pub fn with_blocked<T>(f: impl FnOnce() -> T) -> T {
    with_mask(f)
}

/// SIGINT, SIGTERM and SIGHUP end the process, or, once `begin_run` has been called, set the
/// flag `caught` reads. Those that were ignored stay so.
pub fn catch() {
    let mut pipe = [-1; 2];
    // SAFETY: pipe2 fills in two descriptors.
    if unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } == 0 {
        WAKE_READ.store(pipe[0], Ordering::Relaxed);
        WAKE_WRITE.store(pipe[1], Ordering::Relaxed);
    }
    for signal in ENDING {
        // SAFETY: sigaction is plain data, filled in before it is passed on. The handler only
        // touches atomics and calls async-signal-safe functions.
        unsafe {
            let mut old: libc::sigaction = zeroed();
            if libc::sigaction(signal, std::ptr::null(), &mut old) == 0
                && old.sa_sigaction == libc::SIG_IGN
            {
                continue;
            }
            let mut action: libc::sigaction = zeroed();
            action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
    }
}

/// The run is about to start: from here a signal is a flag, and what is written is saved.
pub fn begin_run() {
    RUNNING.store(true, Ordering::Release);
}

/// The first signal that was caught, or 0 if none has been, for saying what stopped the run.
// Nothing outside the tests says what stopped the run yet; the `expect` goes when something does.
#[cfg_attr(not(test), expect(dead_code))]
pub fn first() -> libc::c_int {
    FIRST.load(Ordering::Acquire)
}

/// Ends the process with the signal that ended the run, if one did. Call it when the files are
/// written and the output is flushed.
pub fn die_if_caught() {
    let first = FIRST.load(Ordering::Acquire);
    if first != 0 {
        die(first);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_support::{Ended, close_pty, in_own_process, lflag, pty};

    // What main does: handlers in, and the run begun.
    fn start() {
        catch();
        begin_run();
    }

    fn raise(signal: libc::c_int) {
        // SAFETY: raise only sends this thread the signal.
        unsafe { libc::raise(signal) };
    }

    // Runs `body` in a process of its own (see `in_own_process`) for the test of this name, and
    // returns how it ended. A signal or a flag set here would be the whole test process's.
    fn in_child(test: &str, body: impl FnOnce()) -> Ended {
        in_child_case(test, "", body).unwrap()
    }

    // As `in_child` for a test that tries more than one case, `None` in a child that is for
    // another one (see `in_own_process`).
    fn in_child_case(test: &str, case: &str, body: impl FnOnce()) -> Option<Ended> {
        in_own_process(&format!("signals::tests::{test}"), case, &[], |_| body())
            .map(|ran| ran.ended)
    }

    #[test]
    fn a_signal_sets_the_flag_and_does_not_end_the_process() {
        for signal in ENDING {
            let Some(ended) = in_child_case(
                "a_signal_sets_the_flag_and_does_not_end_the_process",
                &signal.to_string(),
                || {
                    start();
                    raise(signal);
                    if !caught() {
                        // SAFETY: _exit only ends the child.
                        unsafe { libc::_exit(3) };
                    }
                },
            ) else {
                continue;
            };
            assert_eq!(ended, Ended::Exit(0), "signal {signal}");
        }
    }

    #[test]
    fn die_if_caught_ends_the_process_with_the_signal() {
        for signal in ENDING {
            let Some(ended) = in_child_case(
                "die_if_caught_ends_the_process_with_the_signal",
                &signal.to_string(),
                || {
                    start();
                    raise(signal);
                    die_if_caught();
                },
            ) else {
                continue;
            };
            assert_eq!(ended, Ended::Signal(signal));
        }
    }

    #[test]
    fn die_if_caught_does_nothing_when_nothing_was_caught() {
        assert_eq!(
            in_child(
                "die_if_caught_does_nothing_when_nothing_was_caught",
                die_if_caught
            ),
            Ended::Exit(0)
        );
    }

    #[test]
    fn a_second_signal_ends_the_process_at_once() {
        let ended = in_child("a_second_signal_ends_the_process_at_once", || {
            start();
            raise(libc::SIGTERM);
            raise(libc::SIGTERM);
        });
        assert_eq!(ended, Ended::Signal(libc::SIGTERM));
    }

    #[test]
    fn a_signal_that_was_ignored_stays_ignored() {
        let ended = in_child("a_signal_that_was_ignored_stays_ignored", || {
            // SAFETY: signal with SIG_IGN has no handler to get wrong.
            unsafe { libc::signal(libc::SIGHUP, libc::SIG_IGN) };
            start();
            raise(libc::SIGHUP);
            if caught() {
                // SAFETY: _exit only ends the child.
                unsafe { libc::_exit(3) };
            }
        });
        assert_eq!(ended, Ended::Exit(0));
    }

    #[test]
    fn first_is_0_until_a_signal_is_caught_and_then_the_first_one() {
        let ended = in_child(
            "first_is_0_until_a_signal_is_caught_and_then_the_first_one",
            || {
                start();
                let before = first();
                raise(libc::SIGTERM);
                let right = before == 0 && first() == libc::SIGTERM;
                // SAFETY: _exit only ends the child.
                unsafe { libc::_exit(if right { 0 } else { 3 }) };
            },
        );
        assert_eq!(ended, Ended::Exit(0));
    }

    #[test]
    fn the_signal_is_recorded_and_the_wake_pipe_is_readable_once_it_is_caught() {
        let ended = in_child(
            "the_signal_is_recorded_and_the_wake_pipe_is_readable_once_it_is_caught",
            || {
                start();
                let mut poll = libc::pollfd {
                    fd: wake_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                // SAFETY: poll on one live pollfd, with no wait.
                let before = unsafe { libc::poll(&mut poll, 1, 0) };
                raise(libc::SIGHUP);
                // SAFETY: as above.
                let after = unsafe { libc::poll(&mut poll, 1, 0) };
                let first = FIRST.load(Ordering::Acquire);
                let right = before == 0 && after == 1 && caught() && first == libc::SIGHUP;
                // SAFETY: _exit only ends the child.
                unsafe { libc::_exit(if right { 0 } else { 3 }) };
            },
        );
        assert_eq!(ended, Ended::Exit(0));
    }

    #[test]
    fn a_signal_before_the_run_begins_ends_the_process_at_once() {
        for signal in ENDING {
            let Some(ended) = in_child_case(
                "a_signal_before_the_run_begins_ends_the_process_at_once",
                &signal.to_string(),
                || {
                    catch();
                    raise(signal);
                },
            ) else {
                continue;
            };
            assert_eq!(ended, Ended::Signal(signal));
        }
    }

    #[test]
    fn a_signal_while_threads_are_made_waits_and_is_taken_after() {
        let ended = in_child(
            "a_signal_while_threads_are_made_waits_and_is_taken_after",
            || {
                start();
                let waited = with_blocked(|| {
                    raise(libc::SIGTERM);
                    !caught()
                });
                let handled = caught();
                // SAFETY: _exit only ends the child.
                unsafe { libc::_exit(if waited && handled { 0 } else { 3 }) };
            },
        );
        assert_eq!(ended, Ended::Exit(0));
    }

    #[test]
    fn a_second_signal_puts_the_terminal_back_before_it_ends_the_process() {
        let pty = pty();
        let slave = pty.1;
        let cooked = lflag(slave);
        // The child has the slave open too (`slave` is its number there as well, which `fds`
        // says), and raw mode is made there, so that this process's own is not touched.
        let ended = in_own_process(
            "signals::tests::a_second_signal_puts_the_terminal_back_before_it_ends_the_process",
            "",
            &[slave],
            |fds| {
                let raw = crate::terminal::RawMode::enable_on(fds[0]).expect("a pty is a terminal");
                // The drop that would put it back on the way out never runs.
                std::mem::forget(raw);
                start();
                raise(libc::SIGTERM);
                raise(libc::SIGTERM);
            },
        )
        .unwrap()
        .ended;
        assert_eq!(ended, Ended::Signal(libc::SIGTERM));
        assert_eq!(lflag(slave), cooked);
        close_pty(pty);
    }
}
