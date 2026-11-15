use super::*;

#[test]
fn a_failure_of_the_run_comes_before_those_of_the_trace_and_the_save() {
    let _cursor = CURSOR_TEST.lock().unwrap_or_else(|held| held.into_inner());
    take_cursor();
    let err = |text: &str| RunError::Fault(text.to_string());
    let mut said = Vec::new();
    let last = settle(
        Err(err("fault")),
        [Err(err("trace")), Err(err("shot")), Err(err("save"))],
        &mut said,
    )
    .unwrap_err();
    // The last is returned for the caller to say; the others were said, in order, before it.
    assert_eq!(last.to_string(), "save");
    assert_eq!(
        String::from_utf8(said).unwrap(),
        "cmm2-emulator: fault\ncmm2-emulator: trace\ncmm2-emulator: shot\n"
    );

    // With no fault the order is the trace, the screenshot, the save.
    let mut said = Vec::new();
    let last = settle(
        Ok(true),
        [Err(err("trace")), Ok(()), Err(err("save"))],
        &mut said,
    )
    .unwrap_err();
    assert_eq!(last.to_string(), "save");
    assert_eq!(String::from_utf8(said).unwrap(), "cmm2-emulator: trace\n");

    // One error alone is only returned, and none says nothing.
    let mut said = Vec::new();
    let last = settle(Err(err("fault")), [Ok(()), Ok(()), Ok(())], &mut said).unwrap_err();
    assert_eq!(last.to_string(), "fault");
    let last = settle(Ok(true), [Ok(()), Ok(()), Err(err("save"))], &mut said).unwrap_err();
    assert_eq!(last.to_string(), "save");
    assert!(matches!(
        settle(Ok(false), [Ok(()), Ok(()), Ok(())], &mut said),
        Ok(false)
    ));
    assert!(said.is_empty());
}

const CLEAN: Cursor = Cursor {
    away: false,
    written: false,
    escape: Escape::None,
};

#[test]
fn a_line_said_to_a_raw_terminal_ends_in_cr_lf() {
    assert_eq!(said("x", false, CLEAN), "cmm2-emulator: x\n");
    assert_eq!(said("x", true, CLEAN), "cmm2-emulator: x\r\n");
}

#[test]
fn a_line_said_part_way_along_a_line_starts_a_new_one() {
    let mid = CLEAN.after(b"b", false);
    assert_eq!(said("x", false, mid), "\ncmm2-emulator: x\n");
    assert_eq!(said("x", true, mid), "\r\ncmm2-emulator: x\r\n");
}

#[test]
fn the_cursor_is_followed_through_what_the_guest_wrote() {
    // A line ended in CR LF leaves it clean, and a lone CR after that leaves it clean too.
    assert_eq!(CLEAN.after(b"ready\r\n", false), CLEAN);
    assert_eq!(CLEAN.after(b"ready\r\n\r", false), CLEAN);
    // A lone CR after text is back at the start of a row that has text on it.
    assert!(CLEAN.after(b"abc\r", false).written);
    // A raw terminal's lone LF moves down and not to the first column; a cooked one's does.
    let lone = CLEAN.after(b"b\n", false);
    assert_eq!((lone.away, lone.written), (true, false));
    assert_eq!(said("x", true, lone), "\rcmm2-emulator: x\r\n");
    assert_eq!(CLEAN.after(b"b\n", true), CLEAN);
}

#[test]
fn a_colour_change_or_a_title_leaves_the_cursor_where_it_was() {
    assert_eq!(CLEAN.after(b"ok\r\n\x1b[0m", false), CLEAN);
    assert_eq!(CLEAN.after(b"ok\r\n\x1b[1;31m", false), CLEAN);
    assert_eq!(CLEAN.after(b"ok\r\n\x1b]0;title\x07", false), CLEAN);
    assert_eq!(CLEAN.after(b"ok\r\n\x1b]0;title\x1b\\", false), CLEAN);
    // The same in pieces, as the guest's output comes in as it is written.
    let part = CLEAN.after(b"\x1b[1", false).after(b";3", false);
    assert_eq!(part.after(b"1m", false), CLEAN);
}

#[test]
fn an_escape_that_may_move_the_cursor_counts_as_a_row_with_something_on_it() {
    for moved in [&b"\x1b[2J"[..], b"\x1b[10;5H", b"\x1bc", b"\x1b7"] {
        assert!(CLEAN.after(moved, false).written, "{moved:?}");
    }
    // Text after a colour change is text.
    assert!(CLEAN.after(b"\x1b[0mx", false).written);
}

#[test]
fn the_cursor_goes_through_its_bits_and_back() {
    for escape in [
        Escape::None,
        Escape::Seen,
        Escape::Control,
        Escape::Command,
        Escape::CommandEnd,
    ] {
        for (away, written) in [(false, false), (true, false), (false, true), (true, true)] {
            let cursor = Cursor {
                away,
                written,
                escape,
            };
            assert_eq!(Cursor::from_bits(cursor.bits()), cursor);
        }
    }
}

#[test]
fn a_line_said_inside_an_escape_sequence_cancels_it_first() {
    for open in [&b"\x1b"[..], b"\x1b[3", b"\x1b]0;t", b"\x1b]0;t\x1b"] {
        let cursor = CLEAN.after(open, false);
        assert_eq!(
            said("x", true, cursor),
            "\x18\r\ncmm2-emulator: x\r\n",
            "{open:?}"
        );
        assert_eq!(
            said("x", false, cursor),
            "\x18\ncmm2-emulator: x\n",
            "{open:?}"
        );
    }
}

#[test]
fn cancel_and_substitute_end_an_operating_system_command() {
    // What follows them is shown, so it is text.
    for cancel in [0x18, 0x1A] {
        let ended = CLEAN.after(&[0x1B, b']', b't', cancel], false);
        assert_eq!(ended, CLEAN);
        assert!(ended.after(b"a", false).written);
    }
    let ended = CLEAN.after(b"\x1b]t\x1b\x18", false);
    assert_eq!(ended, CLEAN);
}

#[test]
fn an_escape_in_a_command_that_is_not_its_end_is_an_escape_of_its_own() {
    // ESC \ ends it and moves nothing; ESC E does something else after ending it.
    assert_eq!(CLEAN.after(b"\x1b]t\x1b\\", false), CLEAN);
    assert!(CLEAN.after(b"\x1b]t\x1bE", false).written);
    // And ESC [ starts a control sequence, which a colour change then ends.
    assert_eq!(CLEAN.after(b"\x1b]t\x1b[0m", false), CLEAN);
}

#[test]
fn a_write_cut_short_leaves_nothing_known() {
    let _cursor = CURSOR_TEST.lock().unwrap_or_else(|held| held.into_inner());
    note_cut_short();
    assert_eq!(said("x", false, take_cursor()), "\x18\ncmm2-emulator: x\n");
    // It was used up.
    assert_eq!(take_cursor(), CLEAN);
}
