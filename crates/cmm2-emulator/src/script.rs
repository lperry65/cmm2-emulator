//! Scripted keys for the console: send some keys once the guest has printed some text.

use crate::args::OnRule;

/// How much output is kept for matching.
const KEPT: usize = 4096;

pub struct Script {
    pending: Vec<OnRule>,
    seen: Vec<u8>,
}

impl Script {
    pub fn new(rules: Vec<OnRule>) -> Script {
        Script {
            pending: rules,
            seen: Vec::new(),
        }
    }

    /// Adds the guest's latest output and returns the keys to send now.
    pub fn feed(&mut self, output: &[u8]) -> Vec<u8> {
        self.seen.extend_from_slice(output);
        let excess = self.seen.len().saturating_sub(KEPT);
        self.seen.drain(..excess);

        let mut keys = Vec::new();
        let seen = &mut self.seen;
        self.pending.retain(|rule| {
            let Some(at) = find(seen, rule.text.as_bytes()) else {
                return true;
            };
            // Consumed, so a later rule with the same text needs a new copy of it.
            seen.drain(at..at + rule.text.len());
            keys.extend_from_slice(&rule.keys);
            false
        });
        keys
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(text: &str, keys: &str) -> OnRule {
        OnRule {
            text: text.into(),
            keys: keys.into(),
        }
    }

    #[test]
    fn a_rule_fires_once_when_its_text_appears() {
        let mut script = Script::new(vec![rule("ready", "go")]);
        assert_eq!(script.feed(b"boot... ready\r\n"), b"go");
        assert_eq!(script.feed(b"ready again"), b"");
    }

    #[test]
    fn text_split_across_two_feeds_still_matches() {
        let mut script = Script::new(vec![rule("Keyboard type", "1")]);
        assert_eq!(script.feed(b"Keyboard ty"), b"");
        assert_eq!(script.feed(b"pe: 1=UK"), b"1");
    }

    #[test]
    fn the_matched_text_is_consumed() {
        let mut script = Script::new(vec![rule("ab", "1"), rule("ab", "2")]);
        // One copy of the text satisfies one rule; the second needs a new copy.
        assert_eq!(script.feed(b"ab"), b"1");
        assert_eq!(script.feed(b"x"), b"");
        assert_eq!(script.feed(b"ab"), b"2");
        assert_eq!(script.feed(b"ab"), b"");
    }

    #[test]
    fn rules_fire_in_the_order_given() {
        let mut script = Script::new(vec![rule("two", "2"), rule("one", "1")]);
        assert_eq!(script.feed(b"one two"), b"21");
    }

    #[test]
    fn a_rule_waits_for_its_own_text() {
        let mut script = Script::new(vec![rule("two", "2"), rule("one", "1")]);
        assert_eq!(script.feed(b"one"), b"1");
        assert_eq!(script.feed(b"two"), b"2");
    }

    #[test]
    fn two_rules_can_fire_on_one_feed() {
        let mut script = Script::new(vec![rule("one", "1"), rule("two", "2")]);
        assert_eq!(script.feed(b"one then two"), b"12");
    }

    #[test]
    fn output_that_does_not_match_returns_nothing() {
        let mut script = Script::new(vec![rule("ready", "go")]);
        assert_eq!(script.feed(b"read"), b"");
        assert_eq!(script.feed(b"y"), b"go");
        assert_eq!(Script::new(vec![]).feed(b"anything"), b"");
    }

    #[test]
    fn matching_still_works_after_more_than_the_cap_of_output() {
        let mut script = Script::new(vec![rule("end", "!")]);
        assert_eq!(script.feed(&vec![b'x'; 3 * KEPT]), b"");
        assert_eq!(script.feed(b"en"), b"");
        assert_eq!(script.feed(b"d"), b"!");
    }

    #[test]
    fn only_the_last_part_of_the_output_is_kept() {
        let mut script = Script::new(vec![rule("old", "!")]);
        let mut output = b"old".to_vec();
        output.extend(vec![b'x'; KEPT]);
        assert_eq!(script.feed(&output), b"");
        assert_eq!(script.seen.len(), KEPT);
    }
}
