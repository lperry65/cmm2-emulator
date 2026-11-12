//! The host terminal in raw mode, so keys reach the guest one at a time and unechoed.

use std::mem::zeroed;

const STDIN: libc::c_int = 0;

/// Puts the terminal back as it was when dropped.
pub struct RawMode {
    saved: libc::termios,
}

impl RawMode {
    /// Raw mode when stdin is a terminal; `None` when it is a pipe or a file.
    pub fn enable() -> Option<RawMode> {
        // SAFETY: isatty only looks at the descriptor.
        if unsafe { libc::isatty(STDIN) } != 1 {
            return None;
        }
        // SAFETY: termios is plain data, and tcgetattr fills it in.
        let mut saved: libc::termios = unsafe { zeroed() };
        // SAFETY: the pointer is to a live termios.
        if unsafe { libc::tcgetattr(STDIN, &mut saved) } != 0 {
            return None;
        }
        let mut raw = saved;
        // SAFETY: the pointer is to a live termios.
        unsafe { libc::cfmakeraw(&mut raw) };
        // SAFETY: the pointer is to a live termios.
        if unsafe { libc::tcsetattr(STDIN, libc::TCSANOW, &raw) } != 0 {
            return None;
        }
        Some(RawMode { saved })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        // SAFETY: the pointer is to the termios saved by enable.
        unsafe { libc::tcsetattr(STDIN, libc::TCSANOW, &self.saved) };
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
