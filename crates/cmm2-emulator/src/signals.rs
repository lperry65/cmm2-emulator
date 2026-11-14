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

    #[derive(Debug, PartialEq, Eq)]
    enum Ended {
        Signal(i32),
        Exit(i32),
    }

    // Runs `body` in a forked child and returns how it ended. The child only calls what is safe
    // after a fork from a threaded process, and `_exit`s.
    fn in_child(body: impl FnOnce()) -> Ended {
        // SAFETY: fork; the child runs `body` (signal calls and atomics only) and _exits.
        unsafe {
            let pid = libc::fork();
            if pid == 0 {
                body();
                libc::_exit(0);
            }
            let mut status = 0;
            libc::waitpid(pid, &mut status, 0);
            if libc::WIFSIGNALED(status) {
                Ended::Signal(libc::WTERMSIG(status))
            } else {
                Ended::Exit(libc::WEXITSTATUS(status))
            }
        }
    }

    // What main does: handlers in, and the run begun.
    fn start() {
        catch();
        begin_run();
    }

    fn raise(signal: libc::c_int) {
        // SAFETY: raise only sends this thread the signal.
        unsafe { libc::raise(signal) };
    }

    #[test]
    fn a_signal_sets_the_flag_and_does_not_end_the_process() {
        for signal in ENDING {
            let ended = in_child(|| {
                start();
                raise(signal);
                if !caught() {
                    // SAFETY: _exit only ends the child.
                    unsafe { libc::_exit(3) };
                }
            });
            assert_eq!(ended, Ended::Exit(0), "signal {signal}");
        }
    }

    #[test]
    fn die_if_caught_ends_the_process_with_the_signal() {
        for signal in ENDING {
            let ended = in_child(|| {
                start();
                raise(signal);
                die_if_caught();
            });
            assert_eq!(ended, Ended::Signal(signal));
        }
    }

    #[test]
    fn die_if_caught_does_nothing_when_nothing_was_caught() {
        assert_eq!(in_child(die_if_caught), Ended::Exit(0));
    }

    #[test]
    fn a_second_signal_ends_the_process_at_once() {
        let ended = in_child(|| {
            start();
            raise(libc::SIGTERM);
            raise(libc::SIGTERM);
        });
        assert_eq!(ended, Ended::Signal(libc::SIGTERM));
    }

    #[test]
    fn a_signal_that_was_ignored_stays_ignored() {
        let ended = in_child(|| {
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
    fn the_signal_is_recorded_and_the_wake_pipe_is_readable_once_it_is_caught() {
        let ended = in_child(|| {
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
        });
        assert_eq!(ended, Ended::Exit(0));
    }

    #[test]
    fn a_signal_before_the_run_begins_ends_the_process_at_once() {
        for signal in ENDING {
            let ended = in_child(|| {
                catch();
                raise(signal);
            });
            assert_eq!(ended, Ended::Signal(signal));
        }
    }

    #[test]
    fn a_signal_while_threads_are_made_waits_and_is_taken_after() {
        let ended = in_child(|| {
            start();
            let waited = with_blocked(|| {
                raise(libc::SIGTERM);
                !caught()
            });
            let handled = caught();
            // SAFETY: _exit only ends the child.
            unsafe { libc::_exit(if waited && handled { 0 } else { 3 }) };
        });
        assert_eq!(ended, Ended::Exit(0));
    }

    // The local mode flags of the terminal on `fd`.
    fn lflag(fd: libc::c_int) -> libc::tcflag_t {
        // SAFETY: tcgetattr fills a plain termios.
        unsafe {
            let mut termios: libc::termios = zeroed();
            assert_eq!(libc::tcgetattr(fd, &mut termios), 0);
            termios.c_lflag
        }
    }

    #[test]
    fn a_second_signal_puts_the_terminal_back_before_it_ends_the_process() {
        let (mut master, mut slave) = (0, 0);
        // SAFETY: openpty fills in two descriptors.
        let opened = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        assert_eq!(opened, 0);
        let cooked = lflag(slave);
        let ended = in_child(|| {
            let raw = crate::terminal::RawMode::enable_on(slave).expect("a pty is a terminal");
            // The drop that would put it back on the way out never runs.
            std::mem::forget(raw);
            start();
            raise(libc::SIGTERM);
            raise(libc::SIGTERM);
        });
        assert_eq!(ended, Ended::Signal(libc::SIGTERM));
        assert_eq!(lflag(slave), cooked);
        // SAFETY: closing the two descriptors made above.
        unsafe {
            libc::close(master);
            libc::close(slave);
        }
    }
}
