//! An SD card in SPI mode, a byte at a time. It has no clock: the firmware only depends on what
//! the card answers and in which order, so a reply is there as soon as it is asked for. The
//! card is backed by an image of its blocks. It is addressed by block number (an SDHC card),
//! and it knows nothing of the write-protect switch, which is the socket's.

use std::collections::VecDeque;
use std::fmt;

/// The size of a block, in bytes.
pub const BLOCK: usize = 512;

#[derive(Debug, PartialEq, Eq)]
pub struct ImageSizeError(pub usize);

impl fmt::Display for ImageSizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} bytes is not a whole number of {BLOCK}-byte blocks",
            self.0
        )
    }
}

impl std::error::Error for ImageSizeError {}

// The R1 bits I use.
const R1_IDLE: u8 = 0x01;
const R1_ILLEGAL: u8 = 0x04;
const R1_PARAMETER: u8 = 0x40;

// The operating conditions register: powered up, high capacity (so block addressed), and the
// whole 2.7 to 3.6 V range.
const OCR: [u8; 4] = [0xC0, 0xFF, 0x80, 0x00];

const DATA_TOKEN: u8 = 0xFE;
// The data response of a write the card took, and the busy byte after it.
const DATA_ACCEPTED: u8 = 0x05;
const BUSY: u8 = 0x00;
const IDLE: u8 = 0xFF;

/// A command frame is the command, four argument bytes and a CRC; the CRC is not checked.
const FRAME: usize = 6;
/// A data block on the wire is followed by two CRC bytes, which are not checked either.
const DATA_CRC: usize = 2;

#[derive(Debug)]
enum Phase {
    /// Waiting for the first byte of a command.
    Idle,
    /// The bytes of a command frame so far.
    Command(Vec<u8>),
    /// A write is accepted and waits for the data token.
    WriteToken { block: usize },
    /// The block being written, and the CRC after it, so far.
    WriteData { block: usize, data: Vec<u8> },
}

#[derive(Debug)]
pub struct SdCard {
    image: Vec<u8>,
    /// Out of the idle state, after ACMD41.
    ready: bool,
    /// The last command was CMD55, so the next is an application command.
    app_command: bool,
    phase: Phase,
    /// What the card will shift out next, ahead of the idle byte.
    reply: VecDeque<u8>,
}

impl SdCard {
    /// A card holding `image`, which must be one or more whole blocks.
    pub fn new(image: Vec<u8>) -> Result<SdCard, ImageSizeError> {
        if image.is_empty() || !image.len().is_multiple_of(BLOCK) {
            return Err(ImageSizeError(image.len()));
        }
        Ok(SdCard {
            image,
            ready: false,
            app_command: false,
            phase: Phase::Idle,
            reply: VecDeque::new(),
        })
    }

    /// The card's contents, as they are now.
    pub fn into_image(self) -> Vec<u8> {
        self.image
    }

    /// The byte the card shifts out while the next byte is shifted in.
    pub fn miso(&self) -> u8 {
        self.reply.front().copied().unwrap_or(IDLE)
    }

