//! The SD socket: a card, if one is in, and the four SPI-mode wires the firmware wiggles. The
//! GPIO ports tell the slot what the firmware drives and ask it what the card drives. The
//! slot is a cheap handle: clones share the socket, so the GPIO ports and the front end each
//! keep one.

use std::cell::RefCell;
use std::rc::Rc;

use crate::sd::{ImageSizeError, SdCard};

#[derive(Clone, Debug, Default)]
pub struct SdSlot(Rc<RefCell<Slot>>);

#[derive(Debug)]
struct Slot {
    card: Option<SdCard>,
    /// The write-protect switch is on. It belongs to the socket, and stays when a card is
    /// changed.
    protected: bool,
    /// Chip select is low. It starts out high, as the board pulls it up.
    selected: bool,
    clock: bool,
    /// The byte being shifted in, and how many of its bits have gone by (counted at the
    /// falling edge, where the card moves on to its next bit).
    shift: u8,
    bits: u8,
    /// The byte the card is shifting out; 0xFF when there is nothing to say.
    out: u8,
}

impl Default for Slot {
    fn default() -> Slot {
        Slot {
            card: None,
            protected: false,
            selected: false,
            clock: false,
            shift: 0,
            bits: 0,
            out: 0xFF,
        }
    }
}

impl SdSlot {
    pub fn new() -> SdSlot {
        SdSlot::default()
    }

    /// Puts a card holding `image` in the socket, in place of any card that was there.
    pub fn insert(&self, image: Vec<u8>) -> Result<(), ImageSizeError> {
        let mut slot = self.0.borrow_mut();
        slot.card = Some(SdCard::new(image)?);
        slot.restart_byte();
        Ok(())
    }

    /// Takes the card out and gives back its contents.
    pub fn eject(&self) -> Option<Vec<u8>> {
        let mut slot = self.0.borrow_mut();
        let card = slot.card.take();
        slot.restart_byte();
        card.map(SdCard::into_image)
    }

    /// Sets the write-protect switch. The firmware reads it from PH12 and refuses to write to a
    /// protected card; the card itself takes no notice of it, as a real one does not.
    pub fn set_write_protected(&self, protected: bool) {
        self.0.borrow_mut().protected = protected;
    }

    /// The write-protect switch is on.
    pub fn write_protected(&self) -> bool {
        self.0.borrow().protected
    }

    /// Whether the card in the socket has been written to in a way that changed it, so that its
    /// image is worth writing back. False with no card.
    pub fn modified(&self) -> bool {
        self.0.borrow().card.as_ref().is_some_and(SdCard::dirty)
    }

    /// The card-detect switch: there is a card in the socket.
    pub(crate) fn present(&self) -> bool {
        self.0.borrow().card.is_some()
    }

    /// The level of the chip select wire.
    pub(crate) fn set_chip_select(&self, high: bool) {
        let mut slot = self.0.borrow_mut();
        if slot.selected == !high {
            return;
        }
        slot.selected = !high;
        if high && let Some(card) = &mut slot.card {
            card.deselect();
        }
        slot.restart_byte();
    }

    /// The levels of the clock and data-in wires. SPI mode 0: the card samples data on a
    /// rising edge and moves to its next bit on a falling one.
    pub(crate) fn set_clock(&self, clock: bool, mosi: bool) {
        let mut slot = self.0.borrow_mut();
        let was = std::mem::replace(&mut slot.clock, clock);
        if !slot.selected || was == clock {
            return;
        }
        if clock {
            slot.shift = slot.shift << 1 | u8::from(mosi);
        } else {
            slot.bits += 1;
            if slot.bits == 8 {
                let byte = slot.shift;
                if let Some(card) = &mut slot.card {
                    card.mosi(byte);
                }
                slot.restart_byte();
            }
        }
    }

    /// Whether the data-out wire is high. The card only ever pulls it low: with no card, or
    /// chip select high, it shifts out 1s, which is the wire left to its pull-up.
    pub(crate) fn miso(&self) -> bool {
        let slot = self.0.borrow();
        slot.out >> (7 - slot.bits) & 1 != 0
    }
}

