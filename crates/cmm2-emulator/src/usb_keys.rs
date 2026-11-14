//! The window's keys as the USB keyboard's report: which keys are down, in the order they went
//! down, as the 8-byte boot report the firmware polls for. This is the path that gives the
//! firmware held-key state (`KEYDOWN()`); until it starts polling, the window types into the
//! console instead (see `UsbKeys::active`).

use cmm2_core::KeyReport;
use sdl2::keyboard::Scancode;

/// The most keys a boot report has room for.
const SLOTS: usize = 6;

/// The usage the report has in every slot when more keys are down than fit: ErrorRollOver.
const ROLLOVER: u8 = 0x01;

/// The modifier usages, Left Control to Right GUI, are bits 0 to 7 of the report's first byte.
const FIRST_MODIFIER: u8 = 0xE0;
const LAST_MODIFIER: u8 = 0xE7;

/// The USB HID usage of a key, from the keyboard page. SDL numbers its scancodes by these
/// usages, so the letters, digits, punctuation, F1 to F12, the cursor and editing keys and the
/// keypad (usages 0x04 to 0x65) and the eight modifiers (0xE0 to 0xE7) are the same numbers.
/// Anything else is a key the boot report cannot carry: media keys, F13 and up, and so on.
pub fn usage(scancode: Scancode) -> Option<u8> {
    let number = scancode as i32;
    match u8::try_from(number) {
        Ok(usage @ (0x04..=0x65 | FIRST_MODIFIER..=LAST_MODIFIER)) => Some(usage),
        _ => None,
    }
}

fn is_modifier(usage: u8) -> bool {
    (FIRST_MODIFIER..=LAST_MODIFIER).contains(&usage)
}

/// The caps lock and num lock keys' usages, and their bits in the LED byte the firmware sets.
const CAPS_LOCK: u8 = 0x39;
const NUM_LOCK: u8 = 0x53;
const LED_NUM: u8 = 1 << 0;
const LED_CAPS: u8 = 1 << 1;

/// Left Control and Right Alt, which Windows reports together for AltGr.
const LEFT_CTRL: u8 = 0xE0;
const RIGHT_ALT: u8 = 0xE6;

/// How many polls the locks wait for the firmware to show them on the LEDs, before they take it
/// that it is not going to: about 1.2 s of machine time, as the firmware polls every 6 ms with
/// nothing to send. The firmware sets the LEDs about a poll after it starts to poll, and a poll or
/// two after a lock key, so this is long for it and only matters if it never does.
const LED_WAIT_POLLS: u32 = 200;

/// A key that went down, and what is known of it. A key that is let go and was in a report that
/// the firmware has been sent is gone; one that was let go before that stays until it has been
/// sent, so that a tap is never lost.
struct Key {
    usage: u8,
    down: bool,
    /// The modifiers that were down when it went down, as the report's first byte.
    modifiers: u8,
    /// It is in the report that is set.
    shown: bool,
    /// The firmware has been sent a report with it in.
    sent: bool,
    /// The firmware has been sent a report that has it as a key, and not only the error.
    appeared: bool,
    /// It went down while six other keys were down, so the keyboard has no slot for it: the
    /// report is the rollover error until the firmware has been sent that. The firmware ignores
    /// the error and keeps the report it had.
    overflow: bool,
    /// The firmware has been sent the error for it. It is reported as a key, in its turn, if it
    /// is still down when there is room, and lost if it is let go before.
    errored: bool,
    /// It was let go while the keyboard could show reports (not more than six keys down), or the
    /// keyboard could show them from when the error ended, or every key was let go at once: a
    /// report without it would have been sent after it was.
    up_visible: bool,
    /// An errored key has been in a report that was not the error, so a keyboard polled every
    /// millisecond would have sent it: it takes the modifiers of that report, and is kept until
    /// a report with it has been sent.
    placed: bool,
}

/// The keys held and the report they make. A cheap handle on the keyboard's `KeyReport`, which
/// the keyboard sends the firmware when it polls.
///
/// The report is the keys that are down, as on a real keyboard: the modifiers and up to six keys
/// in the order they were pressed, so that a held key is in every report for as long as it is
/// held, and the firmware repeats it and `KEYDOWN()` sees it. The one thing a real keyboard
/// does not do: the emulator is polled more slowly than real time, so a key that is let go
/// before a poll has sent it is kept until one has, with the modifiers that were down when it
/// was pressed, so a tap is never lost and Shift with a letter types the capital however quickly
/// Shift comes up. Keys that cannot share a report wait for the next one: the same key again
/// (it is reported, then let go for a poll, then reported again), a key pressed with other
/// modifiers, and more than six at once.
pub struct UsbKeys {
    report: KeyReport,
    /// In the order they went down. The same key can be here twice, if it was tapped again
    /// before the firmware had the first tap.
    keys: Vec<Key>,
    /// The last report the firmware was sent that was not the rollover error, which it ignores:
    /// the keys it holds.
    reported: [u8; 8],
    /// Keys that were in `reported` and are up while the keyboard could show reports (six keys
    /// down at most): it would have shown one without them, and pressed again they need that
    /// report sent first, or the firmware would see no key up and down again. Keys let go while
    /// the error was the report are added when it stops being.
    up_seen: Vec<u8>,
    /// The locks have been compared with the host's since the firmware started polling.
    locks_followed: bool,
    /// How many LED updates and polls there were when the lock keys now being waited for were
    /// pressed (or handed to the firmware), and how many of them there are: the firmware shows
    /// each with an update of the LEDs. It is kept up to date at every lock key press and when a
    /// lock key is reported, whether or not the locks are being followed.
    lock_keys: Option<LockKeys>,
    /// Whether the host reports AltGr as a Left Control down and a Right Alt down, as Windows does.
    altgr_is_ctrl_alt: bool,
}

struct LockKeys {
    updates: u32,
    polls: u32,
    presses: u32,
    /// `KeyReport::resets` when they were counted: a reset takes the counts back to 0.
    resets: u32,
}

impl UsbKeys {
    pub fn new(report: KeyReport) -> UsbKeys {
        UsbKeys {
            report,
            keys: Vec::new(),
            reported: [0; 8],
            up_seen: Vec::new(),
            locks_followed: false,
            lock_keys: None,
            altgr_is_ctrl_alt: cfg!(windows),
        }
    }

    /// Whether the firmware is polling the keyboard. Until then the keys are not USB's: they go
    /// to the console, which is all that answers the first-boot question and anything else
    /// typed before the USB stack is up. It is the same again after the chip is reset, until
    /// the firmware has started the USB stack once more.
    ///
    /// A key that was pressed before this became true, and is still down, was typed on the
    /// console, and is not in the report: it is typed once, and does not turn up again as a key
    /// down at the switch. It counts again from its next press. The modifiers are kept all the
    /// while, so Shift held across the switch still shifts the next key.
    pub fn active(&self) -> bool {
        self.report.polled()
    }

    /// A key went down. Pressing a key that is down does nothing: the firmware repeats the key
    /// itself, and SDL's repeats must not reach it.
    ///
    /// Windows reports AltGr as a Left Control down and, straight after it, a Right Alt down. The
    /// window handles them in one batch, so the Control has not been sent when the Alt comes, and
    /// is taken back as if it had not been pressed. A Control that the firmware has had, or that
    /// was not the last key down, is a real one.
    pub fn press(&mut self, usage: u8) {
        self.settle();
        if usage == RIGHT_ALT
            && self.altgr_is_ctrl_alt
            && self
                .keys
                .last()
                .is_some_and(|key| key.usage == LEFT_CTRL && !key.sent)
        {
            self.keys.pop();
        }
        if (is_modifier(usage) || self.active()) && !self.is_down(usage) {
            let overflow = !is_modifier(usage) && self.keys_down() >= SLOTS;
            self.keys.push(Key {
                usage,
                down: true,
                modifiers: self.modifiers_down(),
                shown: false,
                sent: false,
                appeared: false,
                overflow,
                errored: false,
                placed: false,
                up_visible: false,
            });
            // The firmware answers a lock key when it is reported (see `settle`), and the LEDs are
            // counted from here.
            if usage == CAPS_LOCK || usage == NUM_LOCK {
                let report = &self.report;
                let locks = self.lock_keys.get_or_insert(LockKeys {
                    updates: report.led_updates(),
                    polls: report.polls(),
                    presses: 0,
                    resets: report.resets(),
                });
                // Whether or not the locks are being followed, the count is kept to date: what
                // has been answered is done with, and the wait for the ones that have not been
                // is from this press.
                if report.led_updates() >= locks.updates + locks.presses {
                    locks.updates = report.led_updates();
                    locks.presses = 0;
                }
                locks.polls = report.polls();
            }
        }
        self.publish();
    }

    /// A key came up. One that was never pressed (a console key) is ignored.
    pub fn release(&mut self, usage: u8) {
        self.settle();
        let held = self
            .keys
            .iter()
            .rposition(|key| key.usage == usage && key.down);
        if let Some(at) = held {
            self.keys[at].down = false;
            // While more than six keys are down the keyboard shows no report without this one.
            let visible = self.keys_down() <= SLOTS;
            self.keys[at].up_visible = visible;
            self.note_ups_the_keyboard_would_show();
        }
        self.publish();
    }

    /// Every key up, as when the window loses the focus and will not see the releases.
    pub fn release_all(&mut self) {
        self.settle();
        for key in &mut self.keys {
            key.down = false;
            key.up_visible = true;
        }
        self.note_ups_the_keyboard_would_show();
        self.publish();
    }

    /// Looks at the keys again, which the window does at every look: the firmware may have
    /// polled since the last one.
    pub fn sync(&mut self) {
        self.settle();
        self.publish();
    }

    /// Makes the firmware's lock keys agree with the host's, once the firmware polls. The
    /// firmware keeps its own caps lock and num lock, which toggle when the key goes down, and
    /// shows them with the keyboard's LEDs. The key presses keep them in step from the start,
    /// but not a lock that was already on when the keyboard came up, or one that was toggled
    /// with the window out of focus: the first time here after the firmware starts polling (or
    /// after `check_locks`) a lock that is not as the host has it gets a tap of its key.
    ///
    /// The LEDs are only the firmware's state once it has set them (it does so a poll or so after
    /// the first, and not as the keyboard is reset), once the lock keys sent so far have been
    /// answered, and with no key waiting to be sent, so the comparison waits for all that, or for
    /// `LED_WAIT_POLLS` polls if the firmware never does. It also waits while the user holds a
    /// lock key, as a tap would be let go of by the release, and while six keys are down, as a tap
    /// would be a seventh and lost. `num` is `None` where the host does
    /// not say, and then Num Lock is left as it is.
    pub fn follow_locks(&mut self, caps: bool, num: Option<bool>) {
        if !self.active() {
            self.locks_followed = false;
            self.lock_keys = None;
        } else if !self.locks_followed && self.leds_are_the_firmwares() {
            self.locks_followed = true;
            let leds = self.report.leds();
            let locks = [(CAPS_LOCK, Some(caps), LED_CAPS), (NUM_LOCK, num, LED_NUM)];
            for (usage, host, led) in locks {
                if host.is_some_and(|host| host != (leds & led != 0)) {
                    self.press(usage);
                    self.release(usage);
                }
            }
        }
    }

    // Whether the keyboard's LED byte is what the firmware has set, and nothing is on its way
    // that would change it. The firmware answers each lock key with an update of the LEDs, so a
    // number of updates are expected, one for each lock key that has gone down.
    //
    // This leans on the timing of the real firmware (7.00.00b5): it sets the LEDs twice after its
    // first poll (three times with both locks in it), all before its second, so a lock key reported at the second poll or later has
    // its start-up updates behind it, and `was_sent()` below holds the comparison until the poll
    // after the key. A model that answered later could take an update that was not the answer for
    // one.
    fn leds_are_the_firmwares(&mut self) -> bool {
        self.settle();
        let report = &self.report;
        let first = report.led_updates() > 0 || report.polls() >= LED_WAIT_POLLS;
        let answered = self.lock_keys.as_ref().is_none_or(|keys| {
            report.led_updates() >= keys.updates + keys.presses
                || report.polls().saturating_sub(keys.polls) >= LED_WAIT_POLLS
        });
        let held = self.is_down(CAPS_LOCK) || self.is_down(NUM_LOCK);
        // A tap now would be a seventh key, and lost.
        let full = self.keys_down() >= SLOTS;
        let current = first && answered && !held && !full && !self.pending() && report.was_sent();
        if current && self.lock_keys.is_some() {
            self.lock_keys = None;
        }
        current
    }

    /// Has `follow_locks` look at the locks again, as when the window gets the focus back.
    pub fn check_locks(&mut self) {
        self.locks_followed = false;
    }

    // With no more than six keys down the keyboard shows a report without each key of the one the
    // firmware holds that is up: those that came up now, and those that came up while the error was
    // the report, which it now stops being.
    fn note_ups_the_keyboard_would_show(&mut self) {
        if self.keys_down() > SLOTS {
            return;
        }
        // A tap that was let go while the error was the report, and has not been sent, is now
        // shown to the keyboard as a key that went down and up, unless a later press of the same
        // key follows it.
        for at in 0..self.keys.len() {
            let usage = self.keys[at].usage;
            // (A later press that had no slot and is let go before it has one is lost, and is not
            // one.)
            let later = self.keys[at + 1..]
                .iter()
                .any(|key| key.usage == usage && (key.down || !key.overflow || key.placed));
            let key = &mut self.keys[at];
            if !key.down && !key.sent && !is_modifier(usage) && !later {
                key.up_visible = true;
            }
        }
        for usage in self.reported[2..]
            .iter()
            .copied()
            .filter(|&usage| usage > 0)
        {
            // A key of that usage that has not been sent is a later press, and its own coming up
            // is not the up of the key the firmware holds.
            let later = self.keys.iter().any(|key| key.usage == usage && !key.sent);
            if !self.is_down(usage) && !later && !self.up_seen.contains(&usage) {
                self.up_seen.push(usage);
            }
        }
    }

    // How many keys other than modifiers are down.
    fn keys_down(&self) -> usize {
        self.keys
            .iter()
            .filter(|key| key.down && !is_modifier(key.usage))
            .count()
    }

    fn is_down(&self, usage: u8) -> bool {
        self.keys.iter().any(|key| key.usage == usage && key.down)
    }