    /// Takes the next byte from the host.
    pub fn mosi(&mut self, byte: u8) {
        self.reply.pop_front();
        match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Idle if byte & 0xC0 == 0x40 => self.phase = Phase::Command(vec![byte]),
            Phase::Idle => {}
            Phase::Command(mut frame) => {
                frame.push(byte);
                if frame.len() == FRAME {
                    self.command(
                        frame[0] & 0x3F,
                        u32::from_be_bytes([frame[1], frame[2], frame[3], frame[4]]),
                    );
                } else {
                    self.phase = Phase::Command(frame);
                }
            }
            Phase::WriteToken { block } if byte == DATA_TOKEN => {
                self.phase = Phase::WriteData {
                    block,
                    data: Vec::with_capacity(BLOCK + DATA_CRC),
                };
            }
            Phase::WriteToken { block } => self.phase = Phase::WriteToken { block },
            Phase::WriteData { block, mut data } => {
                data.push(byte);
                if data.len() == BLOCK + DATA_CRC {
                    self.image[block * BLOCK..][..BLOCK].copy_from_slice(&data[..BLOCK]);
                    self.reply.extend([DATA_ACCEPTED, BUSY]);
                } else {
                    self.phase = Phase::WriteData { block, data };
                }
            }
        }
    }

    /// Chip select went high: a command in progress is dropped.
    pub fn deselect(&mut self) {
        self.phase = Phase::Idle;
        self.reply.clear();
    }

    fn command(&mut self, index: u8, arg: u32) {
        let app = std::mem::take(&mut self.app_command);
        let idle = if self.ready { 0 } else { R1_IDLE };
        match (app, index, self.ready) {
            (_, 0, _) => {
                self.ready = false;
                self.respond(R1_IDLE, &[]);
            }
            (false, 8, _) => {
                // R7: the voltage range and check pattern of the argument, sent back.
                let [_, _, voltage, pattern] = arg.to_be_bytes();
                self.respond(idle, &[0, 0, voltage, pattern]);
            }
            (false, 55, _) => {
                self.app_command = true;
                self.respond(idle, &[]);
            }
            (true, 41, _) => {
                self.ready = true;
                self.respond(0, &[]);
            }
            (false, 58, true) => self.respond(0, &OCR),
            (false, 17, true) => match self.block(arg) {
                Some(block) => {
                    let mut data = vec![DATA_TOKEN];
                    data.extend_from_slice(&self.image[block * BLOCK..][..BLOCK]);
                    data.extend([IDLE; DATA_CRC]);
                    self.respond(0, &data);
                }
                None => self.respond(R1_PARAMETER, &[]),
            },
            (false, 24, true) => match self.block(arg) {
                Some(block) => {
                    self.respond(0, &[]);
                    self.phase = Phase::WriteToken { block };
                }
                None => self.respond(R1_PARAMETER, &[]),
            },
            _ => self.respond(idle | R1_ILLEGAL, &[]),
        }
    }

    // The block `arg` names, if the card has it.
    fn block(&self, arg: u32) -> Option<usize> {
        let block = usize::try_from(arg).ok()?;
        (block < self.image.len() / BLOCK).then_some(block)
    }

    // A reply is the R1 and then `tail`, after one idle byte: a card never answers in the
    // byte that finishes the command.
    fn respond(&mut self, r1: u8, tail: &[u8]) {
        self.reply.clear();
        self.reply.push_back(IDLE);
        self.reply.push_back(r1);
        self.reply.extend(tail);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A card of `blocks` blocks, each filled with its own number plus one.
    fn card(blocks: usize) -> SdCard {
        let image = (0..blocks).flat_map(|b| [b as u8 + 1; BLOCK]).collect();
        SdCard::new(image).unwrap()
    }

    // One byte over the wire: the card's reply to it, as the host reads it.
    fn exchange(card: &mut SdCard, byte: u8) -> u8 {
        let reply = card.miso();
        card.mosi(byte);
        reply
    }

    // Sends a command frame (the CRC is never checked) and returns the bytes of the reply, up
    // to `len` after the first byte with a clear top bit, which is the R1.
    fn command(card: &mut SdCard, index: u8, arg: u32, len: usize) -> Vec<u8> {
        for byte in [0x40 | index]
            .into_iter()
            .chain(arg.to_be_bytes())
            .chain([0x01])
        {
            exchange(card, byte);
        }
        let mut reply = Vec::new();
        for _ in 0..10 {
            let byte = exchange(card, 0xFF);
            if reply.is_empty() && byte & 0x80 != 0 {
                continue;
            }
            reply.push(byte);
            if reply.len() == len {
                break;
            }
        }
        reply
    }

    // CMD0, CMD8, then CMD55 and ACMD41 until the card is ready.
    fn init(card: &mut SdCard) {
        assert_eq!(command(card, 0, 0, 1), [R1_IDLE]);
        assert_eq!(command(card, 8, 0x1AA, 5), [R1_IDLE, 0, 0, 0x01, 0xAA]);
        for _ in 0..10 {
            assert!(command(card, 55, 0, 1)[0] <= R1_IDLE);
            if command(card, 41, 0x4000_0000, 1) == [0] {
                return;
            }
        }
        panic!("the card never became ready");
    }

    fn read_block(card: &mut SdCard, block: u32) -> Vec<u8> {
        assert_eq!(command(card, 17, block, 1), [0]);
        // The token comes straight after the R1, and then the data.
        assert_eq!(exchange(card, 0xFF), 0xFE);
        let data = (0..BLOCK).map(|_| exchange(card, 0xFF)).collect();
        assert_eq!([exchange(card, 0xFF), exchange(card, 0xFF)], [0xFF, 0xFF]);
        data
    }

    // The data phase of CMD24: token, data, two CRC bytes. Returns the data response.
    fn write_block(card: &mut SdCard, block: u32, fill: u8) -> u8 {
        assert_eq!(command(card, 24, block, 1), [0]);
        exchange(card, 0xFF);
        exchange(card, 0xFE);
        for _ in 0..BLOCK {
            exchange(card, fill);
        }
        exchange(card, 0xFF);
        exchange(card, 0xFF);
        exchange(card, 0xFF)
    }

    #[test]
    fn an_image_of_whole_blocks_is_a_card() {
        assert!(SdCard::new(vec![0; BLOCK]).is_ok());
        assert!(SdCard::new(vec![0; 64 * BLOCK]).is_ok());
    }

    #[test]
    fn an_image_that_is_not_whole_blocks_is_refused() {
        assert_eq!(
            SdCard::new(vec![0; BLOCK + 1]).unwrap_err(),
            ImageSizeError(BLOCK + 1)
        );
        assert_eq!(SdCard::new(Vec::new()).unwrap_err(), ImageSizeError(0));
    }

    #[test]
    fn the_error_says_what_was_wrong() {
        assert_eq!(
            ImageSizeError(1000).to_string(),
            "1000 bytes is not a whole number of 512-byte blocks"
        );
    }

    #[test]
    fn a_card_that_is_asked_nothing_reads_ff() {
        let mut card = card(1);
        for _ in 0..20 {
            assert_eq!(exchange(&mut card, 0xFF), 0xFF);
        }
    }

    #[test]
    fn cmd0_answers_idle() {
        let mut card = card(1);
        assert_eq!(command(&mut card, 0, 0, 1), [R1_IDLE]);
    }

    #[test]
    fn the_reply_comes_one_byte_after_the_command() {
        let mut card = card(1);
        for byte in [0x40, 0, 0, 0, 0, 0x95] {
            assert_eq!(exchange(&mut card, byte), 0xFF);
        }
        assert_eq!(exchange(&mut card, 0xFF), 0xFF);
        assert_eq!(exchange(&mut card, 0xFF), R1_IDLE);
        assert_eq!(exchange(&mut card, 0xFF), 0xFF);
    }

    #[test]
    fn cmd8_echoes_the_voltage_and_the_check_pattern() {
        let mut card = card(1);
        assert_eq!(
            command(&mut card, 8, 0x0000_01AA, 5),
            [R1_IDLE, 0, 0, 0x01, 0xAA]
        );
        assert_eq!(
            command(&mut card, 8, 0x0000_0155, 5),
            [R1_IDLE, 0, 0, 0x01, 0x55]
        );
    }

    #[test]
    fn acmd41_makes_the_card_ready() {
        let mut card = card(1);
        command(&mut card, 0, 0, 1);
        command(&mut card, 55, 0, 1);
        assert_eq!(command(&mut card, 41, 0x4000_0000, 1), [0]);
        assert_eq!(command(&mut card, 55, 0, 1), [0]);
    }

    #[test]
    fn cmd41_without_cmd55_in_front_is_illegal() {
        let mut card = card(1);
        assert_eq!(
            command(&mut card, 41, 0x4000_0000, 1),
            [R1_IDLE | R1_ILLEGAL]
        );
    }

    #[test]
    fn the_app_command_prefix_covers_one_command_only() {
        let mut card = card(1);
        command(&mut card, 55, 0, 1);
        command(&mut card, 0, 0, 1);
        assert_eq!(
            command(&mut card, 41, 0x4000_0000, 1),
            [R1_IDLE | R1_ILLEGAL]
        );
    }

    #[test]
    fn cmd58_says_the_card_is_block_addressed() {
        let mut card = card(1);
        init(&mut card);
        let ocr = command(&mut card, 58, 0, 5);
        assert_eq!(ocr[0], 0);
        let ocr = u32::from_be_bytes(ocr[1..].try_into().unwrap());
        assert_eq!(ocr & 1 << 31, 1 << 31, "powered up");
        assert_eq!(ocr & 1 << 30, 1 << 30, "high capacity");
    }

    #[test]
    fn cmd0_puts_a_ready_card_back_to_idle() {
        let mut card = card(1);
        init(&mut card);
        assert_eq!(command(&mut card, 0, 0, 1), [R1_IDLE]);
        assert_eq!(command(&mut card, 17, 0, 1), [R1_IDLE | R1_ILLEGAL]);
    }

    #[test]
    fn data_commands_need_an_initialised_card() {
        let mut card = card(1);
        for index in [17, 24, 58] {
            assert_eq!(
                command(&mut card, index, 0, 1),
                [R1_IDLE | R1_ILLEGAL],
                "CMD{index}"
            );
        }
    }

    #[test]
    fn an_unknown_command_is_illegal() {
        let mut card = card(1);
        init(&mut card);
        assert_eq!(command(&mut card, 12, 0, 1), [R1_ILLEGAL]);
        assert_eq!(command(&mut card, 18, 0, 1), [R1_ILLEGAL]);
    }

    #[test]
    fn cmd17_reads_a_block() {
        let mut card = card(4);
        init(&mut card);
        assert_eq!(read_block(&mut card, 0), vec![1; BLOCK]);
        assert_eq!(read_block(&mut card, 3), vec![4; BLOCK]);
        assert_eq!(read_block(&mut card, 1), vec![2; BLOCK]);
    }

    #[test]
    fn cmd17_past_the_end_is_a_parameter_error_with_no_data() {
        let mut card = card(4);
        init(&mut card);
        assert_eq!(command(&mut card, 17, 4, 1), [R1_PARAMETER]);
        for _ in 0..BLOCK + 8 {
            assert_eq!(exchange(&mut card, 0xFF), 0xFF);
        }
        assert_eq!(command(&mut card, 17, u32::MAX, 1), [R1_PARAMETER]);
    }

    #[test]
    fn cmd24_writes_a_block_and_the_others_are_left_alone() {
        let mut card = card(4);
        init(&mut card);
        assert_eq!(write_block(&mut card, 2, 0xA5) & 0x1F, 0x05);
        assert_eq!(read_block(&mut card, 2), vec![0xA5; BLOCK]);
        let image = card.into_image();
        assert_eq!(image.len(), 4 * BLOCK);
        assert!(
            image[..2 * BLOCK]
                .chunks(BLOCK)
                .enumerate()
                .all(|(b, data)| data == [b as u8 + 1; BLOCK])
        );
        assert_eq!(image[2 * BLOCK..3 * BLOCK], [0xA5; BLOCK]);
        assert_eq!(image[3 * BLOCK..], [4; BLOCK]);
    }

    #[test]
    fn a_write_is_busy_for_a_byte_and_then_ready() {
        let mut card = card(1);
        init(&mut card);
        assert_eq!(command(&mut card, 24, 0, 1), [0]);
        exchange(&mut card, 0xFF);
        exchange(&mut card, 0xFE);
        for _ in 0..BLOCK + 2 {
            exchange(&mut card, 0x11);
        }
        assert_eq!(exchange(&mut card, 0xFF) & 0x1F, 0x05);
        assert_eq!(exchange(&mut card, 0xFF), 0x00);
        assert_eq!(exchange(&mut card, 0xFF), 0xFF);
    }

    #[test]
    fn a_write_waits_for_its_token() {
        let mut card = card(1);
        init(&mut card);
        command(&mut card, 24, 0, 1);
        for _ in 0..20 {
            exchange(&mut card, 0xFF);
        }
        exchange(&mut card, 0xFE);
        for _ in 0..BLOCK + 2 {
            exchange(&mut card, 0x22);
        }
        assert_eq!(read_block(&mut card, 0), vec![0x22; BLOCK]);
    }

    #[test]
    fn cmd24_past_the_end_is_a_parameter_error_and_takes_no_data() {
        let mut card = card(2);
        init(&mut card);
        assert_eq!(command(&mut card, 24, 2, 1), [R1_PARAMETER]);
        exchange(&mut card, 0xFE);
        for _ in 0..BLOCK + 2 {
            exchange(&mut card, 0x33);
        }
        assert_eq!(
            card.into_image(),
            [1; BLOCK]
                .into_iter()
                .chain([2; BLOCK])
                .collect::<Vec<u8>>()
        );
    }

    #[test]
    fn the_card_never_refuses_a_write_for_a_write_protect_switch() {
        // The switch is on the socket and the firmware reads it; the card has no say.
        let mut card = card(1);
        init(&mut card);
        assert_eq!(write_block(&mut card, 0, 0x5A) & 0x1F, 0x05);
    }

    #[test]
    fn deselecting_drops_a_command_half_way() {
        let mut card = card(1);
        init(&mut card);
        for byte in [0x51, 0, 0] {
            exchange(&mut card, byte);
        }
        card.deselect();
        // A fresh frame is taken from its first byte, not as the end of the old one.
        assert_eq!(command(&mut card, 17, 0, 1), [0]);
    }

    #[test]
    fn deselecting_drops_the_reply_in_progress() {
        let mut card = card(1);
        init(&mut card);
        command(&mut card, 17, 0, 1);
        card.deselect();
        assert_eq!(card.miso(), 0xFF);
        for _ in 0..BLOCK + 4 {
            assert_eq!(exchange(&mut card, 0xFF), 0xFF);
        }
    }

    #[test]
    fn deselecting_drops_a_write_before_it_lands() {
        let mut card = card(1);
        init(&mut card);
        command(&mut card, 24, 0, 1);
        exchange(&mut card, 0xFE);
        for _ in 0..100 {
            exchange(&mut card, 0x77);
        }
        card.deselect();
        for _ in 0..BLOCK {
            exchange(&mut card, 0x77);
        }
        assert_eq!(card.into_image(), vec![1; BLOCK]);
    }

    #[test]
    fn deselecting_keeps_the_card_initialised() {
        let mut card = card(1);
        init(&mut card);
        card.deselect();
        assert_eq!(read_block(&mut card, 0), vec![1; BLOCK]);
    }

    #[test]
    fn a_command_cuts_short_the_reply_before_it() {
        let mut card = card(2);
        init(&mut card);
        command(&mut card, 17, 0, 1);
        exchange(&mut card, 0xFF);
        assert_eq!(read_block(&mut card, 1), vec![2; BLOCK]);
    }
}
