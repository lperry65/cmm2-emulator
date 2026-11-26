//! The refusal a person sees when the command line cannot be read: every arm of `ArgError` that
//! echoes an argument, driven through the real binary. The unit fence over the same arms is in
//! `crates/cmm2-emulator/src/args.rs`, and this is the other side of it: what the process writes,
//! where a raw escape byte and a broken line were measured before the echo was escaped.

mod common;

use common::*;

// An argument as it was typed, holding what a terminal acts on: a line feed and a colour change.
const HOSTILE: &str = "evil\n\x1b[31mRED\x1b[0m";

#[test]
fn every_argument_error_that_echoes_what_was_typed_says_it_escaped_in_two_lines() {
    // The keys of `--on` are read for escapes of their own, so the argument that reaches `BadKeys`
    // ends in one that has none (`\q`): the arm echoes the whole argument it was handed, hostile
    // bytes and all. Each message line is written out here by hand, with the argument's line feed
    // as `\n`, its escape as `\x1B` and the backslash of that `\q` doubled once, rather than built
    // with the crate's escaping function, so that a broken one cannot agree with itself.
    let unknown = format!("--{HOSTILE}");
    let keys = format!("{HOSTILE}\\q");
    let cases: [(&[&str], &str); 5] = [
        (
            &["--run", HOSTILE],
            r"cmm2-emulator: bad count 'evil\n\x1B[31mRED\x1B[0m'",
        ),
        (
            &["--rng-seed", HOSTILE],
            r"cmm2-emulator: bad seed 'evil\n\x1B[31mRED\x1B[0m'",
        ),
        (
            &["--lookup", HOSTILE],
            r"cmm2-emulator: bad address 'evil\n\x1B[31mRED\x1B[0m'",
        ),
        (
            &[unknown.as_str()],
            r"cmm2-emulator: unknown argument '--evil\n\x1B[31mRED\x1B[0m'",
        ),
        (
            &["--on", "x", keys.as_str()],
            r"cmm2-emulator: bad keys 'evil\n\x1B[31mRED\x1B[0m\\q'",
        ),
    ];
    for (args, message) in cases {
        let output = emulator()
            .args(args)
            .output()
            .expect("failed to run cmm2-emulator");
        let err = stderr(&output);

        assert_eq!(output.status.code(), Some(2), "{args:?}: {err:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        // One line feed ends the message line and one ends the usage line the error brings with
        // it: an argument error is two lines, and this echo adds none of its own. Said twice,
        // because the count alone cannot see a carriage return on the same line.
        assert_eq!(err.matches('\n').count(), 2, "{args:?}: {err:?}");
        let mut lines = err.lines();
        assert_eq!(
            lines.next(),
            Some(message),
            "{args:?} did not echo the argument"
        );
        // The usage line is not this test's to pin (`tests/usage.rs` holds its text); what is
        // pinned here is that the echo added no byte to the message line and no line after it.
        assert!(
            lines
                .next()
                .is_some_and(|line| line.starts_with("usage: cmm2-emulator")),
            "{args:?} did not end with the usage line: {err:?}"
        );
        assert_eq!(
            lines.next(),
            None,
            "{args:?} said more than two lines: {err:?}"
        );
        // Said per line as well, because the count above cannot see a bare escape that leaves the
        // line and its feeds alone.
        for line in err.lines() {
            assert!(
                !line.chars().any(char::is_control),
                "{args:?} put a control character on a line: {err:?}"
            );
        }
    }
}

// A name inside those quotes must not read as ended early: a `'` in it is doubled, so the last
// quote on the line is the one that ends the name. Before that was so, `-x' --burn evil` was
// measured on the binary as `unknown argument '-x' --burn evil'`, where the name reads as ended at
// `-x`. The keys of `--on` must hold an escape the parser refuses, so that case also ends in `\z`,
// which the message says as the two characters of a backslash with `shown`'s own doubling.
#[test]
fn every_argument_error_that_echoes_a_quote_says_it_doubled() {
    let cases: [(&[&str], &str); 6] = [
        (&["--run", "1'000"], r"cmm2-emulator: bad count '1''000'"),
        (&["--rng-seed", "''"], r"cmm2-emulator: bad seed ''''''"),
        (&["--lookup", "0x1'"], r"cmm2-emulator: bad address '0x1'''"),
        (
            &["-x' --burn evil"],
            r"cmm2-emulator: unknown argument '-x'' --burn evil'",
        ),
        (
            &["--on", "x", "it's\\z"],
            r"cmm2-emulator: bad keys 'it''s\\z'",
        ),
        // A control character and a quote together: escaped once, doubled once.
        (
            &["--lookup", "evil\n\x1b[31m'"],
            r"cmm2-emulator: bad address 'evil\n\x1B[31m'''",
        ),
    ];
    for (args, message) in cases {
        let output = emulator()
            .args(args)
            .output()
            .expect("failed to run cmm2-emulator");
        let err = stderr(&output);

        assert_eq!(output.status.code(), Some(2), "{args:?}: {err:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert_eq!(err.matches('\n').count(), 2, "{args:?}: {err:?}");
        let mut lines = err.lines();
        assert_eq!(
            lines.next(),
            Some(message),
            "{args:?} did not quote the argument"
        );
        assert!(
            lines
                .next()
                .is_some_and(|line| line.starts_with("usage: cmm2-emulator")),
            "{args:?} did not end with the usage line: {err:?}"
        );
        assert_eq!(
            lines.next(),
            None,
            "{args:?} said more than two lines: {err:?}"
        );
        for line in err.lines() {
            assert!(
                !line.chars().any(char::is_control),
                "{args:?} put a control character on a line: {err:?}"
            );
        }
    }
}