    // The modifiers that are down, as the first byte of a report.
    fn modifiers_down(&self) -> u8 {
        self.keys
            .iter()
            .filter(|key| key.down && is_modifier(key.usage))
            .fold(0, |bits, key| bits | 1 << (key.usage - FIRST_MODIFIER))
    }

    /// Whether a key has not been sent to the firmware yet.
    fn pending(&self) -> bool {
        self.keys.iter().any(|key| !key.sent)
    }

    // Takes in what the keyboard has done since the last look: if the firmware has been sent
    // the report that is set, it has had the keys that are in it, and a key that has been let go
    // after that is no longer wanted.
    fn settle(&mut self) {
        // A reset of the keyboard took the LED and poll counts back to 0, and what was counted
        // against them with it.
        if self
            .lock_keys
            .as_ref()
            .is_some_and(|keys| keys.resets != self.report.resets())
        {
            self.lock_keys = None;
        }
        if self.report.was_sent() {
            let report = self.report.get();
            let error = report[2] == ROLLOVER;
            if !error {
                self.reported = report;
                // The report without a key that was let go has been sent.
                self.up_seen.retain(|usage| report[2..].contains(usage));
            }
            // What the keyboard counted as it handed the report over, not as the window looks.
            let (polls, updates) = (
                self.report.polls_at_report(),
                self.report.led_updates_at_report(),
            );
            // The first poll since the firmware started is followed by an update of the LEDs that
            // is its own, the state it starts with, and then by the answer to a lock key in it
            // (measured on the real firmware, with one update for any number of lock keys).
            let mut start = polls == 1 && updates == 0;
            for key in &mut self.keys {
                // A tap that has been sent was let go before, and the report without it is next.
                if !key.sent
                    && key.shown
                    && key.up_visible
                    && !is_modifier(key.usage)
                    && !self.up_seen.contains(&key.usage)
                {
                    self.up_seen.push(key.usage);
                }
                key.sent |= key.shown;
                key.errored |= error && key.overflow;
                // A lock key toggles the firmware's lock when it first comes in a report as a
                // key: one that was lost to the error, or is held through it, does so later.
                if !key.appeared && !error && report[2..].contains(&key.usage) {
                    key.appeared = true;
                    if matches!(key.usage, CAPS_LOCK | NUM_LOCK) {
                        // With nothing counted (the keyboard was reset since the press, or the
                        // count was done with) the answer is to be waited for from the poll
                        // that had the key.
                        let locks = self.lock_keys.get_or_insert(LockKeys {
                            updates,
                            polls,
                            presses: 0,
                            resets: self.report.resets(),
                        });
                        // One reported late has waited since it went down, and the firmware's
                        // answer is to be waited for from when it was reported: the polls and
                        // the LED updates before then are not its. Presses that have been
                        // answered by then are done with.
                        if key.errored {
                            locks.polls = polls;
                            if updates >= locks.updates + locks.presses {
                                locks.updates = updates;
                                locks.presses = 0;
                            }
                        }
                        locks.presses += 1;
                        if start {
                            locks.presses += 1;
                            start = false;
                        }
                    }
                }
            }
        }
        self.keys
            .retain(|key| key.down || !(key.sent || (key.errored && !key.placed)));
    }

    // The report for the keys as they are now.
    fn publish(&mut self) {
        self.keys
            .retain(|key| key.down || !(key.sent || (key.errored && !key.placed)));
        if !self.active() {
            // Nothing is polling, so there is nothing to keep a key for: only the modifiers are
            // kept, down, for when it does.
            self.keys.retain(|key| is_modifier(key.usage) && key.down);
            for key in &mut self.keys {
                key.shown = false;
                key.sent = false;
            }
            self.reported = [0; 8];
            self.up_seen.clear();
            self.report.set(self.modifiers_down_report());
            return;
        }
        let report = self.build();
        self.report.set(report);
    }

    fn modifiers_down_report(&self) -> [u8; 8] {
        [self.modifiers_down(), 0, 0, 0, 0, 0, 0, 0]
    }