impl Slot {
    // The next byte starts: nothing shifted in yet, and the card's reply in the shifter.
    fn restart_byte(&mut self) {
        self.shift = 0;
        self.bits = 0;
        self.out = self.card.as_ref().map_or(0xFF, SdCard::miso);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot_with_card() -> SdSlot {
        let slot = SdSlot::new();
        slot.insert(vec![0x42; 4 * 512]).unwrap();
        slot
    }

    // One byte as the firmware's bit-bang sends it: data set up while the clock is low, the
    // clock raised, the data-out wire read while it is high, the clock lowered.
    fn transfer(slot: &SdSlot, byte: u8) -> u8 {
        let mut read = 0;
        for bit in (0..8).rev() {
            let mosi = byte >> bit & 1 != 0;
            slot.set_clock(false, mosi);
            slot.set_clock(true, mosi);
            read = read << 1 | u8::from(slot.miso());
            slot.set_clock(false, mosi);
        }
        read
    }

    fn command(slot: &SdSlot, index: u8, arg: u32) -> u8 {
        for byte in [0x40 | index]
            .into_iter()
            .chain(arg.to_be_bytes())
            .chain([0x01])
        {
            transfer(slot, byte);
        }
        (0..8)
            .map(|_| transfer(slot, 0xFF))
            .find(|byte| byte & 0x80 == 0)
            .expect("no reply")
    }

    #[test]
    fn an_empty_slot_has_no_card_and_leaves_the_wire_high() {
        let slot = SdSlot::new();
        assert!(!slot.present());
        slot.set_chip_select(false);
        assert!(slot.miso());
        assert_eq!(read_idle(&slot), [0xFF; 10]);
    }

    fn read_idle(slot: &SdSlot) -> Vec<u8> {
        (0..10).map(|_| transfer(slot, 0)).collect()
    }

    #[test]
    fn inserting_a_card_is_seen_by_the_card_detect_switch() {
        let slot = SdSlot::new();
        slot.insert(vec![0; 512]).unwrap();
        assert!(slot.present());
        slot.eject();
        assert!(!slot.present());
    }

    #[test]
    fn a_bad_image_is_refused_and_nothing_is_inserted() {
        let slot = SdSlot::new();
        assert_eq!(slot.insert(vec![0; 100]), Err(ImageSizeError(100)));
        assert!(!slot.present());
    }

    #[test]
    fn eject_gives_the_image_back() {
        let slot = SdSlot::new();
        assert_eq!(slot.eject(), None);
        slot.insert(vec![7; 1024]).unwrap();
        assert_eq!(slot.eject(), Some(vec![7; 1024]));
        assert_eq!(slot.eject(), None);
    }

    #[test]
    fn a_second_card_replaces_the_first() {
        let slot = SdSlot::new();
        slot.insert(vec![1; 512]).unwrap();
        slot.insert(vec![2; 1024]).unwrap();
        assert_eq!(slot.eject(), Some(vec![2; 1024]));
    }

    #[test]
    fn a_card_taken_out_mid_reply_leaves_the_wire_high() {
        let slot = slot_with_card();
        slot.set_chip_select(false);
        for byte in [0x40, 0, 0, 0, 0, 0x95, 0xFF] {
            transfer(&slot, byte);
        }
        slot.eject();
        assert_eq!(read_idle(&slot), [0xFF; 10]);
    }

    #[test]
    fn clones_share_the_socket() {
        let slot = SdSlot::new();
        let other = slot.clone();
        other.insert(vec![0; 512]).unwrap();
        assert!(slot.present());
    }

    #[test]
    fn cmd0_over_the_wires_answers_idle() {
        let slot = slot_with_card();
        slot.set_chip_select(false);
        assert_eq!(command(&slot, 0, 0), 0x01);
    }

    #[test]
    fn nothing_is_heard_while_chip_select_is_high() {
        let slot = slot_with_card();
        for _ in 0..10 {
            assert_eq!(transfer(&slot, 0xFF), 0xFF);
        }
        for byte in [0x40, 0, 0, 0, 0, 0x95] {
            transfer(&slot, byte);
        }
        slot.set_chip_select(false);
        assert_eq!(read_idle(&slot), [0xFF; 10]);
    }

    #[test]
    fn a_bit_is_out_before_its_rising_edge_and_the_next_one_comes_on_the_falling_edge() {
        let slot = slot_with_card();
        slot.set_chip_select(false);
        // The frame, and the idle byte before the reply. The reply is R1 = 0000_0001.
        for byte in [0x40, 0, 0, 0, 0, 0x95, 0xFF] {
            transfer(&slot, byte);
        }
        for bit in 0..8 {
            let high = bit == 7;
            assert_eq!(slot.miso(), high, "before the rising edge of bit {bit}");
            slot.set_clock(true, true);
            assert_eq!(slot.miso(), high, "while the clock is high for bit {bit}");
            slot.set_clock(false, true);
        }
        assert!(slot.miso(), "the idle byte after it");
    }

    #[test]
    fn raising_chip_select_mid_byte_starts_the_next_byte_afresh() {
        let slot = slot_with_card();
        slot.set_chip_select(false);
        for _ in 0..3 {
            slot.set_clock(true, true);
            slot.set_clock(false, true);
        }
        slot.set_chip_select(true);
        slot.set_chip_select(false);
        assert_eq!(command(&slot, 0, 0), 0x01);
    }

    #[test]
    fn the_write_protect_switch_is_the_sockets_and_stays_through_a_change_of_card() {
        let slot = SdSlot::new();
        assert!(!slot.write_protected());
        slot.set_write_protected(true);
        slot.insert(vec![0; 512]).unwrap();
        assert!(slot.write_protected());
        slot.eject();
        assert!(slot.write_protected());
        slot.set_write_protected(false);
        assert!(!slot.write_protected());
    }

    #[test]
    fn the_slot_says_whether_the_card_was_written_to() {
        let slot = slot_with_card();
        assert!(!slot.modified());
        slot.set_chip_select(false);
        command(&slot, 0, 0);
        command(&slot, 55, 0);
        command(&slot, 41, 0x4000_0000);
        assert_eq!(command(&slot, 17, 1), 0);
        assert!(!slot.modified(), "a read");
        assert_eq!(command(&slot, 24, 1), 0);
        transfer(&slot, 0xFF);
        transfer(&slot, 0xFE);
        for _ in 0..512 + 2 {
            transfer(&slot, 0x99);
        }
        assert!(slot.modified());
        // A card put in its place is its own, and with none the answer is no.
        slot.insert(vec![0; 512]).unwrap();
        assert!(!slot.modified());
        slot.eject();
        assert!(!slot.modified());
    }

    #[test]
    fn a_write_reaches_the_image() {
        let slot = slot_with_card();
        slot.set_chip_select(false);
        command(&slot, 0, 0);
        command(&slot, 55, 0);
        command(&slot, 41, 0x4000_0000);
        assert_eq!(command(&slot, 24, 1), 0);
        transfer(&slot, 0xFF);
        transfer(&slot, 0xFE);
        for _ in 0..512 + 2 {
            transfer(&slot, 0x99);
        }
        assert_eq!(transfer(&slot, 0xFF) & 0x1F, 0x05);
        let image = slot.eject().unwrap();
        assert_eq!(image[512..1024], [0x99; 512]);
        assert_eq!(image[..512], [0x42; 512]);
    }
}
