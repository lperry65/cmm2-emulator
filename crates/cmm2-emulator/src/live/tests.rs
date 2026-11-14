use super::*;

#[test]
fn the_quit_key_and_what_follows_it_are_not_typed() {
    assert_eq!(until_quit(b"ab"), (&b"ab"[..], false));
    assert_eq!(until_quit(b"a\x1Db"), (&b"a"[..], true));
    assert_eq!(until_quit(b"\x1D"), (&b""[..], true));
}