    // Which keys go in the report, and the report. The keys that are down and were sent are
    // there, as they are held. A key that has not been sent goes in if it can share the report
    // with the others that have not: it is not one that was in the last report sent (that
    // report has to be sent without it first, or the firmware would see no key up and down
    // again), is not in the report already, went down with the same modifiers as they did, and
    // there is room (the keys that are held have their slots whatever came before them). The first
    // that does not go in holds back the ones after it, so that they are sent in the order they
    // were pressed, and so does a modifier that was tapped and over before a key went down, which
    // goes in a report of its own first. The modifiers are those of the first key that has
    // not been sent, which are the ones it went down with and not the ones that have been
    // pressed since, or, with none, the ones that are down.
    fn build(&mut self) -> [u8; 8] {
        let mut in_report: Vec<usize> = Vec::new();
        let mut modifiers = None;
        // A key that had no slot and is still waiting for one takes the modifiers that are down
        // when it is reported, as on a keyboard; the others those they went down with, which
        // is for a tap that waited a poll.
        let down_now = self.modifiers_down();
        let own = |key: &Key| {
            if key.errored && !key.placed {
                down_now
            } else {
                key.modifiers
            }
        };
        let mut held_back = false;
        // The keys that are held are in the report whatever came before them in the order, so
        // a key waiting for its turn has only the slots they leave.
        let held = self
            .keys
            .iter()
            .filter(|key| key.sent && !is_modifier(key.usage))
            .count();
        let mut waiting_in_report = 0;
        for (at, key) in self.keys.iter().enumerate() {
            if is_modifier(key.usage) {
                // A tap of a modifier that came before a key and was over before it went down
                // is sent ahead of that key and not with it, or the firmware would see the key
                // held with a modifier it never had.
                let bit = 1 << (key.usage - FIRST_MODIFIER);
                let before_a_key = self.keys[at + 1..]
                    .iter()
                    .any(|later| !later.sent && !is_modifier(later.usage) && own(later) & bit == 0);
                held_back |= !key.sent && before_a_key;
                continue;
            }
            if key.sent {
                in_report.push(at);
                continue;
            }
            let taken = |usage: u8| {
                in_report
                    .iter()
                    .any(|&other| self.keys[other].usage == usage)
            };
            let clashes = held_back
                || waiting_in_report + held >= SLOTS
                || self.reported[2..].contains(&key.usage) && self.up_seen.contains(&key.usage)
                || taken(key.usage)
                || modifiers.is_some_and(|bits| bits != own(key));
            if clashes {
                held_back = true;
            } else {
                modifiers = Some(own(key));
                waiting_in_report += 1;
                in_report.push(at);
            }
        }
        let modifiers = modifiers.unwrap_or_else(|| {
            self.keys
                .iter()
                .filter(|key| is_modifier(key.usage) && (key.down || !key.sent))
                .fold(0, |bits, key| bits | 1 << (key.usage - FIRST_MODIFIER))
        });
        let mut report = [0; 8];
        report[0] = modifiers;
        for (slot, &at) in in_report.iter().take(SLOTS).enumerate() {
            report[2 + slot] = self.keys[at].usage;
        }
        // More keys down than there are slots is the rollover error, which reports no key: the
        // firmware is told only that there are too many, as by a real keyboard.
        // A key that went down with no slot for it makes the error until the firmware has been
        // sent it, whatever has come up since: the keyboard would have been polled while the
        // seven were down, and the emulator is polled too slowly to be sure it was.
        let rollover =
            self.keys_down() > SLOTS || self.keys.iter().any(|key| key.overflow && !key.errored);
        if rollover {
            report[2..].fill(ROLLOVER);
        }
        for (at, key) in self.keys.iter_mut().enumerate() {
            key.shown = if is_modifier(key.usage) {
                modifiers >> (key.usage - FIRST_MODIFIER) & 1 != 0
            } else {
                // The error report shows no key: the firmware has only the keys it already had,
                // and the others, down or let go, wait for their turn. A key that had no slot
                // is lost if it is let go before it has one (see `errored`).
                if rollover {
                    key.down && key.sent
                } else {
                    in_report.contains(&at)
                }
            };
            // The modifiers of the first report that has the key are its own from then on.
            if key.errored && !key.placed && key.shown {
                key.modifiers = modifiers;
                key.placed = true;
            }
        }
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmm2_core::{HidKeyboard, In, UsbDevice};

    const A: u8 = 0x04;
    const B: u8 = 0x05;
    const LEFT_SHIFT: u8 = 0xE1;
    const RIGHT_GUI: u8 = 0xE7;

    /// The keys, the keyboard they feed and the report between them, before the firmware has
    /// polled.
    fn unpolled() -> (UsbKeys, HidKeyboard, KeyReport) {
        let report = KeyReport::new();
        let mut keyboard = HidKeyboard::new(report.clone());
        keyboard.setup([0x00, 0x09, 1, 0, 0, 0, 0, 0]);
        keyboard.in_token(0, 8);
        (UsbKeys::new(report.clone()), keyboard, report)
    }

    /// As `unpolled`, once the firmware has polled once.
    fn polled() -> (UsbKeys, HidKeyboard, KeyReport) {
        let (keys, mut keyboard, report) = unpolled();
        keyboard.in_token(1, 8);
        (keys, keyboard, report)
    }

    fn poll(keyboard: &mut HidKeyboard) -> In {
        keyboard.in_token(1, 8)
    }

    /// The reports the firmware is sent, in order, if it polls until the keys have no more for
    /// it.
    fn delivered(keys: &mut UsbKeys, keyboard: &mut HidKeyboard) -> Vec<[u8; 8]> {
        let mut reports = Vec::new();
        for _ in 0..100 {
            if let In::Data(report) = poll(keyboard) {
                reports.push(report.try_into().unwrap());
            }
            keys.sync();
            if !keys.pending() && keys.report.was_sent() {
                return reports;
            }
        }
        panic!("the keys never had nothing more to send");
    }

    /// What a firmware types from the reports it is sent, after one with no keys (see `hear`).
    fn typed_by_the_firmware(reports: impl IntoIterator<Item = [u8; 8]>) -> Vec<(u8, u8)> {
        let mut last = [0; 8];
        reports
            .into_iter()
            .flat_map(|report| hear(&mut last, In::Data(report.to_vec())))
            .collect()
    }

    /// The report the firmware ends up with, once it has all of them.
    fn settled(keys: &mut UsbKeys, keyboard: &mut HidKeyboard) -> [u8; 8] {
        delivered(keys, keyboard);
        keys.report.get()
    }

    /// The firmware sets the LEDs, as it does with SET_REPORT.
    fn set_leds(keyboard: &mut HidKeyboard, leds: u8) {
        keyboard.setup([0x21, 0x09, 0, 0x02, 0, 0, 1, 0]);
        keyboard.out(0, &[leds]);
    }

    #[test]
    fn scancodes_are_the_usb_usages() {
        assert_eq!(usage(Scancode::A), Some(0x04));
        assert_eq!(usage(Scancode::Z), Some(0x1D));
        assert_eq!(usage(Scancode::Num1), Some(0x1E));
        assert_eq!(usage(Scancode::Num0), Some(0x27));
        assert_eq!(usage(Scancode::Return), Some(0x28));
        assert_eq!(usage(Scancode::Space), Some(0x2C));
        assert_eq!(usage(Scancode::F1), Some(0x3A));
        assert_eq!(usage(Scancode::F12), Some(0x45));
        assert_eq!(usage(Scancode::Right), Some(0x4F));
        assert_eq!(usage(Scancode::Up), Some(0x52));
        assert_eq!(usage(Scancode::KpEnter), Some(0x58));
        assert_eq!(usage(Scancode::NonUsBackslash), Some(0x64));
        assert_eq!(usage(Scancode::Application), Some(0x65));
        assert_eq!(usage(Scancode::LCtrl), Some(0xE0));
        assert_eq!(usage(Scancode::LShift), Some(0xE1));
        assert_eq!(usage(Scancode::RGui), Some(0xE7));
        assert_eq!(usage(Scancode::CapsLock), Some(CAPS_LOCK));
        assert_eq!(usage(Scancode::NumLockClear), Some(NUM_LOCK));
    }

    #[test]
    fn keys_the_boot_report_cannot_carry_have_no_usage() {
        assert_eq!(usage(Scancode::Power), None);
        assert_eq!(usage(Scancode::F13), None);
        assert_eq!(usage(Scancode::AudioMute), None);
        assert_eq!(usage(Scancode::Mode), None);
    }

    #[test]
    fn it_is_active_once_the_firmware_polls() {
        let (keys, _, _) = unpolled();
        assert!(!keys.active());
        let (keys, _, _) = polled();
        assert!(keys.active());
    }

    #[test]
    fn keys_pressed_together_are_one_report_in_the_order_they_went_down() {
        let (mut keys, mut keyboard, _) = polled();
        keys.press(B);
        keys.press(A);
        keys.press(0x1D);
        assert_eq!(
            delivered(&mut keys, &mut keyboard),
            [[0, 0, B, A, 0x1D, 0, 0, 0]]
        );
    }

    #[test]
    fn the_modifiers_are_the_bits_of_the_first_byte() {
        let (mut keys, mut keyboard, _) = polled();
        for modifier in 0xE0..=0xE7 {
            keys.press(modifier);
        }
        assert_eq!(
            settled(&mut keys, &mut keyboard),
            [0xFF, 0, 0, 0, 0, 0, 0, 0]
        );
        keys.release(LEFT_SHIFT);
        keys.release(RIGHT_GUI);
        assert_eq!(
            settled(&mut keys, &mut keyboard),
            [0b0111_1101, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn a_modifier_does_not_take_a_key_slot() {
        let (mut keys, mut keyboard, _) = polled();
        keys.press(LEFT_SHIFT);
        for usage in A..A + 6 {
            keys.press(usage);
        }
        assert_eq!(settled(&mut keys, &mut keyboard), [2, 0, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn a_key_let_go_leaves_the_others_in_their_order() {
        let (mut keys, mut keyboard, _) = polled();
        keys.press(A);
        keys.press(B);
        keys.press(0x06);
        delivered(&mut keys, &mut keyboard);
        keys.release(B);
        assert_eq!(
            settled(&mut keys, &mut keyboard),
            [0, 0, A, 0x06, 0, 0, 0, 0]
        );
    }

    #[test]
    fn more_than_six_keys_down_is_the_rollover_error_in_every_slot() {
        let (mut keys, mut keyboard, _) = polled();
        keys.press(LEFT_SHIFT);
        for usage in A..A + 7 {
            keys.press(usage);
        }
        // The error is what the firmware is sent for as long as they are down.
        for _ in 0..3 {
            poll(&mut keyboard);
            keys.sync();
        }
        assert_eq!(keys.report.get(), [2, 0, 1, 1, 1, 1, 1, 1]);
        // Back to six, and the keys are reported again.
        keys.release(A + 6);
        assert_eq!(settled(&mut keys, &mut keyboard), [2, 0, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn six_keys_are_not_a_rollover() {
        let (mut keys, mut keyboard, _) = polled();
        for usage in A..A + 6 {
            keys.press(usage);
        }
        assert_eq!(settled(&mut keys, &mut keyboard), [0, 0, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn eight_keys_tapped_at_once_are_typed_six_and_two_in_order() {
        let (mut keys, mut keyboard, _) = polled();
        for usage in A..A + 8 {
            keys.press(usage);
            keys.release(usage);
        }
        let typed = typed_by_the_firmware(delivered(&mut keys, &mut keyboard));
        assert_eq!(
            typed,
            (A..A + 8).map(|usage| (usage, 0)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_key_pressed_again_while_down_is_one_key() {
        let (mut keys, mut keyboard, _) = polled();
        keys.press(A);
        keys.press(A);
        assert_eq!(
            delivered(&mut keys, &mut keyboard),
            [[0, 0, A, 0, 0, 0, 0, 0]]
        );
    }

    #[test]
    fn a_key_that_is_held_is_in_every_report_for_as_long_as_it_is_held() {
        let (mut keys, mut keyboard, report) = polled();
        keys.press(A);
        assert_eq!(poll(&mut keyboard), In::Data(vec![0, 0, A, 0, 0, 0, 0, 0]));
        for _ in 0..1000 {
            keys.sync();
            assert_eq!(report.get(), [0, 0, A, 0, 0, 0, 0, 0]);
            assert_eq!(poll(&mut keyboard), In::Nak);
        }
        keys.release(A);
        assert_eq!(report.get(), [0; 8]);
        assert_eq!(poll(&mut keyboard), In::Data(vec![0; 8]));
    }

    #[test]
    fn a_key_held_with_others_comes_and_goes_with_them_as_they_are_held() {
        let (mut keys, mut keyboard, _) = polled();
        keys.press(A);
        delivered(&mut keys, &mut keyboard);
        keys.press(B);
        keys.release(B);
        assert_eq!(
            delivered(&mut keys, &mut keyboard),
            [[0, 0, A, B, 0, 0, 0, 0], [0, 0, A, 0, 0, 0, 0, 0]]
        );
    }

    #[test]
    fn letting_go_of_a_key_never_pressed_changes_nothing() {
        let (mut keys, mut keyboard, _) = polled();
        keys.press(A);
        keys.release(B);
        assert_eq!(
            delivered(&mut keys, &mut keyboard),
            [[0, 0, A, 0, 0, 0, 0, 0]]
        );
    }

    #[test]
    fn losing_the_focus_lets_go_of_every_key() {
        let (mut keys, mut keyboard, report) = polled();
        keys.press(LEFT_SHIFT);
        keys.press(A);
        keys.press(B);
        delivered(&mut keys, &mut keyboard);
        keys.release_all();
        assert_eq!(report.get(), [0; 8]);
        // And a key pressed after it is a fresh press.
        keys.press(B);
        assert_eq!(settled(&mut keys, &mut keyboard), [0, 0, B, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn a_key_tapped_between_two_polls_is_held_until_it_is_polled() {
        let (mut keys, mut keyboard, report) = polled();
        keys.press(A);
        keys.release(A);
        assert_eq!(report.get(), [0, 0, A, 0, 0, 0, 0, 0]);
        // Nothing has polled yet, so it stays however often the window looks.
        keys.sync();
        keys.sync();
        assert_eq!(report.get(), [0, 0, A, 0, 0, 0, 0, 0]);
        assert_eq!(poll(&mut keyboard), In::Data(vec![0, 0, A, 0, 0, 0, 0, 0]));
        // The firmware has it, and the next look lets it go.
        keys.sync();
        assert_eq!(report.get(), [0; 8]);
        assert_eq!(poll(&mut keyboard), In::Data(vec![0; 8]));
    }

    #[test]
    fn a_tapped_modifier_is_held_until_it_is_polled_too() {
        let (mut keys, mut keyboard, _) = polled();
        keys.press(LEFT_SHIFT);
        keys.release(LEFT_SHIFT);
        assert_eq!(
            delivered(&mut keys, &mut keyboard),
            [[2, 0, 0, 0, 0, 0, 0, 0], [0; 8]]
        );
    }

    #[test]
    fn a_key_tapped_twice_between_two_polls_is_reported_let_go_and_reported_again() {
        let (mut keys, mut keyboard, _) = polled();
        keys.press(A);
        keys.release(A);
        keys.press(A);
        keys.release(A);
        assert_eq!(
            delivered(&mut keys, &mut keyboard),
            [
                [0, 0, A, 0, 0, 0, 0, 0],
                [0; 8],
                [0, 0, A, 0, 0, 0, 0, 0],
                [0; 8]
            ]
        );
    }

    #[test]
    fn a_key_pressed_again_before_it_was_polled_is_held_down() {
        let (mut keys, mut keyboard, _) = polled();
        keys.press(A);
        keys.release(A);
        keys.press(A);
        assert_eq!(
            delivered(&mut keys, &mut keyboard),
            [[0, 0, A, 0, 0, 0, 0, 0], [0; 8], [0, 0, A, 0, 0, 0, 0, 0]]
        );
        keys.release(A);
        assert_eq!(keys.report.get(), [0; 8]);
    }

    #[test]
    fn a_held_key_let_go_and_pressed_again_before_a_poll_is_two_keystrokes() {
        let (mut keys, mut keyboard, _) = polled();
        keys.press(A);
        delivered(&mut keys, &mut keyboard);
        keys.release(A);
        keys.press(A);
        assert_eq!(
            delivered(&mut keys, &mut keyboard),
            [[0; 8], [0, 0, A, 0, 0, 0, 0, 0]]
        );
    }

    #[test]
    fn shift_held_across_a_poll_and_a_letter_tapped_fast_is_the_letter_with_shift() {
        let (mut keys, mut keyboard, _) = polled();
        keys.press(LEFT_SHIFT);
        delivered(&mut keys, &mut keyboard);
        keys.press(A);
        keys.release(LEFT_SHIFT);
        keys.release(A);
        assert_eq!(
            delivered(&mut keys, &mut keyboard),
            [[2, 0, A, 0, 0, 0, 0, 0], [0; 8]]
        );
    }

    #[test]
    fn shift_and_a_letter_both_tapped_between_two_polls_are_the_letter_with_shift() {
        let (mut keys, mut keyboard, _) = polled();
        keys.press(LEFT_SHIFT);
        keys.press(A);
        keys.release(LEFT_SHIFT);
        keys.release(A);
        assert_eq!(
            delivered(&mut keys, &mut keyboard),
            [[2, 0, A, 0, 0, 0, 0, 0], [0; 8]]
        );
    }

    #[test]
    fn a_shift_tapped_just_before_a_key_is_not_held_with_it() {
        let (mut keys, mut keyboard, report) = polled();
        keys.press(LEFT_SHIFT);
        keys.release(LEFT_SHIFT);
        keys.press(A);
        // The Shift went before the key, so it is sent on its own first, and A is held after it
        // without the Shift: KEYDOWN(7) must not see a Shift that was never held with it.
        let mut reports = Vec::new();
        for _ in 0..10 {
            if let In::Data(sent) = poll(&mut keyboard) {
                reports.push(sent);
            }
            keys.sync();
        }
        assert_eq!(
            reports,
            [vec![2, 0, 0, 0, 0, 0, 0, 0], vec![0, 0, A, 0, 0, 0, 0, 0]]
        );
        assert_eq!(report.get(), [0, 0, A, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn a_shift_tapped_while_a_key_is_held_is_held_with_it() {
        let (mut keys, mut keyboard, _) = polled();
        keys.press(A);
        delivered(&mut keys, &mut keyboard);
        keys.press(LEFT_SHIFT);
        keys.release(LEFT_SHIFT);
        assert_eq!(
            delivered(&mut keys, &mut keyboard),
            [[2, 0, A, 0, 0, 0, 0, 0], [0, 0, A, 0, 0, 0, 0, 0]]
        );
    }

    #[test]
    fn a_key_tapped_before_more_than_six_go_down_is_not_lost() {
        let (mut keys, mut keyboard, _) = polled();
        // The tap has not been polled when seven keys go down, and the firmware's next poll is
        // of the error report, which has no key in it.
        keys.press(0x20);
        keys.release(0x20);
        for usage in A..A + 7 {
            keys.press(usage);
        }
        poll(&mut keyboard);
        keys.sync();
        assert_eq!(keys.report.get(), [0, 0, 1, 1, 1, 1, 1, 1]);
        // Back to six keys the tap still has no slot, and with five it has.
        keys.release(A + 6);
        keys.release(A + 5);
        let reports = delivered(&mut keys, &mut keyboard);
        assert!(
            reports.iter().any(|report| report[2..].contains(&0x20)),
            "{reports:?}"
        );
    }

    #[test]
    fn a_key_tapped_while_six_are_held_is_the_rollover_error_and_is_lost() {
        // As on a real keyboard: a seventh key down is the error in every slot for as long as it
        // is down, and it is never typed.
        let (mut keys, mut keyboard, report) = polled();
        for usage in A..A + 6 {
            keys.press(usage);
        }
        delivered(&mut keys, &mut keyboard);
        keys.press(0x20);
        keys.release(0x20);
        // The tap is over before the firmware polls, and it still sees the error once.
        assert_eq!(report.get(), [0, 0, 1, 1, 1, 1, 1, 1]);
        let reports = delivered(&mut keys, &mut keyboard);
        assert_eq!(
            reports,
            [[0, 0, 1, 1, 1, 1, 1, 1], [0, 0, 4, 5, 6, 7, 8, 9]]
        );
        // Nothing is left waiting, so nothing holds up the locks.
        assert!(!keys.pending());
        assert!(!reports.iter().any(|report| report[2..].contains(&0x20)));
    }

    #[test]
    fn seven_keys_tapped_together_are_the_error_once_and_the_seventh_is_lost() {
        let (mut keys, mut keyboard, _) = polled();
        for usage in A..A + 7 {
            keys.press(usage);
        }
        for usage in A..A + 7 {
            keys.release(usage);
        }
        // The seventh was down with six others, and the error goes out whatever came up since.
        let reports = delivered(&mut keys, &mut keyboard);
        assert_eq!(reports[0], [0, 0, 1, 1, 1, 1, 1, 1], "{reports:?}");
        let typed: Vec<u8> = typed_by_the_firmware(reports)
            .iter()
            .map(|&(key, _)| key)
            .collect();
        assert_eq!(typed, (A..A + 6).collect::<Vec<_>>());
    }

    #[test]
    fn a_seventh_key_tapped_as_a_held_one_comes_up_is_still_the_error() {
        let (mut keys, mut keyboard, _) = polled();
        for usage in A..A + 6 {
            keys.press(usage);
        }
        delivered(&mut keys, &mut keyboard);
        keys.press(0x20);
        keys.release(0x20);
        keys.release(A);
        let reports = delivered(&mut keys, &mut keyboard);
        assert_eq!(reports[0], [0, 0, 1, 1, 1, 1, 1, 1], "{reports:?}");
        assert!(!reports.iter().any(|report| report[2..].contains(&0x20)));
    }

    #[test]
    fn a_lock_key_lost_to_the_error_is_not_waited_for() {
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        for usage in A..A + 6 {
            keys.press(usage);
        }
        delivered(&mut keys, &mut keyboard);
        // Caps Lock goes down as a seventh key: the firmware never sees it, so no LED update is
        // to be expected for it.
        keys.press(CAPS_LOCK);
        keys.release(CAPS_LOCK);
        delivered(&mut keys, &mut keyboard);
        for usage in A..A + 6 {
            keys.release(usage);
        }
        delivered(&mut keys, &mut keyboard);
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard).len(), 2);
    }

    fn typed_keys(reports: Vec<[u8; 8]>) -> Vec<u8> {
        typed_by_the_firmware(reports)
            .iter()
            .map(|&(key, _)| key)
            .collect()
    }

    #[test]
    fn a_key_pressed_and_let_go_after_the_error_went_out_is_typed() {
        let (mut keys, mut keyboard, _) = polled();
        for usage in A..A + 6 {
            keys.press(usage);
        }
        delivered(&mut keys, &mut keyboard);
        keys.press(0x20);
        keys.release(0x20);
        keys.release(A);
        keys.release(B);
        // H goes down before the error has gone out, and comes up before the poll after it.
        keys.press(0x0B);
        poll(&mut keyboard);
        keys.sync();
        keys.release(0x0B);
        let reports = delivered(&mut keys, &mut keyboard);
        assert!(typed_keys(reports).contains(&0x0B));
    }

    #[test]
    fn keys_pressed_with_a_seventh_are_typed_when_it_comes_up_first() {
        let (mut keys, mut keyboard, _) = polled();
        for usage in A..A + 7 {
            keys.press(usage);
        }
        keys.release(A + 6);
        poll(&mut keyboard);
        keys.sync();
        for usage in A..A + 6 {
            keys.release(usage);
        }
        let reports = delivered(&mut keys, &mut keyboard);
        assert_eq!(typed_keys(reports), (A..A + 6).collect::<Vec<_>>());
    }

    #[test]
    fn a_waiting_tap_is_typed_before_the_key_after_it_when_a_seventh_is_lost() {
        let (mut keys, mut keyboard, _) = polled();
        for usage in A..A + 5 {
            keys.press(usage);
        }
        delivered(&mut keys, &mut keyboard);
        keys.press(0x20);
        keys.release(0x20);
        keys.press(0x21);
        // Six are down, so this is the seventh.
        keys.press(0x22);
        keys.release(0x22);
        let reports = delivered(&mut keys, &mut keyboard);
        let typed = typed_keys(reports);
        let at = |key| typed.iter().position(|&typed| typed == key);
        assert!(at(0x20) < at(0x21) && at(0x20).is_some(), "{typed:?}");
        assert_eq!(at(0x22), None);
    }

    #[test]
    fn a_lock_key_held_as_a_seventh_is_waited_for_once_it_is_reported() {
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        for usage in A..A + 6 {
            keys.press(usage);
        }
        delivered(&mut keys, &mut keyboard);
        // The error goes out for it, and then a slot frees and it is reported and toggles.
        keys.press(CAPS_LOCK);
        poll(&mut keyboard);
        keys.sync();
        keys.release(A);
        let reports = delivered(&mut keys, &mut keyboard);
        assert!(
            reports
                .iter()
                .any(|report| report[2..].contains(&CAPS_LOCK))
        );
        keys.release(CAPS_LOCK);
        delivered(&mut keys, &mut keyboard);
        // The firmware has not shown it on the LEDs yet, so the host's lock is not compared.
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
    }

    #[test]
    fn keys_are_reported_in_the_order_they_went_down_after_the_error() {
        let (mut keys, mut keyboard, _) = polled();
        keys.press(LEFT_SHIFT);
        for usage in A..A + 7 {
            keys.press(usage);
        }
        // The error goes out, and the next report has the keys that fit.
        let mut reports = Vec::new();
        for _ in 0..3 {
            if let In::Data(report) = poll(&mut keyboard) {
                reports.push(report.try_into().unwrap());
            }
            keys.sync();
        }
        keys.release(A);
        reports.extend(delivered(&mut keys, &mut keyboard));
        // Typed in the order they went down: G, which the error was for, is not ahead of F.
        let typed = typed_keys(reports.clone());
        assert!(typed.is_sorted(), "{typed:?} {reports:?}");
        assert_eq!(
            reports.last(),
            Some(&[2, 0, B, 6, 7, 8, 9, 0x0A]),
            "{reports:?}"
        );
    }

    #[test]
    fn the_locks_are_not_followed_while_six_keys_are_down() {
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        for usage in A..A + 6 {
            keys.press(usage);
        }
        delivered(&mut keys, &mut keyboard);
        // A tap of Caps Lock now would be a seventh key, and lost.
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
        for usage in A..A + 6 {
            keys.release(usage);
        }
        delivered(&mut keys, &mut keyboard);
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard).len(), 2);
    }

    /// Six keys held, Caps Lock pressed as a seventh and held, `polls` polls and `updates` LED
    /// updates by the firmware while it waits, then a key comes up and Caps Lock is reported.
    fn caps_reported_late(polls: u32, updates: u32) -> (UsbKeys, HidKeyboard) {
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        for usage in A..A + 6 {
            keys.press(usage);
        }
        delivered(&mut keys, &mut keyboard);
        keys.press(CAPS_LOCK);
        for _ in 0..polls {
            poll(&mut keyboard);
            keys.sync();
        }
        for _ in 0..updates {
            set_leds(&mut keyboard, 0);
        }
        keys.release(A);
        delivered(&mut keys, &mut keyboard);
        keys.release(CAPS_LOCK);
        delivered(&mut keys, &mut keyboard);
        (keys, keyboard)
    }

    #[test]
    fn the_wait_for_a_lock_key_held_through_the_error_counts_from_when_it_was_reported() {
        let (mut keys, mut keyboard) = caps_reported_late(LED_WAIT_POLLS - 50, 0);
        // More than LED_WAIT_POLLS since the press, and less since the report.
        for _ in 0..100 {
            poll(&mut keyboard);
        }
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
    }

    #[test]
    fn the_leds_set_while_a_lock_key_waited_are_not_its_answer() {
        let (mut keys, mut keyboard) = caps_reported_late(10, 1);
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
        // Its own answer, and then the host's lock is compared.
        set_leds(&mut keyboard, 0);
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard).len(), 2);
    }

    /// Six keys held and sent (A to F), with `modifier` down before them if it is not 0, G as a
    /// seventh, and the error gone out for it.
    fn error_sent_for_a_seventh(modifier: u8) -> (UsbKeys, HidKeyboard) {
        let (mut keys, mut keyboard, _) = polled();
        if modifier != 0 {
            keys.press(modifier);
        }
        for usage in A..A + 6 {
            keys.press(usage);
        }
        delivered(&mut keys, &mut keyboard);
        keys.press(A + 6);
        let In::Data(error) = poll(&mut keyboard) else {
            panic!("the error is a report");
        };
        assert_eq!(error[2..], [ROLLOVER; 6]);
        keys.sync();
        (keys, keyboard)
    }

    #[test]
    fn a_seventh_key_takes_the_modifiers_that_are_down_when_it_is_reported() {
        // Shift came up while the error was the report, and the key is not typed with it.
        let (mut keys, mut keyboard) = error_sent_for_a_seventh(LEFT_SHIFT);
        keys.release(LEFT_SHIFT);
        keys.release(A);
        let reports = delivered(&mut keys, &mut keyboard);
        assert!(reports.iter().all(|report| report[0] == 0), "{reports:?}");
        assert_eq!(reports.last(), Some(&[0, 0, B, 6, 7, 8, 9, 0x0A]));
        // And the other way: Shift pressed while it waited is down when it is reported.
        let (mut keys, mut keyboard) = error_sent_for_a_seventh(0);
        keys.press(LEFT_SHIFT);
        keys.release(A);
        let reports = delivered(&mut keys, &mut keyboard);
        assert_eq!(
            reports.last(),
            Some(&[2, 0, B, 6, 7, 8, 9, 0x0A]),
            "{reports:?}"
        );
    }

    #[test]
    fn a_seventh_key_takes_the_modifiers_of_the_report_that_first_has_it() {
        // Shift comes up after the slot frees and before the poll: the firmware types G as it was
        // in the report that was set when A came up, with Shift.
        let (mut keys, mut keyboard) = error_sent_for_a_seventh(LEFT_SHIFT);
        keys.release(A);
        keys.release(LEFT_SHIFT);
        assert_eq!(
            poll(&mut keyboard),
            In::Data(vec![2, 0, B, 6, 7, 8, 9, 0x0A])
        );
        // And Shift pressed after the slot frees is not in the key's report.
        let (mut keys, mut keyboard) = error_sent_for_a_seventh(0);
        keys.release(A);
        keys.press(LEFT_SHIFT);
        assert_eq!(
            poll(&mut keyboard),
            In::Data(vec![0, 0, B, 6, 7, 8, 9, 0x0A])
        );
        keys.sync();
        assert_eq!(
            poll(&mut keyboard),
            In::Data(vec![2, 0, B, 6, 7, 8, 9, 0x0A])
        );
    }

    #[test]
    fn a_seventh_key_keeps_the_modifiers_it_was_first_reported_with_not_those_it_went_down_with() {
        // G went down with no Shift, and Shift came down while the error was the report.
        let (mut keys, mut keyboard) = error_sent_for_a_seventh(0);
        keys.press(LEFT_SHIFT);
        keys.release(A);
        keys.release(LEFT_SHIFT);
        assert_eq!(
            poll(&mut keyboard),
            In::Data(vec![2, 0, B, 6, 7, 8, 9, 0x0A])
        );
    }

    #[test]
    fn an_extra_led_update_is_not_taken_for_the_answer_to_a_later_lock_key() {
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        keys.press(CAPS_LOCK);
        keys.release(CAPS_LOCK);
        delivered(&mut keys, &mut keyboard);
        set_leds(&mut keyboard, LED_CAPS);
        set_leds(&mut keyboard, LED_CAPS);
        keys.press(NUM_LOCK);
        keys.release(NUM_LOCK);
        delivered(&mut keys, &mut keyboard);
        // The firmware has Caps Lock on and the host has not: a tap, once the answer to Num Lock
        // is in, and not before.
        keys.follow_locks(false, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
        set_leds(&mut keyboard, LED_CAPS | LED_NUM);
        keys.follow_locks(false, Some(true));
        assert_eq!(delivered(&mut keys, &mut keyboard).len(), 2);
    }

    #[test]
    fn a_seventh_key_in_a_report_that_a_new_error_replaces_is_not_lost() {
        let (mut keys, mut keyboard) = error_sent_for_a_seventh(0);
        // A slot frees, and before the report with G is polled another key is a seventh.
        keys.release(A);
        keys.press(0x0B);
        keys.release(A + 6);
        keys.release(B);
        let reports = delivered(&mut keys, &mut keyboard);
        assert!(
            reports.iter().any(|report| report[2..].contains(&(A + 6))),
            "{reports:?}"
        );
    }

    #[test]
    fn a_lock_pressed_and_not_answered_is_waited_for_after_an_older_one_was_answered() {
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        keys.follow_locks(false, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
        keys.press(CAPS_LOCK);
        keys.release(CAPS_LOCK);
        delivered(&mut keys, &mut keyboard);
        set_leds(&mut keyboard, LED_CAPS);
        for _ in 0..250 {
            poll(&mut keyboard);
        }
        // Tapped again and handed over, and the firmware has not shown it yet.
        keys.press(CAPS_LOCK);
        keys.release(CAPS_LOCK);
        delivered(&mut keys, &mut keyboard);
        keys.check_locks();
        keys.follow_locks(false, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
    }

    #[test]
    fn a_seventh_key_reported_and_let_go_before_the_next_poll_is_typed() {
        let (mut keys, mut keyboard) = error_sent_for_a_seventh(0);
        // A slot frees, which sets the report with G in it, and G comes up before it is polled.
        keys.release(A);
        keys.release(A + 6);
        let reports = delivered(&mut keys, &mut keyboard);
        assert!(
            reports.iter().any(|report| report[2..].contains(&(A + 6))),
            "{reports:?}"
        );
    }

    #[test]
    fn the_answer_to_a_lock_key_reported_late_that_came_before_the_look_is_counted() {
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        for usage in A..A + 6 {
            keys.press(usage);
        }
        delivered(&mut keys, &mut keyboard);
        keys.press(CAPS_LOCK);
        poll(&mut keyboard);
        keys.sync();
        keys.release(A);
        // The report with Caps Lock in it goes out, and the firmware answers it before the window
        // looks at the keys again.
        poll(&mut keyboard);
        set_leds(&mut keyboard, LED_CAPS);
        keys.sync();
        keys.release(CAPS_LOCK);
        delivered(&mut keys, &mut keyboard);
        // The lock is on in the firmware and off at the host: decided at once, with a tap.
        keys.follow_locks(false, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard).len(), 2);
    }

    #[test]
    fn the_wait_for_a_lock_key_held_through_the_error_counts_from_the_report_that_had_it() {
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        for usage in A..A + 6 {
            keys.press(usage);
        }
        delivered(&mut keys, &mut keyboard);
        keys.press(CAPS_LOCK);
        poll(&mut keyboard);
        keys.sync();
        keys.release(A);
        // It is handed over, and the window does not look for as long as the firmware never
        // answers: the wait is over by then, though the look is just now.
        for _ in 0..=LED_WAIT_POLLS {
            poll(&mut keyboard);
        }
        keys.sync();
        keys.release(CAPS_LOCK);
        delivered(&mut keys, &mut keyboard);
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard).len(), 2);
    }

    #[test]
    fn an_earlier_lock_key_that_was_answered_does_not_hold_a_later_one_to_its_count() {
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        // Caps Lock tapped and answered, and the locks not looked at since.
        keys.press(CAPS_LOCK);
        keys.release(CAPS_LOCK);
        delivered(&mut keys, &mut keyboard);
        set_leds(&mut keyboard, LED_CAPS);
        // Another, held as a seventh, with the firmware setting the LEDs while it waits.
        for usage in A..A + 6 {
            keys.press(usage);
        }
        delivered(&mut keys, &mut keyboard);
        keys.press(NUM_LOCK);
        poll(&mut keyboard);
        keys.sync();
        set_leds(&mut keyboard, LED_CAPS);
        keys.release(A);
        delivered(&mut keys, &mut keyboard);
        keys.release(NUM_LOCK);
        delivered(&mut keys, &mut keyboard);
        // Num Lock has not been answered: the host's locks are not compared yet.
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
        set_leds(&mut keyboard, LED_CAPS | LED_NUM);
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard).len(), 2);
    }

    #[test]
    fn ctrl_and_c_tapped_between_two_polls_arrive_together() {
        let (mut keys, mut keyboard, _) = polled();
        keys.press(0xE0);
        keys.press(0x06);
        keys.release(0xE0);
        keys.release(0x06);
        assert_eq!(
            delivered(&mut keys, &mut keyboard),
            [[1, 0, 6, 0, 0, 0, 0, 0], [0; 8]]
        );
    }

    #[test]
    fn a_letter_and_then_a_capital_before_a_poll_each_keep_their_own_modifiers() {
        let (mut keys, mut keyboard, _) = polled();
        keys.press(A);
        keys.release(A);
        keys.press(LEFT_SHIFT);
        keys.press(B);
        keys.release(B);
        keys.release(LEFT_SHIFT);
        assert_eq!(
            typed_by_the_firmware(delivered(&mut keys, &mut keyboard)),
            [(A, 0), (B, 2)]
        );
    }

    #[test]
    fn two_letters_tapped_before_a_poll_are_one_report_and_both_are_typed() {
        let (mut keys, mut keyboard, _) = polled();
        keys.press(A);
        keys.release(A);
        keys.press(B);
        keys.release(B);
        assert_eq!(
            delivered(&mut keys, &mut keyboard),
            [[0, 0, A, B, 0, 0, 0, 0], [0; 8]]
        );
    }

    #[test]
    fn before_the_firmware_polls_the_keys_are_not_the_reports() {
        let (mut keys, _, report) = unpolled();
        keys.press(A);
        keys.release(A);
        assert_eq!(report.get(), [0; 8]);
    }

    #[test]
    fn before_the_firmware_polls_the_modifiers_are_kept() {
        let (mut keys, _, report) = unpolled();
        keys.press(LEFT_SHIFT);
        assert_eq!(report.get(), [2, 0, 0, 0, 0, 0, 0, 0]);
        keys.release(LEFT_SHIFT);
        // Not held for a poll that may never come: a tap here is gone.
        assert_eq!(report.get(), [0; 8]);
    }

    #[test]
    fn a_key_held_across_the_switch_is_typed_once_and_is_not_pressed_again() {
        let (mut keys, mut keyboard, report) = unpolled();
        // A and Shift go down before the firmware polls: A is the console's, Shift is kept.
        keys.press(LEFT_SHIFT);
        keys.press(A);
        assert!(!keys.active());
        assert_eq!(poll(&mut keyboard), In::Data(vec![2, 0, 0, 0, 0, 0, 0, 0]));
        assert!(keys.active());
        keys.sync();
        assert_eq!(report.get(), [2, 0, 0, 0, 0, 0, 0, 0]);
        // Its release is a key the report never had, and a new press is USB's.
        keys.release(A);
        assert_eq!(report.get(), [2, 0, 0, 0, 0, 0, 0, 0]);
        keys.press(A);
        assert_eq!(report.get(), [2, 0, A, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn a_bus_reset_goes_back_to_the_console() {
        let (mut keys, mut keyboard, _) = polled();
        keyboard.reset();
        assert!(!keys.active());
        keys.press(A);
        keys.release(A);
        assert_eq!(keys.report.get(), [0; 8]);
    }

    #[test]
    fn a_bus_reset_drops_the_keys_held_and_waiting_but_not_the_modifiers() {
        let (mut keys, mut keyboard, report) = polled();
        keys.press(LEFT_SHIFT);
        keys.press(A);
        keys.release(A);
        keys.press(B);
        keyboard.reset();
        keys.sync();
        assert_eq!(report.get(), [2, 0, 0, 0, 0, 0, 0, 0]);
        // Only the Shift is left: the A that is waiting (tapped) and the B that is held are gone.
        assert_eq!(keys.keys.len(), 1);
        assert!(is_modifier(keys.keys[0].usage) && keys.keys[0].down);
        keys.release(LEFT_SHIFT);
        assert_eq!(report.get(), [0; 8]);
    }

    /// Polled, with the LEDs as the firmware has set them.
    fn polled_with_leds(leds: u8) -> (UsbKeys, HidKeyboard, KeyReport) {
        let (keys, mut keyboard, report) = polled();
        set_leds(&mut keyboard, leds);
        (keys, keyboard, report)
    }

    const NOTHING: [[u8; 8]; 0] = [];

    #[test]
    fn a_lock_the_host_has_on_and_the_firmware_has_off_is_tapped_once() {
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        keys.follow_locks(true, Some(false));
        assert_eq!(
            delivered(&mut keys, &mut keyboard),
            [[0, 0, CAPS_LOCK, 0, 0, 0, 0, 0], [0; 8]]
        );
        // The firmware has not shown the lock yet, and it is not tapped again.
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
    }

    #[test]
    fn num_lock_follows_the_host_too() {
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        keys.follow_locks(false, Some(true));
        assert_eq!(
            delivered(&mut keys, &mut keyboard),
            [[0, 0, NUM_LOCK, 0, 0, 0, 0, 0], [0; 8]]
        );
    }

    #[test]
    fn a_lock_the_host_does_not_report_is_left_as_it_is() {
        let (mut keys, mut keyboard, _) = polled_with_leds(LED_NUM);
        keys.follow_locks(false, None);
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
        keys.check_locks();
        keys.follow_locks(true, None);
        assert_eq!(delivered(&mut keys, &mut keyboard).len(), 2);
    }

    #[test]
    fn a_lock_the_firmware_has_off_and_the_host_has_off_is_left() {
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        keys.follow_locks(false, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
    }

    #[test]
    fn a_lock_the_firmware_has_on_and_the_host_has_off_is_tapped() {
        let (mut keys, mut keyboard, _) = polled_with_leds(LED_NUM | LED_CAPS);
        keys.follow_locks(false, Some(true));
        assert_eq!(
            delivered(&mut keys, &mut keyboard),
            [[0, 0, CAPS_LOCK, 0, 0, 0, 0, 0], [0; 8]]
        );
    }

    #[test]
    fn locks_that_agree_are_left_alone() {
        let (mut keys, mut keyboard, _) = polled_with_leds(LED_NUM | LED_CAPS);
        keys.follow_locks(true, Some(true));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
    }

    #[test]
    fn the_locks_wait_for_the_firmware_to_set_the_leds() {
        // The firmware has polled and not set the LEDs yet, which is how it starts: they are 0
        // and are not the firmware's state. With the host's locks on nothing is tapped.
        let (mut keys, mut keyboard, report) = polled();
        keys.follow_locks(true, Some(true));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
        // The firmware shows both locks on, as it does with its saved options, and they agree.
        set_leds(&mut keyboard, LED_NUM | LED_CAPS);
        keys.follow_locks(true, Some(true));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
        assert_eq!(report.led_updates(), 1);
    }

    #[test]
    fn the_locks_stop_waiting_for_a_firmware_that_never_sets_the_leds() {
        let (mut keys, mut keyboard, _) = polled();
        for _ in 0..LED_WAIT_POLLS - 2 {
            poll(&mut keyboard);
        }
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
        // The poll that delivered() made, and one more, are the last it waits for.
        poll(&mut keyboard);
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard).len(), 2);
    }

    #[test]
    fn the_locks_wait_for_the_firmware_to_answer_a_lock_key_just_pressed() {
        // The user pressed Caps Lock and let go, and the window gets the focus back before the
        // firmware has shown it: the lock is not tapped a second time.
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        keys.press(CAPS_LOCK);
        keys.release(CAPS_LOCK);
        keys.check_locks();
        keys.follow_locks(true, Some(false));
        // Both reports of the key are sent, and the LED has not changed.
        assert_eq!(delivered(&mut keys, &mut keyboard).len(), 2);
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
        // The firmware's answer comes, and they agree.
        set_leds(&mut keyboard, LED_CAPS);
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
    }

    #[test]
    fn the_locks_wait_while_a_report_is_still_to_be_sent() {
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        keys.press(A);
        keys.release(A);
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard).len(), 2);
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard).len(), 2);
    }

    #[test]
    fn a_lock_key_the_user_holds_is_not_tapped() {
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        keys.press(CAPS_LOCK);
        delivered(&mut keys, &mut keyboard);
        set_leds(&mut keyboard, LED_CAPS);
        keys.check_locks();
        keys.follow_locks(false, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
        keys.release(CAPS_LOCK);
        assert_eq!(delivered(&mut keys, &mut keyboard), [[0; 8]]);
        // It is let go, and the lock that the firmware has on and the host has off is tapped.
        keys.follow_locks(false, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard).len(), 2);
    }

    #[test]
    fn the_locks_are_looked_at_again_when_asked() {
        let (mut keys, mut keyboard, _) = polled_with_leds(LED_CAPS);
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
        // The host's lock was toggled while the window was away, and it is taken up again.
        keys.follow_locks(false, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
        keys.check_locks();
        keys.follow_locks(false, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard).len(), 2);
    }

    #[test]
    fn the_locks_wait_for_the_firmware_to_poll() {
        let (mut keys, mut keyboard, _) = unpolled();
        keys.follow_locks(true, Some(false));
        assert_eq!(keys.report.get(), [0; 8]);
        poll(&mut keyboard);
        set_leds(&mut keyboard, 0);
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard).len(), 2);
    }

    #[test]
    fn the_locks_are_looked_at_again_after_a_reset() {
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        keys.follow_locks(false, Some(false));
        keyboard.reset();
        keys.follow_locks(true, Some(false));
        poll(&mut keyboard);
        // The configuration is gone with the reset, so the keyboard is not polled until the
        // firmware sets it up again.
        assert!(!keys.active());
        keyboard.setup([0x00, 0x09, 1, 0, 0, 0, 0, 0]);
        keyboard.in_token(0, 8);
        poll(&mut keyboard);
        set_leds(&mut keyboard, 0);
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard).len(), 2);
    }

    #[test]
    fn a_reset_between_two_looks_takes_the_lock_count_with_it() {
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        // Caps Lock tapped and handed over, and the keyboard reset before the firmware answered
        // and before the window looked at the locks.
        keys.press(CAPS_LOCK);
        keys.release(CAPS_LOCK);
        delivered(&mut keys, &mut keyboard);
        keyboard.reset();
        keyboard.setup([0x00, 0x09, 1, 0, 0, 0, 0, 0]);
        keyboard.in_token(0, 8);
        poll(&mut keyboard);
        // The firmware starts again with its own lock on, and the host's is off: decided at once.
        set_leds(&mut keyboard, LED_CAPS);
        keys.follow_locks(false, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard).len(), 2);
    }

    #[test]
    fn a_lock_key_pressed_after_a_reset_is_waited_for() {
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        keyboard.reset();
        keys.sync();
        keyboard.setup([0x00, 0x09, 1, 0, 0, 0, 0, 0]);
        keyboard.in_token(0, 8);
        poll(&mut keyboard);
        set_leds(&mut keyboard, 0);
        keys.press(CAPS_LOCK);
        keys.release(CAPS_LOCK);
        delivered(&mut keys, &mut keyboard);
        // The firmware has not answered it yet.
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
    }

    #[test]
    fn a_lock_key_reported_first_after_a_reset_is_answered_by_the_second_led_update() {
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        // Caps Lock is pressed and the keyboard reset before the firmware has it, so that the
        // first poll after the reset has it, and the window has not looked in between.
        keys.press(CAPS_LOCK);
        keys.release(CAPS_LOCK);
        keyboard.reset();
        keyboard.setup([0x00, 0x09, 1, 0, 0, 0, 0, 0]);
        keyboard.in_token(0, 8);
        poll(&mut keyboard);
        keys.sync();
        delivered(&mut keys, &mut keyboard);
        // The first update is the firmware's own start, and not its answer to the key.
        set_leds(&mut keyboard, 0);
        keys.follow_locks(true, Some(false));
        let reports = delivered(&mut keys, &mut keyboard);
        assert_eq!(reports, NOTHING);
        // Its answer is the second, the lock is on, and the host's is not: a tap.
        set_leds(&mut keyboard, LED_CAPS);
        keys.follow_locks(false, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard).len(), 2);
    }

    #[test]
    fn a_key_let_go_and_pressed_again_before_the_error_is_polled_is_typed_twice() {
        let (mut keys, mut keyboard, _) = polled();
        let mut reports: Vec<[u8; 8]> = Vec::new();
        let mut look = |keys: &mut UsbKeys, keyboard: &mut HidKeyboard| {
            if let In::Data(report) = poll(keyboard) {
                reports.push(report.try_into().unwrap());
            }
            keys.sync();
        };
        for usage in A..A + 6 {
            keys.press(usage);
            look(&mut keys, &mut keyboard);
        }
        // A comes up and goes down again, and a seventh key goes down, with no poll between: the
        // keyboard would have sent the report without A, and the firmware ignores the error.
        keys.release(A);
        keys.press(A);
        keys.press(0x0B);
        look(&mut keys, &mut keyboard);
        keys.release(0x0B);
        look(&mut keys, &mut keyboard);
        look(&mut keys, &mut keyboard);
        let typed = typed_keys(reports);
        assert_eq!(
            typed.iter().filter(|&&key| key == A).count(),
            2,
            "{typed:?}"
        );
    }

    #[test]
    fn a_tap_let_go_while_more_than_six_keys_are_down_needs_no_gap_before_it_is_pressed_again() {
        // Two key changes between polls, so outside the limits of the property test, shrunk from
        // it: the tap of 9 is let go with seven keys down, which the keyboard shows no report
        // for, and is typed once by both.
        let events = [
            (57, false),
            (6, false),
            (9, false),
            (4, true),
            (83, false),
            (9, false),
            (9, true),
            (7, false),
            (5, true),
            (8, false),
            (9, false),
            (9, false),
            (8, true),
            (57, true),
            (10, false),
        ];
        let (firmware, ideal) = typed_by_the_keys_and_by_a_1_ms_keyboard(&events);
        assert_eq!(firmware, ideal);
    }

    #[test]
    fn a_key_let_go_while_the_error_was_the_report_is_pressed_again_after_it_ends() {
        // A to F held, G and H seventh and eighth, A let go (seven down: the keyboard shows
        // nothing), H let go (six: it shows the report without A), and A pressed again before a
        // poll: typed twice.
        let events = [
            (4, true),
            (5, true),
            (6, true),
            (7, true),
            (8, true),
            (9, true),
            (10, true),
            (11, true),
            (4, true),
            (11, false),
            (4, false),
            (5, true),
        ];
        let (firmware, ideal) = typed_by_the_keys_and_by_a_1_ms_keyboard(&events);
        assert_eq!(firmware, ideal);
    }

    #[test]
    fn a_key_pressed_again_in_the_error_and_let_go_is_not_a_gap_for_the_key_the_firmware_holds() {
        // 4 is let go and pressed again while seven keys are down, then comes up for good: the
        // firmware holds the first 4 throughout and types nothing more (shrunk from the
        // property test with the limit of one key change between polls lifted).
        let events = [
            (11, false),
            (83, false),
            (4, false),
            (7, true),
            (10, false),
            (5, true),
            (12, true),
            (6, false),
            (4, false),
            (4, true),
            (6, false),
            (12, false),
            (4, true),
        ];
        let (firmware, ideal) = typed_by_the_keys_and_by_a_1_ms_keyboard(&events);
        assert_eq!(firmware, ideal);
    }

    #[test]
    fn a_tap_let_go_while_the_error_was_the_report_has_its_gap_when_the_error_ends() {
        // Shrunk from the property test with the limit of one key change between polls lifted:
        // 11 is tapped as a seventh key, and pressed again after the keys have come down to six.
        let events = [
            (8, false),
            (12, false),
            (7, false),
            (6, true),
            (4, false),
            (11, false),
            (10, false),
            (5, false),
            (11, true),
            (12, false),
            (11, false),
        ];
        let (firmware, ideal) = typed_by_the_keys_and_by_a_1_ms_keyboard(&events);
        assert_eq!(firmware, ideal);
    }

    #[test]
    fn a_later_press_that_is_dropped_does_not_take_the_gap_of_the_tap_before_it() {
        // 4 is tapped as the error begins, and a later 4 is pressed and let go while the error is
        // still the report, and dropped without a place: the first tap has its gap all the same.
        let events = [
            (5, true),
            (6, true),
            (7, true),
            (8, true),
            (9, true),
            (4, false),
            (10, false),
            (11, false),
            (4, false),
            (4, false),
            (4, false),
            (10, false),
            (11, true),
            (4, true),
            (4, true),
        ];
        let (firmware, ideal) = typed_by_the_keys_and_by_a_1_ms_keyboard(&events);
        // (11 is lost, outside the limits of the property test, and is not this test's business.)
        let fours = |typed: &Typed| typed.iter().filter(|&&(key, _)| key == 4).count();
        assert_eq!((fours(&firmware), fours(&ideal)), (2, 2), "{firmware:?}");
    }

    #[test]
    fn the_model_of_the_firmware_ignores_the_error() {
        let error = [0, 0, 1, 1, 1, 1, 1, 1];
        let a = [0, 0, A, 0, 0, 0, 0, 0];
        assert_eq!(typed_by_the_firmware([a, error, a]), [(A, 0)]);
        assert_eq!(
            typed_by_the_firmware([a, error, [0; 8], a]),
            [(A, 0), (A, 0)]
        );
    }

    #[test]
    fn caps_and_num_lock_in_the_first_report_after_a_start_are_answered_by_the_third_update() {
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        for usage in [CAPS_LOCK, NUM_LOCK] {
            keys.press(usage);
            keys.release(usage);
        }
        keyboard.reset();
        keyboard.setup([0x00, 0x09, 1, 0, 0, 0, 0, 0]);
        keyboard.in_token(0, 8);
        poll(&mut keyboard);
        keys.sync();
        delivered(&mut keys, &mut keyboard);
        // The firmware's own start, and then one update for the pair: 0, then 3 and 3.
        set_leds(&mut keyboard, 0);
        set_leds(&mut keyboard, LED_CAPS | LED_NUM);
        keys.follow_locks(false, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
        set_leds(&mut keyboard, LED_CAPS | LED_NUM);
        keys.follow_locks(false, Some(false));
        // Both are tapped, in one report.
        assert_eq!(
            delivered(&mut keys, &mut keyboard),
            [[0, 0, CAPS_LOCK, NUM_LOCK, 0, 0, 0, 0], [0; 8]]
        );
    }

    #[test]
    fn a_lock_key_in_the_first_report_is_not_given_a_start_if_the_firmware_has_set_the_leds() {
        // An update before the first poll is the firmware's start, and is over.
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        keys.press(CAPS_LOCK);
        keys.release(CAPS_LOCK);
        keyboard.reset();
        keyboard.setup([0x00, 0x09, 1, 0, 0, 0, 0, 0]);
        keyboard.in_token(0, 8);
        set_leds(&mut keyboard, 0);
        poll(&mut keyboard);
        keys.sync();
        delivered(&mut keys, &mut keyboard);
        set_leds(&mut keyboard, LED_CAPS);
        keys.follow_locks(false, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard).len(), 2);
    }

    #[test]
    fn a_lock_key_in_a_later_poll_is_not_given_a_start() {
        let (mut keys, mut keyboard, _) = polled();
        keys.press(CAPS_LOCK);
        keys.release(CAPS_LOCK);
        delivered(&mut keys, &mut keyboard);
        // No update yet, and the first one is the answer.
        set_leds(&mut keyboard, LED_CAPS);
        keys.follow_locks(false, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard).len(), 2);
    }

    #[test]
    fn altgr_as_control_and_alt_in_a_row_leaves_the_control_out_where_the_host_does_that() {
        let (mut keys, mut keyboard, _) = polled();
        keys.altgr_is_ctrl_alt = true;
        keys.press(LEFT_CTRL);
        keys.press(RIGHT_ALT);
        keys.press(A);
        let reports = delivered(&mut keys, &mut keyboard);
        assert_eq!(
            reports.last(),
            Some(&[0x40, 0, A, 0, 0, 0, 0, 0]),
            "{reports:?}"
        );
        assert!(
            reports.iter().all(|report| report[0] & 1 == 0),
            "{reports:?}"
        );
        // The Control comes up on its own, which it was not, and there is nothing to let go of.
        keys.release(LEFT_CTRL);
        keys.release(RIGHT_ALT);
        keys.release(A);
        assert_eq!(settled(&mut keys, &mut keyboard), [0; 8]);
    }

    #[test]
    fn a_control_and_an_alt_are_both_kept_where_the_host_does_not_do_that() {
        let (mut keys, mut keyboard, _) = polled();
        keys.altgr_is_ctrl_alt = false;
        keys.press(LEFT_CTRL);
        keys.press(RIGHT_ALT);
        assert_eq!(
            settled(&mut keys, &mut keyboard),
            [0x41, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn a_control_the_firmware_has_had_is_a_real_one() {
        let (mut keys, mut keyboard, _) = polled();
        keys.altgr_is_ctrl_alt = true;
        keys.press(LEFT_CTRL);
        delivered(&mut keys, &mut keyboard);
        keys.press(RIGHT_ALT);
        assert_eq!(
            settled(&mut keys, &mut keyboard),
            [0x41, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn a_control_that_was_not_the_last_key_down_is_a_real_one() {
        let (mut keys, mut keyboard, _) = polled();
        keys.altgr_is_ctrl_alt = true;
        keys.press(LEFT_CTRL);
        keys.press(A);
        keys.press(RIGHT_ALT);
        assert_eq!(
            settled(&mut keys, &mut keyboard),
            [0x41, 0, A, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn two_lock_keys_in_a_batch_are_two_answers_to_wait_for() {
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        for _ in 0..2 {
            keys.press(CAPS_LOCK);
            keys.release(CAPS_LOCK);
        }
        delivered(&mut keys, &mut keyboard);
        // The firmware has answered one of them, and not the other: the LED shows the first, and
        // the host's lock is off, which would be a tap if it were the firmware's state.
        set_leds(&mut keyboard, LED_CAPS);
        keys.follow_locks(false, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard), NOTHING);
        // Both have been answered: the LED is off, the host's lock is on, and it is tapped.
        set_leds(&mut keyboard, 0);
        keys.follow_locks(true, Some(false));
        assert_eq!(delivered(&mut keys, &mut keyboard).len(), 2);
    }

    #[test]
    fn a_lock_key_pressed_before_a_reset_does_not_underflow_the_polls() {
        let (mut keys, mut keyboard, _) = polled_with_leds(0);
        for _ in 0..10 {
            poll(&mut keyboard);
        }
        keys.press(CAPS_LOCK);
        keys.release(CAPS_LOCK);
        keyboard.reset();
        keyboard.setup([0x00, 0x09, 1, 0, 0, 0, 0, 0]);
        keyboard.in_token(0, 8);
        // Fewer polls than there were at the key, with no look in between to forget it.
        poll(&mut keyboard);
        set_leds(&mut keyboard, 0);
        keys.follow_locks(true, Some(false));
    }

    #[test]
    fn the_locks_wait_while_the_report_that_is_set_has_not_been_sent() {
        let (mut keys, mut keyboard, report) = polled_with_leds(0);
        keys.press(A);
        delivered(&mut keys, &mut keyboard);
        // A is let go: no key is waiting to be sent, and the report without it has not been polled.
        keys.release(A);
        assert!(!keys.pending() && !report.was_sent());
        keys.follow_locks(true, Some(false));
        assert_eq!(report.get(), [0; 8]);
        poll(&mut keyboard);
        keys.follow_locks(true, Some(false));
        assert_eq!(report.get(), [0, 0, CAPS_LOCK, 0, 0, 0, 0, 0]);
    }

    // A small generator, so the sequences are the same every run.
    struct Random(u64);

    impl Random {
        fn below(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }

    #[test]
    fn every_keystroke_is_typed_in_order_when_keys_are_not_pressed_faster_than_they_are_polled() {
        const USAGES: [u8; 6] = [A, B, 0x06, LEFT_SHIFT, 0xE0, 0xE2];
        let mut random = Random(0x9E37_79B9_7F4A_7C15);
        for round in 0..3000 {
            let (mut keys, mut keyboard, _) = polled();
            let mut firmware = Vec::new();
            let mut last = [0; 8];
            let mut truth = Vec::new();
            let mut down: Vec<u8> = Vec::new();
            // At most one key goes down between two polls, but modifiers and releases are free.
            let mut key_this_poll = false;
            for _ in 0..40 {
                let usage = USAGES[random.below(USAGES.len() as u64) as usize];
                if down.contains(&usage) {
                    down.retain(|&held| held != usage);
                    keys.release(usage);
                } else if !is_modifier(usage) && key_this_poll {
                    // Too soon for another key: the firmware polls first.
                    firmware.extend(hear(&mut last, poll(&mut keyboard)));
                    keys.sync();
                    key_this_poll = false;
                    continue;
                } else {
                    if !is_modifier(usage) {
                        let modifiers = down
                            .iter()
                            .filter(|&&held| is_modifier(held))
                            .fold(0, |bits, &held| bits | 1 << (held - FIRST_MODIFIER));
                        truth.push((usage, modifiers));
                        key_this_poll = true;
                    }
                    down.push(usage);
                    keys.press(usage);
                }
                // The firmware polls now and then, in between the user's events.
                if random.below(3) == 0 {
                    firmware.extend(hear(&mut last, poll(&mut keyboard)));
                    keys.sync();
                    key_this_poll = false;
                }
            }
            for usage in down {
                keys.release(usage);
            }
            for _ in 0..20 {
                firmware.extend(hear(&mut last, poll(&mut keyboard)));
                keys.sync();
            }
            assert_eq!(firmware, truth, "round {round}");
        }
    }

    /// The report a keyboard polled every millisecond sends for the keys that are down: the
    /// modifiers, and up to six keys in the order they went down or the error in every slot.
    fn ideal_report(down: &[u8]) -> [u8; 8] {
        let mut report = [0; 8];
        let mut keys = Vec::new();
        for &usage in down {
            if is_modifier(usage) {
                report[0] |= 1 << (usage - FIRST_MODIFIER);
            } else {
                keys.push(usage);
            }
        }
        if keys.len() > SLOTS {
            report[2..].fill(ROLLOVER);
        } else {
            report[2..2 + keys.len()].copy_from_slice(&keys);
        }
        report
    }

    /// The keys a firmware typed, with the modifiers of each.
    type Typed = Vec<(u8, u8)>;

    /// What the firmware types from the keys, and from a keyboard polled every millisecond, for a
    /// run of events: each is a key toggled (down if it was up) and whether the keys are polled
    /// after it, and then the keys are let go one at a time. The 1 ms keyboard is polled after
    /// every event.
    fn typed_by_the_keys_and_by_a_1_ms_keyboard(events: &[(u8, bool)]) -> (Typed, Typed) {
        let (mut keys, mut keyboard, _) = polled();
        let (mut firmware, mut ideal) = (Vec::new(), Vec::new());
        let (mut firmware_last, mut ideal_last) = ([0; 8], [0; 8]);
        let mut down: Vec<u8> = Vec::new();
        for &(usage, polled) in events {
            if down.contains(&usage) {
                down.retain(|&held| held != usage);
                keys.release(usage);
            } else {
                down.push(usage);
                keys.press(usage);
            }
            ideal.extend(hear(
                &mut ideal_last,
                In::Data(ideal_report(&down).to_vec()),
            ));
            if polled {
                firmware.extend(hear(&mut firmware_last, poll(&mut keyboard)));
                keys.sync();
            }
        }
        // The last change is seen before the keys are let go.
        firmware.extend(hear(&mut firmware_last, poll(&mut keyboard)));
        keys.sync();
        while !down.is_empty() {
            let usage = down.remove(0);
            keys.release(usage);
            ideal.extend(hear(
                &mut ideal_last,
                In::Data(ideal_report(&down).to_vec()),
            ));
            firmware.extend(hear(&mut firmware_last, poll(&mut keyboard)));
            keys.sync();
        }
        for _ in 0..20 {
            firmware.extend(hear(&mut firmware_last, poll(&mut keyboard)));
            keys.sync();
        }
        (firmware, ideal)
    }

    /// What a run of events may do between two polls of the keys. Each is allowed when true.
    #[derive(Clone, Copy)]
    struct Limits {
        /// Press a modifier after a key has changed: the window takes it to the next report, a
        /// poll later than a 1 ms keyboard.
        modifier_after_key: bool,
        /// Change a modifier twice, which a 1 ms keyboard would have seen go down and up.
        modifier_twice: bool,
        /// Change two keys.
        two_keys: bool,
    }

    const STRICT: Limits = Limits {
        modifier_after_key: false,
        modifier_twice: false,
        two_keys: false,
    };

    /// Whether a run of events keeps to `limits` between its polls of the keys.
    fn changes_are_seen_in_time(events: &[(u8, bool)], limits: Limits) -> bool {
        let (mut changed, mut down): (Vec<u8>, Vec<u8>) = (Vec::new(), Vec::new());
        for &(usage, polled) in events {
            let pressed = !down.contains(&usage);
            if pressed {
                down.push(usage);
            } else {
                down.retain(|&held| held != usage);
            }
            let twice = changed.contains(&usage) && is_modifier(usage);
            let two_keys = !is_modifier(usage) && changed.iter().any(|&other| !is_modifier(other));
            let after_key =
                is_modifier(usage) && pressed && changed.iter().any(|&other| !is_modifier(other));
            // A key is not changed twice at all: that is a tap between two polls, which the
            // window keeps with its modifiers, and which is the other limit's business.
            let key_twice = changed.contains(&usage) && !is_modifier(usage);
            if twice && !limits.modifier_twice
                || two_keys && !limits.two_keys
                || key_twice && !limits.two_keys
                || after_key && !limits.modifier_after_key
            {
                return false;
            }
            if polled {
                changed.clear();
            } else {
                changed.push(usage);
            }
        }
        true
    }

    /// How the keys typed by the window differ from those typed for a 1 ms keyboard.
    #[derive(Default)]
    struct Kinds {
        /// Only some of the keys the keyboard gives, counting repeats: keys lost.
        lost: u32,
        /// The same keys, with other modifiers.
        modifiers: u32,
        /// The same keys in another order.
        order: u32,
        /// More keys than the keyboard gives.
        extra: u32,
        /// Some lost and some extra.
        mixed: u32,
        /// The rounds that differ, for comparing two versions of the keys.
        rounds: Vec<u32>,
    }

    impl Kinds {
        fn total(&self) -> u32 {
            self.lost + self.modifiers + self.order + self.extra + self.mixed
        }

        fn count(&mut self, firmware: &Typed, ideal: &Typed) {
            let keys = |typed: &Typed| typed.iter().map(|&(key, _)| key).collect::<Vec<u8>>();
            let (f, i) = (keys(firmware), keys(ideal));
            let sorted = |mut keys: Vec<u8>| {
                keys.sort_unstable();
                keys
            };
            // Whether every key of `a` is in `b`, counting repeats.
            let within = |a: &Vec<u8>, b: &Vec<u8>| {
                let mut rest = b.clone();
                a.iter().all(|key| {
                    rest.iter()
                        .position(|other| other == key)
                        .map(|at| rest.remove(at))
                        .is_some()
                })
            };
            if f == i {
                self.modifiers += 1;
            } else if sorted(f.clone()) == sorted(i.clone()) {
                self.order += 1;
            } else if within(&f, &i) {
                self.lost += 1;
            } else if within(&i, &f) {
                self.extra += 1;
            } else {
                self.mixed += 1;
            }
        }
    }

    /// How many of `rounds` random runs of 60 changes of nine keys, the two lock keys and two
    /// modifiers, that keep to `limits`, the firmware types differently from the keys than from a
    /// 1 ms keyboard. The first is printed, as short as it can be made.
    fn mismatches_with(limits: Limits, rounds: u32) -> Kinds {
        const USAGES: [u8; 13] = [
            A, B, 6, 7, 8, 9, 10, 11, 12, CAPS_LOCK, NUM_LOCK, LEFT_SHIFT, 0xE0,
        ];
        let mut random = Random(0x2545_F491_4F6C_DD1D);
        let mut kinds = Kinds::default();
        let mut shown = false;
        for round in 0..rounds {
            let mut events: Vec<(u8, bool)> = Vec::new();
            for _ in 0..60 {
                let usage = USAGES[random.below(USAGES.len() as u64) as usize];
                events.push((usage, random.below(3) == 0));
                if !changes_are_seen_in_time(&events, limits) {
                    // Too soon: the keys are polled first.
                    let last = events.len() - 2;
                    events[last].1 = true;
                }
            }
            let (firmware, ideal) = typed_by_the_keys_and_by_a_1_ms_keyboard(&events);
            if firmware == ideal {
                continue;
            }
            kinds.count(&firmware, &ideal);
            kinds.rounds.push(round);
            if !shown {
                shown = true;
                let differs = |events: &[(u8, bool)]| {
                    let (firmware, ideal) = typed_by_the_keys_and_by_a_1_ms_keyboard(events);
                    firmware != ideal
                };
                let mut shrunk = true;
                while shrunk {
                    shrunk = false;
                    for width in [1, 2, 3] {
                        let mut at = 0;
                        while at + width <= events.len() {
                            let mut shorter = events.clone();
                            shorter.drain(at..at + width);
                            if changes_are_seen_in_time(&shorter, limits) && differs(&shorter) {
                                events = shorter;
                                shrunk = true;
                            } else {
                                at += 1;
                            }
                        }
                    }
                }
                let (firmware, ideal) = typed_by_the_keys_and_by_a_1_ms_keyboard(&events);
                eprintln!("round {round}: {events:?}\n{firmware:?}\n{ideal:?}");
            }
        }
        kinds
    }

    #[test]
    fn what_the_firmware_types_is_what_a_keyboard_polled_every_millisecond_gives_it() {
        // At most one key changes between two polls of the keys, as with a 1 ms keyboard and a
        // person, and a modifier is not changed twice or pressed after a key before a poll.
        assert_eq!(mismatches_with(STRICT, 20_000).total(), 0);
    }

    #[test]
    #[ignore = "prints how often the keys differ from a 1 ms keyboard outside the limits"]
    fn how_often_the_keys_differ_from_a_1_ms_keyboard_outside_the_limits() {
        for (name, limits) in [
            (
                "modifier pressed after a key",
                Limits {
                    modifier_after_key: true,
                    ..STRICT
                },
            ),
            (
                "modifier changed twice",
                Limits {
                    modifier_twice: true,
                    ..STRICT
                },
            ),
            (
                "two key changes",
                Limits {
                    two_keys: true,
                    ..STRICT
                },
            ),
        ] {
            let kinds = mismatches_with(limits, 20_000);
            println!(
                "{name}: {} of 20000 (lost {}, modifiers {}, order {}, extra {}, mixed {})",
                kinds.total(),
                kinds.lost,
                kinds.modifiers,
                kinds.order,
                kinds.extra,
                kinds.mixed
            );
            // For comparing two versions: CMM2_FUZZ_OUT=<file> lists the rounds that differ.
            if let Ok(path) = std::env::var("CMM2_FUZZ_OUT") {
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .unwrap();
                writeln!(file, "{name}: {:?}", kinds.rounds).unwrap();
            }
        }
    }

    // What the firmware types from a poll, given the report it had before. A rollover error is
    // ignored: the firmware keeps the report it had (checked on the real firmware, see
    // `real_firmware_ignores_the_rollover_error_over_usb`).
    fn hear(last: &mut [u8; 8], reply: In) -> Vec<(u8, u8)> {
        let In::Data(report) = reply else {
            return Vec::new();
        };
        let report: [u8; 8] = report.try_into().unwrap();
        if report[2] == ROLLOVER {
            return Vec::new();
        }
        let typed = report[2..]
            .iter()
            .filter(|&&key| key > ROLLOVER && !last[2..].contains(&key))
            .map(|&key| (key, report[0]))
            .collect();
        *last = report;
        typed
    }
}

/// The keys the window would send, typed into the real firmware over USB: what `UsbKeys` makes of
/// the key events is what the firmware's prompt, `KEYDOWN()` and program break take. They need the
/// firmware and are run with `CMM2_FIRMWARE=<path> cargo test --release -- --ignored`.
#[cfg(test)]
mod real_firmware {
    use super::*;
    use cmm2_core::{Console, Cpu, Firmware, HidKeyboard, Irqs, Stop, cmm2_memory};
    use std::time::Instant;

    const PROMPT: &str = "\r\n> ";

    /// How many instructions run between looks at the keys, as the window looks every 16 ms
    /// of wall time, and about 3 million instructions are 10 ms of the machine's.
    const SLICE: u64 = 3_000_000;

    /// The firmware at its prompt with the keyboard polling, and the keys that drive it.
    struct Board {
        cpu: Cpu,
        console: Console,
        keys: UsbKeys,
    }

    impl Board {
        /// Boots as the window does: the first-boot questions are answered over the console,
        /// as no USB keyboard is up for them, and then the machine runs until the firmware
        /// polls the keyboard.
        fn boot() -> Board {
            let path = std::env::var("CMM2_FIRMWARE")
                .expect("CMM2_FIRMWARE must be set to the path of CMM2_MMBasic_7.00.00b5.bin");
            let firmware = Firmware::load(path).unwrap();
            let irqs = Irqs::new();
            let memory = cmm2_memory(&irqs);
            let console = memory.console();
            let report = KeyReport::new();
            memory.usb_port().attach(HidKeyboard::new(report.clone()));
            let mut board = Board {
                cpu: Cpu::new(&firmware, memory, irqs).unwrap(),
                console,
                keys: UsbKeys::new(report),
            };
            board.answer("Keyboard type", "1", 300_000_000);
            board.answer("HH:MM:SS", "12:00:00\r", 20_000_000);
            board.answer("DD/MM/YY", "12/11/26\r", 20_000_000);
            while board.cpu.resets() == 0 {
                board.run(1_000_000);
            }
            board.until(PROMPT, 500_000_000);
            let (prompt_at, prompt_instructions) = (Instant::now(), board.cpu.instruction_count());
            assert!(!board.keys.active());
            while !board.keys.active() {
                assert!(
                    board.cpu.instruction_count() - prompt_instructions < 1_500_000_000,
                    "the firmware never polled the keyboard"
                );
                board.run(1_000_000);
            }
            println!(
                "USB keys work {:.1} s after the prompt ({} million instructions)",
                prompt_at.elapsed().as_secs_f64(),
                (board.cpu.instruction_count() - prompt_instructions) / 1_000_000
            );
            board
        }

        fn run(&mut self, count: u64) -> String {
            let stop = self.cpu.run(count);
            assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");
            String::from_utf8_lossy(&self.console.take_output()).into_owned()
        }

        /// Runs until the output has `text` in it, or `cap` instructions have run.
        fn until(&mut self, text: &str, cap: u64) -> String {
            let limit = self.cpu.instruction_count() + cap;
            let mut output = String::new();
            while !output.contains(text) && self.cpu.instruction_count() < limit {
                output.push_str(&self.run(1_000_000));
            }
            assert!(output.contains(text), "no {text:?} in {output:?}");
            output
        }

        fn answer(&mut self, text: &str, keys: &str, cap: u64) {
            self.until(text, cap);
            self.console.push_input(keys.as_bytes());
        }

        /// Types `CPU RESTART` and runs until the chip has been reset. The firmware restarts at
        /// the Return, before it polls the release.
        fn restart(&mut self) {
            self.type_text("CPU RESTART");
            self.press(Scancode::Return);
            self.release(Scancode::Return);
            let restarts = self.cpu.resets();
            let limit = self.cpu.instruction_count() + 100_000_000;
            while self.cpu.resets() == restarts {
                assert!(self.cpu.instruction_count() < limit, "no restart");
                self.tick();
            }
        }

        fn run_until_polled(&mut self) {
            let limit = self.cpu.instruction_count() + 1_500_000_000;
            while !self.keys.active() {
                assert!(
                    self.cpu.instruction_count() < limit,
                    "no poll after the restart"
                );
                self.tick();
            }
        }

        /// A slice and the window's look at the keys, with the host's locks as given.
        fn look(&mut self, caps: bool, num: Option<bool>) {
            self.tick();
            self.keys.follow_locks(caps, num);
        }

        /// Runs a slice, as the emulator does between two looks of the window.
        fn tick(&mut self) -> String {
            let output = self.run(SLICE);
            self.keys.sync();
            output
        }

        fn press(&mut self, scancode: Scancode) {
            self.keys.press(usage(scancode).unwrap());
        }

        fn release(&mut self, scancode: Scancode) {
            self.keys.release(usage(scancode).unwrap());
        }

        /// Runs until the keys have nothing more to send the firmware, and a little more, as
        /// the firmware types a key a little after it polls.
        fn settle(&mut self) -> String {
            let mut output = String::new();
            for _ in 0..400 {
                output += &self.tick();
                if !self.keys.pending() && self.keys.report.was_sent() {
                    break;
                }
            }
            assert!(
                !self.keys.pending() && self.keys.report.was_sent(),
                "the firmware did not poll"
            );
            output += &self.tick();
            output
        }

        /// Sets the report as it is, which the firmware is sent at its next poll, and runs until
        /// it has been and a little more. Returns what the firmware printed. The keys are not
        /// looked at in this time, so the report stays as set.
        fn send(&mut self, report: [u8; 8]) -> String {
            self.keys.report.set(report);
            let mut output = String::new();
            for _ in 0..400 {
                output += &self.run(SLICE);
                if self.keys.report.was_sent() {
                    break;
                }
            }
            assert!(self.keys.report.was_sent(), "the firmware did not poll");
            output + &self.run(SLICE)
        }

        /// Presses and lets go at once, with no poll between: the firmware has to be sent the
        /// key all the same.
        fn tap(&mut self, scancode: Scancode) -> String {
            self.press(scancode);
            self.release(scancode);
            self.settle()
        }

        /// Taps a key with a modifier as quickly as it can be done: the modifier goes down
        /// with the key and up before it, with no poll between any of them, and the firmware
        /// types the key with the modifiers it had when the key went down.
        fn tap_with(&mut self, modifier: Scancode, scancode: Scancode) -> String {
            self.press(modifier);
            self.press(scancode);
            self.release(modifier);
            self.release(scancode);
            self.settle()
        }

        /// Types text of letters, digits, space and the few marks used here, with Shift for the
        /// ones that need it, as a UK keyboard does. Returns what the firmware printed.
        fn type_text(&mut self, text: &str) -> String {
            let mut output = String::new();
            for character in text.chars() {
                let (scancode, shift) = match character {
                    'A'..='Z' => (letter(character), true),
                    'a'..='z' => (letter(character.to_ascii_uppercase()), false),
                    ' ' => (Scancode::Space, false),
                    ',' => (Scancode::Comma, false),
                    '0'..='9' => (digit(character), false),
                    '"' => (Scancode::Num2, true),
                    '+' => (Scancode::Equals, true),
                    '=' => (Scancode::Equals, false),
                    '$' => (Scancode::Num4, true),
                    '(' => (Scancode::Num9, true),
                    ')' => (Scancode::Num0, true),
                    ':' => (Scancode::Semicolon, true),
                    '<' => (Scancode::Comma, true),
                    '>' => (Scancode::Period, true),
                    _ => panic!("no key for {character:?}"),
                };
                output += &if shift {
                    self.tap_with(Scancode::LShift, scancode)
                } else {
                    self.tap(scancode)
                };
            }
            output
        }

        /// Presses Return and runs to the next prompt. Returns what was printed on the way.
        fn enter(&mut self) -> String {
            let mut output = self.tap(Scancode::Return);
            let limit = self.cpu.instruction_count() + 200_000_000;
            while !output.ends_with(PROMPT) {
                assert!(self.cpu.instruction_count() < limit, "{output:?}");
                output += &self.tick();
            }
            output
        }
    }

    fn digit(digit: char) -> Scancode {
        const DIGITS: [Scancode; 10] = [
            Scancode::Num0,
            Scancode::Num1,
            Scancode::Num2,
            Scancode::Num3,
            Scancode::Num4,
            Scancode::Num5,
            Scancode::Num6,
            Scancode::Num7,
            Scancode::Num8,
            Scancode::Num9,
        ];
        DIGITS[(digit as u8 - b'0') as usize]
    }

    fn letter(upper: char) -> Scancode {
        const LETTERS: [Scancode; 26] = [
            Scancode::A,
            Scancode::B,
            Scancode::C,
            Scancode::D,
            Scancode::E,
            Scancode::F,
            Scancode::G,
            Scancode::H,
            Scancode::I,
            Scancode::J,
            Scancode::K,
            Scancode::L,
            Scancode::M,
            Scancode::N,
            Scancode::O,
            Scancode::P,
            Scancode::Q,
            Scancode::R,
            Scancode::S,
            Scancode::T,
            Scancode::U,
            Scancode::V,
            Scancode::W,
            Scancode::X,
            Scancode::Y,
            Scancode::Z,
        ];
        LETTERS[(upper as u8 - b'A') as usize]
    }

    // Caps Lock and Num Lock together in the first report after the firmware starts to poll are
    // answered by one more update, not two: the LEDs are set three times in all, the first with
    // the firmware's own state and then twice with both locks on, and no more after that.
    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
    fn real_firmware_sets_the_leds_three_times_for_caps_and_num_lock_in_its_first_report_over_usb()
    {
        let mut board = Board::boot();
        board.restart();
        board
            .keys
            .report
            .set([0, 0, CAPS_LOCK, NUM_LOCK, 0, 0, 0, 0]);
        let report = board.keys.report.clone();
        let mut changes = Vec::new();
        while report.polls() <= 400 {
            board.run(10_000);
            let now = (report.led_updates(), report.leds());
            if changes.last() != Some(&now) && now.0 > 0 {
                changes.push(now);
            }
        }
        assert_eq!(report.polls_at_report(), 1, "the first poll had the keys");
        assert_eq!(report.led_updates_at_report(), 0);
        assert_eq!(
            changes,
            [(1, 0), (2, LED_CAPS | LED_NUM), (3, LED_CAPS | LED_NUM)]
        );
    }

    // The firmware ignores a report with the rollover error in the key slots and goes on with the
    // report it had, which the model of the firmware in the tests above relies on.
    // A Caps Lock in the first report after the firmware starts to poll is answered by the second
    // update of the LEDs: the first is its own, the state it starts with.
    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
    fn real_firmware_answers_a_lock_key_in_its_first_report_with_the_second_led_update_over_usb() {
        let mut board = Board::boot();
        board.restart();
        // The keys are not looked at, so the report stays as it is until the firmware polls.
        board.keys.report.set([0, 0, CAPS_LOCK, 0, 0, 0, 0, 0]);
        let report = board.keys.report.clone();
        let mut first_leds = None;
        for _ in 0..300_000 {
            board.run(10_000);
            if report.led_updates() == 1 && first_leds.is_none() {
                first_leds = Some(report.leds());
            }
            if report.led_updates() >= 2 {
                break;
            }
        }
        assert_eq!(report.polls_at_report(), 1, "the first poll had the key");
        assert_eq!(report.led_updates_at_report(), 0);
        // The first update is the firmware's own state, all off, and then the lock is shown.
        assert_eq!(first_leds, Some(0));
        assert_eq!(report.led_updates(), 2);
        assert_eq!(report.leds() & LED_CAPS, LED_CAPS);
    }

    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
    fn real_firmware_ignores_the_rollover_error_over_usb() {
        const ERROR: [u8; 8] = [0, 0, 1, 1, 1, 1, 1, 1];
        let report = |modifiers: u8, keys: &[u8]| {
            let mut report = [modifiers, 0, 0, 0, 0, 0, 0, 0];
            report[2..2 + keys.len()].copy_from_slice(keys);
            report
        };
        let mut board = Board::boot();
        let mut typed = |reports: &[[u8; 8]]| {
            let mut output = String::new();
            for &report in reports {
                output += &board.send(report);
            }
            output + &board.send([0; 8])
        };
        // The same key before and after it is one keystroke.
        assert_eq!(typed(&[report(0, &[4]), ERROR, report(0, &[4])]), "a");
        // Six keys, the error, and five of the six with a new one: the new one is typed, and
        // the five are not typed again.
        let six = [4, 5, 6, 7, 8, 9];
        let more = [5, 6, 7, 8, 9, 10];
        assert_eq!(
            typed(&[report(0, &six), ERROR, report(0, &more)]),
            "abcdefg"
        );
        // A Shift in the report after the error does not retype the key.
        assert_eq!(typed(&[report(0, &[4]), ERROR, report(2, &[4])]), "a");
        assert_eq!(typed(&[report(0, &six), ERROR, report(0, &six)]), "abcdef");
    }

    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
    fn real_firmware_takes_typing_at_the_prompt_over_usb() {
        let mut board = Board::boot();
        board.type_text("PRINT 1+2");
        let answer = board.enter();
        assert!(answer.ends_with("\r\n 3\r\n> "), "{answer:?}");

        // The firmware's layout makes the characters: Shift with 2 is the quote on its UK
        // layout, and Shift with a letter is the capital.
        board.type_text("PRINT \"Ab\"");
        let answer = board.enter();
        assert!(answer.ends_with("\r\nAb\r\n> "), "{answer:?}");
    }

    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
    fn real_firmware_reads_a_held_key_in_a_running_loop_over_usb() {
        let mut board = Board::boot();
        // The loop ends when 'a' is down, and then when no key is.
        board.type_text(
            "DO WHILE KEYDOWN(1)<>97:LOOP:PRINT KEYDOWN(0):DO WHILE KEYDOWN(0)>0:LOOP:PRINT 0",
        );
        board.tap(Scancode::Return);
        // It runs and does not finish with nothing held.
        for _ in 0..5 {
            let output = board.tick();
            assert!(!output.contains(PROMPT), "{output:?}");
        }
        board.press(Scancode::A);
        let mut output = String::new();
        for _ in 0..10 {
            output += &board.tick();
        }
        assert!(output.contains(" 1\r\n"), "{output:?}");
        // Still held, the second loop has not ended.
        assert!(!output.ends_with(PROMPT), "{output:?}");
        board.release(Scancode::A);
        for _ in 0..10 {
            output += &board.tick();
        }
        assert!(output.ends_with("\r\n 0\r\n> "), "{output:?}");
    }

    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
    fn real_firmware_stops_a_running_program_on_ctrl_c_over_usb() {
        let mut board = Board::boot();
        board.type_text("FOR I = 1 TO 1000000000 : NEXT");
        board.tap(Scancode::Return);
        let running = board.tick();
        assert!(!running.contains(PROMPT), "{running:?}");

        let mut stopped = board.tap_with(Scancode::LCtrl, Scancode::C);
        let limit = board.cpu.instruction_count() + 100_000_000;
        while !stopped.ends_with(PROMPT) {
            assert!(board.cpu.instruction_count() < limit, "{stopped:?}");
            stopped += &board.tick();
        }
        board.type_text("PRINT 1+2");
        let answer = board.enter();
        assert!(answer.ends_with("\r\n 3\r\n> "), "{answer:?}");
    }

    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
    fn real_firmware_types_a_capital_when_shift_comes_up_with_the_letter_over_usb() {
        let mut board = Board::boot();
        // Shift is held across a poll, and then A goes down, Shift comes up and A comes up
        // before the next one: the firmware has to be sent the A while the Shift is down.
        board.press(Scancode::LShift);
        let mut output = board.tick();
        board.press(Scancode::A);
        board.release(Scancode::LShift);
        board.release(Scancode::A);
        output += &board.settle();
        assert!(output.contains('A') && !output.contains('a'), "{output:?}");
        // And a Shift and a letter that both go down and up between two polls.
        let output = board.tap_with(Scancode::LShift, Scancode::B);
        assert!(output.contains('B') && !output.contains('b'), "{output:?}");
    }

    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
    fn real_firmware_types_the_same_key_twice_between_two_polls_over_usb() {
        let mut board = Board::boot();
        for (scancode, text) in [(Scancode::L, "l"), (Scancode::S, "s")] {
            board.press(scancode);
            board.release(scancode);
            board.press(scancode);
            board.release(scancode);
            let output = board.settle();
            assert_eq!(output.matches(text).count(), 2, "{output:?}");
        }
    }

    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
    fn real_firmware_asks_for_the_console_keys_again_after_a_restart_over_usb() {
        let mut board = Board::boot();
        board.restart();
        // The USB stack is not up again yet: the keys are the console's until the firmware polls.
        board.tick();
        assert!(!board.keys.active());
        assert!(!board.keys.report.polled());
        board.run_until_polled();
        board.type_text("PRINT 1+2");
        let answer = board.enter();
        assert!(answer.ends_with("\r\n 3\r\n> "), "{answer:?}");
    }

    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
    fn real_firmware_follows_a_caps_lock_the_host_had_on_over_usb() {
        let mut board = Board::boot();
        // The host's caps lock was on when the window opened. The window looks at every slice,
        // and the lock is only compared once the firmware has set the LEDs.
        for _ in 0..40 {
            board.look(true, Some(false));
        }
        assert_ne!(board.keys.report.leds() & LED_CAPS, 0);
        let output = board.tap(Scancode::A);
        assert!(output.contains('A') && !output.contains('a'), "{output:?}");
        // Nothing more to do while they agree.
        board.look(true, Some(false));
        assert!(!board.keys.pending());
        // And off again at the host.
        board.keys.check_locks();
        for _ in 0..40 {
            board.look(false, Some(false));
        }
        assert_eq!(board.keys.report.leds() & LED_CAPS, 0);
        let output = board.tap(Scancode::A);
        assert!(output.contains('a') && !output.contains('A'), "{output:?}");
    }

    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
    fn real_firmware_keeps_the_locks_it_starts_with_when_the_host_has_them_on_over_usb() {
        let mut board = Board::boot();
        // The saved options have both locks on. The firmware sets the LEDs a little after it
        // first polls, so a comparison before that would see them off and tap them to off.
        board.type_text("OPTION KEYBOARD UK,1,1");
        board.enter();
        board.restart();
        // The window looks at the keys once a slice, and a short slice is as likely as a long one
        // to end just after the first poll.
        let limit = board.cpu.instruction_count() + 1_500_000_000;
        while !board.keys.active() {
            assert!(
                board.cpu.instruction_count() < limit,
                "no poll after the restart"
            );
            board.run(100_000);
            board.keys.sync();
            board.keys.follow_locks(true, Some(true));
        }
        for _ in 0..20 {
            board.run(100_000);
            board.keys.sync();
            board.keys.follow_locks(true, Some(true));
        }
        println!(
            "after the first poll: LEDs {:#b}, {} updates, {} polls",
            board.keys.report.leds(),
            board.keys.report.led_updates(),
            board.keys.report.polls()
        );
        for _ in 0..100 {
            board.look(true, Some(true));
        }
        assert_eq!(
            board.keys.report.leds() & (LED_CAPS | LED_NUM),
            LED_CAPS | LED_NUM
        );
        let output = board.tap(Scancode::A);
        assert!(output.contains('A') && !output.contains('a'), "{output:?}");
    }

    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
    fn real_firmware_sees_a_held_key_in_every_poll_of_a_long_hold_over_usb() {
        let mut board = Board::boot();
        // 'seen' is set once A is the key down, and 'lost' counts the readings after that when it
        // is not: with the key held for the whole loop there are none.
        board.type_text(
            "seen=0:lost=0:FOR i=1 TO 40:PAUSE 20:k=(KEYDOWN(1)=97):lost=lost+((seen=1) AND (k=0)):seen=seen OR k:NEXT",
        );
        board.type_text(":PRINT STR$(seen)+\",\"+STR$(lost)");
        let mut output = board.tap(Scancode::Return);
        board.press(Scancode::A);
        // The loop is about 0.8 s of machine time, and the key is held for 1.5 s.
        for _ in 0..150 {
            output += &board.tick();
        }
        board.release(Scancode::A);
        // Seen once, and not lost in any of the readings after: the key repeats at the prompt
        // that follows, which is no matter here.
        assert!(output.contains("\r\n1,0\r\n"), "{output:?}");
    }

    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
    fn real_firmware_repeats_a_held_key_at_the_prompt_over_usb() {
        let mut board = Board::boot();
        board.press(Scancode::A);
        let start = board.cpu.cycle_count();
        let mut output = String::new();
        // Two seconds of machine time.
        while board.cpu.cycle_count() - start < 2 * cmm2_core::CORE_HZ {
            output += &board.tick();
        }
        board.release(Scancode::A);
        output += &board.settle();
        let typed = output.matches('a').count();
        println!("a key held for 2 s typed {typed} characters");
        // The firmware types the key at once, then repeats it after MMBasic's default delay of
        // 500 ms every 75 ms: 1 + (2000 - 500) / 75 = 21, give or take the edges of the 2 s.
        assert!((19..=22).contains(&typed), "{output:?}");
        assert!(output.chars().all(|c| c == 'a'), "{output:?}");
        // And once it is let go it stops.
        let after = board.tick();
        assert!(!after.contains('a'), "{after:?}");
    }

    // How fast the firmware takes typing that is all pressed at once, in the time of the machine
    // and of the host. The figures in docs/usb-host.md section 12 are from here.
    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
    fn real_firmware_takes_a_burst_of_typing_over_usb_in_order_at_a_measured_speed() {
        let mut board = Board::boot();
        let bursts = [
            ("lowercase", "abcdefghijklmnopqrstuvwxy", false),
            ("shifted", "abcdefghijklmnopqrstuvwxy", true),
            ("the same key", "llllllllllllllllllll", false),
        ];
        for (name, text, shifted) in bursts {
            let (start, wall) = (board.cpu.cycle_count(), Instant::now());
            let polls = board.keys.report.polls();
            for character in text.chars() {
                let scancode = letter(character.to_ascii_uppercase());
                if shifted {
                    board.press(Scancode::LShift);
                    board.press(scancode);
                    board.release(Scancode::LShift);
                    board.release(scancode);
                } else {
                    board.press(scancode);
                    board.release(scancode);
                }
            }
            let output = board.settle();
            let machine = (board.cpu.cycle_count() - start) as f64 / cmm2_core::CORE_HZ as f64;
            let host = wall.elapsed().as_secs_f64();
            let polls = board.keys.report.polls() - polls;
            println!(
                "{name}: {} characters in {polls} polls, {machine:.2} s of machine time and \
                 {host:.2} s of host time",
                output.len()
            );
            let expected = if shifted {
                text.to_ascii_uppercase()
            } else {
                text.to_string()
            };
            assert_eq!(output, expected);
        }
    }

    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
    fn real_firmware_stops_a_program_with_ctrl_c_right_behind_forty_taps_over_usb() {
        let mut board = Board::boot();
        board.type_text("FOR I = 1 TO 1000000000 : NEXT");
        board.tap(Scancode::Return);
        let running = board.tick();
        assert!(!running.contains(PROMPT), "{running:?}");
        for _ in 0..40 {
            board.press(Scancode::L);
            board.release(Scancode::L);
        }
        board.press(Scancode::LCtrl);
        board.press(Scancode::C);
        board.release(Scancode::LCtrl);
        board.release(Scancode::C);
        // The Ctrl-C comes after the taps, and it still arrives.
        let mut output = String::new();
        let limit = board.cpu.instruction_count() + 1_000_000_000;
        while !output.contains(PROMPT) {
            assert!(board.cpu.instruction_count() < limit, "{output:?}");
            output += &board.tick();
        }
    }

    #[test]
    #[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
    fn real_firmware_shows_num_lock_on_the_leds_over_usb() {
        let mut board = Board::boot();
        for _ in 0..40 {
            board.tick();
        }
        let before = board.keys.report.led_updates();
        board.tap(Scancode::NumLockClear);
        println!(
            "Num Lock: LEDs {:#b}, {} update(s) for the tap",
            board.keys.report.leds(),
            board.keys.report.led_updates() - before
        );
        assert_eq!(board.keys.report.leds() & LED_NUM, LED_NUM);
        assert_eq!(board.keys.report.led_updates() - before, 1);
    }
}
