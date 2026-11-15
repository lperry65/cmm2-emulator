use super::*;

#[test]
fn the_quit_key_and_what_follows_it_are_not_typed() {
    assert_eq!(until_quit(b"ab"), (&b"ab"[..], false));
    assert_eq!(until_quit(b"a\x1Db"), (&b"a"[..], true));
    assert_eq!(until_quit(b"\x1D"), (&b""[..], true));
}

#[test]
fn each_end_says_why() {
    assert_eq!(End::QuitKey.to_string(), "quit key (Ctrl-]) pressed");
    assert_eq!(End::Signal(libc::SIGTERM).to_string(), "stopped by SIGTERM");
    assert_eq!(
        End::Address(0x0800_0104).to_string(),
        "reached address 0x08000104"
    );
    assert_eq!(
        End::InstructionLimit.to_string(),
        "instruction limit reached"
    );
    assert_eq!(End::WindowClosed.to_string(), "window closed");
}

#[test]
fn a_closed_window_ends_the_run_and_an_open_one_does_not() {
    assert!(matches!(window_end(false), Some(End::WindowClosed)));
    assert!(window_end(true).is_none());
}

#[test]
fn the_signals_that_end_a_run_are_named_and_any_other_is_numbered() {
    assert_eq!(signal_name(libc::SIGINT), "SIGINT");
    assert_eq!(signal_name(libc::SIGTERM), "SIGTERM");
    assert_eq!(signal_name(libc::SIGHUP), "SIGHUP");
    assert_eq!(
        signal_name(libc::SIGUSR1),
        format!("signal {}", libc::SIGUSR1)
    );
}

struct Cut;

impl Write for Cut {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("cut short"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn what_the_cursor_is_left_at_is_noted_when_the_output_is_written_and_when_it_is_cut_short() {
    use crate::error::{CURSOR_TEST, Cursor, take_cursor};
    let _cursor = CURSOR_TEST.lock().unwrap_or_else(|held| held.into_inner());
    take_cursor();

    forward(b"ab", &mut Vec::new(), Some(true)).unwrap();
    assert_eq!(take_cursor(), Cursor::default().after(b"ab", true));

    assert!(forward(b"ab", &mut Cut, Some(true)).is_err());
    let cut = take_cursor();
    assert!(cut.written && cut.escape != crate::error::Escape::None);

    // Nothing is noted when the output is not for a terminal, whichever way the write goes.
    forward(b"ab", &mut Vec::new(), None).unwrap();
    assert!(forward(b"ab", &mut Cut, None).is_err());
    assert_eq!(take_cursor(), Cursor::default());
}
