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
    /// The report the firmware was last sent.
    reported: [u8; 8],
    /// The locks have been compared with the host's since the firmware started polling.
    locks_followed: bool,
    /// What `KeyReport::led_updates` and `polls` were when a lock key first went down since the
    /// firmware last answered, and how many lock keys have gone down since: the firmware shows
    /// each with an update of the LEDs.
    lock_keys: Option<LockKeys>,
    /// Whether the host reports AltGr as a Left Control down and a Right Alt down, as Windows does.
    altgr_is_ctrl_alt: bool,
}

struct LockKeys {
    updates: u32,
    polls: u32,
    presses: u32,
}

impl UsbKeys {
    pub fn new(report: KeyReport) -> UsbKeys {
        UsbKeys {
            report,
            keys: Vec::new(),
            reported: [0; 8],
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
            self.keys.push(Key {
                usage,
                down: true,
                modifiers: self.modifiers_down(),
                shown: false,
                sent: false,
            });
            if usage == CAPS_LOCK || usage == NUM_LOCK {
                let report = &self.report;
                let keys = self.lock_keys.get_or_insert(LockKeys {
                    updates: report.led_updates(),
                    polls: report.polls(),
                    presses: 0,
                });
                keys.presses += 1;
            }
        }
        self.publish();
    }

    /// A key came up. One that was never pressed (a console key) is ignored.
    pub fn release(&mut self, usage: u8) {
        self.settle();
        if let Some(key) = self
            .keys
            .iter_mut()
            .rev()
            .find(|key| key.usage == usage && key.down)
        {
            key.down = false;
        }
        self.publish();
    }

    /// Every key up, as when the window loses the focus and will not see the releases.
    pub fn release_all(&mut self) {
        self.settle();
        for key in &mut self.keys {
            key.down = false;
        }
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
    /// lock key, as a tap would be let go of by the release. `num` is `None` where the host does
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
    fn leds_are_the_firmwares(&mut self) -> bool {
        self.settle();
        let report = &self.report;
        let first = report.led_updates() > 0 || report.polls() >= LED_WAIT_POLLS;
        let answered = self.lock_keys.as_ref().is_none_or(|keys| {
            report.led_updates() >= keys.updates + keys.presses
                || report.polls().saturating_sub(keys.polls) >= LED_WAIT_POLLS
        });
        let held = self.is_down(CAPS_LOCK) || self.is_down(NUM_LOCK);
        let current = first && answered && !held && !self.pending() && report.was_sent();
        if current && self.lock_keys.is_some() {
            self.lock_keys = None;
        }
        current
    }

    /// Has `follow_locks` look at the locks again, as when the window gets the focus back.
    pub fn check_locks(&mut self) {
        self.locks_followed = false;
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
        if self.report.was_sent() {
            self.reported = self.report.get();
            for key in &mut self.keys {
                key.sent |= key.shown;
            }
        }
        self.keys.retain(|key| key.down || !key.sent);
    }

    // The report for the keys as they are now.
    fn publish(&mut self) {
        self.keys.retain(|key| key.down || !key.sent);
        if !self.active() {
            // Nothing is polling, so there is nothing to keep a key for: only the modifiers are
            // kept, down, for when it does.
            self.keys.retain(|key| is_modifier(key.usage) && key.down);
            for key in &mut self.keys {
                key.shown = false;
                key.sent = false;
            }
            self.reported = [0; 8];
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
    // there is room. The first that does not go in holds back the ones after it, so that they
    // are sent in the order they were pressed. The modifiers are those of the first key that has
    // not been sent, which are the ones it went down with and not the ones that have been
    // pressed since, or, with none, the ones that are down.
    fn build(&mut self) -> [u8; 8] {
        let mut in_report: Vec<usize> = Vec::new();
        let mut modifiers = None;
        let mut held_back = false;
        for (at, key) in self.keys.iter().enumerate() {
            if is_modifier(key.usage) {
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
                || in_report.len() >= SLOTS
                || self.reported[2..].contains(&key.usage)
                || taken(key.usage)
                || modifiers.is_some_and(|bits| bits != key.modifiers);
            if clashes {
                held_back = true;
            } else {
                modifiers = Some(key.modifiers);
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
        let mut count = 0;
        for &at in &in_report {
            if count < SLOTS {
                report[2 + count] = self.keys[at].usage;
            }
            count += 1;
        }
        // More keys down than there are slots is the rollover error, which reports no key: the
        // firmware is told of all of them, and a real keyboard does no better.
        let rollover = self
            .keys
            .iter()
            .filter(|key| key.down && !is_modifier(key.usage))
            .count()
            > SLOTS;
        if rollover || count > SLOTS {
            report[2..].fill(ROLLOVER);
        }
        for (at, key) in self.keys.iter_mut().enumerate() {
            key.shown = if is_modifier(key.usage) {
                modifiers >> (key.usage - FIRST_MODIFIER) & 1 != 0
            } else {
                rollover || in_report.contains(&at)
            };
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

    /// What a firmware types from the reports it is sent, after one with no keys: each key is
    /// typed when it is in a report and was not in the one before, with that report's modifiers.
    fn typed_by_the_firmware(reports: impl IntoIterator<Item = [u8; 8]>) -> Vec<(u8, u8)> {
        let mut typed = Vec::new();
        let mut last = [0; 8];
        for report in reports {
            for &key in &report[2..] {
                if key > ROLLOVER && !last[2..].contains(&key) {
                    typed.push((key, report[0]));
                }
            }
            last = report;
        }
        typed
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
        keyboard.reset();
        keys.sync();
        assert_eq!(report.get(), [2, 0, 0, 0, 0, 0, 0, 0]);
        assert!(!keys.pending() || keys.keys.iter().all(|key| is_modifier(key.usage)));
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

    // What the firmware types from a poll, given the report it had before.
    fn hear(last: &mut [u8; 8], reply: In) -> Vec<(u8, u8)> {
        let In::Data(report) = reply else {
            return Vec::new();
        };
        let report: [u8; 8] = report.try_into().unwrap();
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
        assert!(typed >= 5, "{output:?}");
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
