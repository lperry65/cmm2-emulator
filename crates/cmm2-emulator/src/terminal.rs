//! The host terminal in raw mode, so keys reach the guest one at a time and unechoed.

use std::cell::UnsafeCell;
use std::mem::{MaybeUninit, zeroed};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

const STDIN: libc::c_int = 0;

/// The terminal as it was before raw mode, and the descriptor it was on, while raw mode is on.
/// It is here, and not only in `RawMode`, so that the panic hook and the signal handler can put
/// the terminal back too, which they can only do if reading it takes no lock: it is written
/// once, before `RAW` is set, and read only after `RAW` is seen set.
struct Saved(UnsafeCell<MaybeUninit<libc::termios>>);

// SAFETY: written only by `enable_on` before `RAW` is set (and by nothing else while it is),
// and read only after `RAW` has been seen set, by whichever of the handler and the main thread
// restores first; both read the same bytes.
unsafe impl Sync for Saved {}

static SAVED: Saved = Saved(UnsafeCell::new(MaybeUninit::uninit()));
static SAVED_FD: AtomicI32 = AtomicI32::new(-1);
static RAW: AtomicBool = AtomicBool::new(false);

/// Puts the terminal back as it was when dropped.
pub struct RawMode;

impl RawMode {
    /// Raw mode when stdin is a terminal; `None` when it is a pipe or a file.
    pub fn enable() -> Option<RawMode> {
        RawMode::enable_on(STDIN)
    }

    pub(crate) fn enable_on(fd: libc::c_int) -> Option<RawMode> {
        // SAFETY: isatty only looks at the descriptor.
        if unsafe { libc::isatty(fd) } != 1 {
            return None;
        }
        // SAFETY: termios is plain data, and tcgetattr fills it in.
        let mut saved: libc::termios = unsafe { zeroed() };
        // SAFETY: the pointer is to a live termios.
        if unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
            return None;
        }
        let mut raw = saved;
        // SAFETY: the pointer is to a live termios.
        unsafe { libc::cfmakeraw(&mut raw) };
        // The copy goes in first, so that no moment has raw mode on and nothing to restore.
        // SAFETY: RAW is not set, so nothing reads SAVED, and this is the only writer.
        unsafe { (*SAVED.0.get()).write(saved) };
        SAVED_FD.store(fd, Ordering::Relaxed);
        RAW.store(true, Ordering::Release);
        // SAFETY: the pointer is to a live termios.
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            RAW.store(false, Ordering::Release);
            return None;
        }
        Some(RawMode)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        restore();
    }
}

/// Puts the terminal back if raw mode is on. Doing it twice is harmless. It takes no lock and
/// calls only tcsetattr, so a signal handler can use it. `RAW` is cleared after the terminal is
/// back, not before, so that a signal that comes in between still finds it set and puts the
/// terminal back itself (setting it twice does no harm).
pub fn restore() {
    if RAW.load(Ordering::Acquire) {
        // SAFETY: RAW was set, so SAVED holds the termios that enable_on wrote, and nothing
        // writes it again while RAW is set. tcsetattr is async-signal-safe.
        unsafe {
            libc::tcsetattr(
                SAVED_FD.load(Ordering::Relaxed),
                libc::TCSANOW,
                (*SAVED.0.get()).as_ptr(),
            );
        }
        RAW.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::IsTerminal;

    #[test]
    fn there_is_no_raw_mode_when_stdin_is_not_a_terminal() {
        // Under a terminal, `cargo test` hands its stdin on, and enabling would change it.
        if !std::io::stdin().is_terminal() {
            assert!(RawMode::enable().is_none());
        }
    }
}
