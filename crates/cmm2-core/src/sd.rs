//! An SD card in SPI mode, a byte at a time. It has no clock: the firmware only depends on what
//! the card answers and in which order, so a reply is there as soon as it is asked for. The
//! card is backed by an image of its blocks. It is addressed by block number (an SDHC card),
//! and it knows nothing of the write-protect switch, which is the socket's. The image is held
//! in memory whole.

use std::collections::VecDeque;
use std::fmt;

/// The size of a block, in bytes.
pub const BLOCK: usize = 512;

/// The smallest image a front end should make a card of. A version 2.0 CSD counts in units of
/// 512 KB and cannot say less, so a smaller card would claim blocks it does not have.
pub const MIN_IMAGE_SIZE: usize = 512 * 1024;

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
// whole 2.7 to 3.6 V range. Until ACMD41 has finished the busy bit (31) is clear, and the
// capacity bit, which is only valid after it, with it.
const OCR: [u8; 4] = [0xC0, 0xFF, 0x80, 0x00];
const OCR_BUSY: [u8; 4] = [0x00, 0xFF, 0x80, 0x00];

// The token ahead of a block the card sends, or a single block it is sent, and the token that
// starts each block of a multiple block write. 0xFD ends such a write.
const DATA_TOKEN: u8 = 0xFE;
const MULTI_WRITE_TOKEN: u8 = 0xFC;
const STOP_TRAN_TOKEN: u8 = 0xFD;
// What the card sends in place of a block when a multiple block read has run off its end: the
// data error token with the out of range bit.
const OUT_OF_RANGE_TOKEN: u8 = 0x08;
// The data response of a write the card took, and the busy byte after it.
const DATA_ACCEPTED: u8 = 0x05;
// The data response of a write the card refused because the block is not on it.
const DATA_WRITE_ERROR: u8 = 0x0D;
const BUSY: u8 = 0x00;
const IDLE: u8 = 0xFF;

/// A command frame is the command, four argument bytes and a CRC; the CRC is not checked.
const FRAME: usize = 6;
/// A data block on the wire is followed by two CRC bytes, which are not checked either.
const DATA_CRC: usize = 2;

/// The size of a CSD or a CID, which the host reads as a data block.
const REGISTER: usize = 16;

/// The CID is a made-up identity, not that of any real card: manufacturer 0x00, application "CM",
/// product "CMM2E" revision 1.0, serial number 1, made in January 2026. The last byte is the
/// CRC7, filled in by `register`.
const CID: [u8; REGISTER] = [
    0x00, b'C', b'M', b'C', b'M', b'M', b'2', b'E', 0x10, 0x00, 0x00, 0x00, 0x01, 0x01, 0xA1, 0x01,
];

/// The CRC7 of the SD command and register formats, over `bytes`.
fn crc7(bytes: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &byte in bytes {
        for bit in (0..8).rev() {
            let feedback = (crc >> 6 ^ byte >> bit) & 1;
            crc = (crc << 1) & 0x7F;
            if feedback != 0 {
                crc ^= 0x09;
            }
        }
    }
    crc
}

/// A CSD or CID with its last byte made right: the CRC7 of the 15 before it, and the end bit.
fn register(mut bytes: [u8; REGISTER]) -> [u8; REGISTER] {
    bytes[REGISTER - 1] = crc7(&bytes[..REGISTER - 1]) << 1 | 1;
    bytes
}

#[derive(Debug)]
enum Phase {
    /// Waiting for the first byte of a command.
    Idle,
    /// The bytes of a command frame so far.
    Command(Vec<u8>),
    /// A write is accepted and waits for the data token. A multiple block write takes the
    /// token 0xFC for each block, from `block` on, and 0xFD to end.
    WriteToken { block: usize, multi: bool },
    /// The block being written, and the CRC after it, so far.
    WriteData {
        block: usize,
        multi: bool,
        data: Vec<u8>,
    },
}

/// A multiple block read (CMD18).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
    /// There is none.
    Off,
    /// It sends this block next.
    Next(usize),
    /// It has run off the end of the card and sent the error token. It only waits for CMD12.
    Ended,
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
    /// A multiple block read goes on until CMD12, or any other command, ends it.
    stream: Stream,
    /// A write has changed the image since the card was made.
    dirty: bool,
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
            stream: Stream::Off,
            dirty: false,
        })
    }

    /// Whether a write has changed the image since the card was made, so that it is worth
    /// writing back. A write of what a block already holds does not.
    pub fn dirty(&self) -> bool {
        self.dirty
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
            Phase::WriteToken { block, multi } => self.write_token(block, multi, byte),
            Phase::WriteData {
                block,
                multi,
                mut data,
            } => {
                data.push(byte);
                if data.len() != BLOCK + DATA_CRC {
                    self.phase = Phase::WriteData { block, multi, data };
                } else if block >= self.blocks() {
                    // A multiple block write that ran off the end of the card. The whole block
                    // is taken first, so that none of it is read as commands, and then refused,
                    // and the write is over.
                    self.reply.push_back(DATA_WRITE_ERROR);
                } else {
                    self.store(block, &data[..BLOCK]);
                    self.reply.extend([DATA_ACCEPTED, BUSY]);
                    if multi {
                        self.phase = Phase::WriteToken {
                            block: block + 1,
                            multi,
                        };
                    }
                }
            }
        }
        self.continue_read();
    }

    // The byte after a write was accepted: a token that starts a block, the one that ends a
    // multiple block write, or nothing yet.
    fn write_token(&mut self, block: usize, multi: bool, byte: u8) {
        let start = if multi { MULTI_WRITE_TOKEN } else { DATA_TOKEN };
        self.phase = Phase::WriteToken { block, multi };
        if byte == start {
            self.phase = Phase::WriteData {
                block,
                multi,
                data: Vec::with_capacity(BLOCK + DATA_CRC),
            };
        } else if multi && byte == STOP_TRAN_TOKEN {
            // The card is busy for a byte, as after a block.
            self.reply.push_back(BUSY);
            self.phase = Phase::Idle;
        }
    }

    // Writes `data` to `block`, and notes it if that changes the image.
    fn store(&mut self, block: usize, data: &[u8]) {
        let target = &mut self.image[block * BLOCK..][..BLOCK];
        if target != data {
            target.copy_from_slice(data);
            self.dirty = true;
        }
    }

    // A multiple block read sends its next block as soon as the last one is out, and carries
    // on until it is stopped; while a command is arriving it waits, so that CMD12 is heard.
    fn continue_read(&mut self) {
        let Stream::Next(block) = self.stream else {
            return;
        };
        if !self.reply.is_empty() || !matches!(self.phase, Phase::Idle) {
            return;
        }
        if block < self.blocks() {
            self.queue_block(block);
            self.stream = Stream::Next(block + 1);
        } else {
            self.reply.push_back(OUT_OF_RANGE_TOKEN);
            self.stream = Stream::Ended;
        }
    }

    /// Chip select went high: a command in progress is dropped, and a multiple block read with
    /// it.
    pub fn deselect(&mut self) {
        self.phase = Phase::Idle;
        self.reply.clear();
        self.stream = Stream::Off;
    }

    fn command(&mut self, index: u8, arg: u32) {
        let app = std::mem::take(&mut self.app_command);
        let idle = if self.ready { 0 } else { R1_IDLE };
        // Any command but CMD12 ends a multiple block read that is going.
        let stream = std::mem::replace(&mut self.stream, Stream::Off);
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
            // The OCR is there before ACMD41 too, but it is not ready, and says so.
            (false, 58, ready) => self.respond(idle, if ready { &OCR } else { &OCR_BUSY }),
            // Only CMD0, CMD1, CMD8, ACMD41, CMD58 and CMD59 are valid in the idle state, as I
            // remember the specification, so the registers wait for ACMD41 too.
            (false, 9, true) => self.respond_register(0, self.csd()),
            (false, 10, true) => self.respond_register(0, register(CID)),
            (false, 17 | 18, true) => match self.block(arg) {
                Some(block) => {
                    self.respond(0, &[]);
                    self.queue_block(block);
                    if index == 18 {
                        self.stream = Stream::Next(block + 1);
                    }
                }
                None => self.respond(R1_PARAMETER, &[]),
            },
            // CMD12 ends a multiple block read. Its reply is R1 and then busy, after the
            // stuff byte that the host throws away.
            (false, 12, true) if stream != Stream::Off => {
                self.respond(0, &[BUSY]);
            }
            (false, 24 | 25, true) => match self.block(arg) {
                Some(block) => {
                    self.respond(0, &[]);
                    self.phase = Phase::WriteToken {
                        block,
                        multi: index == 25,
                    };
                }
                None => self.respond(R1_PARAMETER, &[]),
            },
            // The number of blocks to erase before a multiple block write: nothing to erase
            // here, so it is accepted and ignored.
            (true, 23, true) => self.respond(0, &[]),
            _ => self.respond(idle | R1_ILLEGAL, &[]),
        }
    }

    fn blocks(&self) -> usize {
        self.image.len() / BLOCK
    }

    // The block `arg` names, if the card has it.
    fn block(&self, arg: u32) -> Option<usize> {
        let block = usize::try_from(arg).ok()?;
        (block < self.blocks()).then_some(block)
    }

    // The token, the block's data and the two CRC bytes, after whatever is queued.
    fn queue_block(&mut self, block: usize) {
        self.reply.push_back(DATA_TOKEN);
        self.reply.extend(&self.image[block * BLOCK..][..BLOCK]);
        self.reply.extend([IDLE; DATA_CRC]);
    }

    // The card's CSD, version 2.0 as an SDHC card has it. C_SIZE counts 512 KB, so the
    // capacity is the image in whole units of that. A version 2.0 CSD cannot say less than
    // 512 KB, so an image under that has a card that claims more blocks than it has (the ones
    // it does not have answer with a parameter error); the front end does not take one. The
    // rest is a typical card's: 25 MHz, 512-byte blocks, 64 KB erase blocks.
    fn csd(&self) -> [u8; REGISTER] {
        let units = (self.image.len() / MIN_IMAGE_SIZE).clamp(1, 1 << 22);
        let [_, c_size_high, c_size_mid, c_size_low] = (units as u32 - 1).to_be_bytes();
        register([
            0x40,
            0x0E,
            0x00,
            0x32,
            0x5B,
            0x59,
            0x00,
            c_size_high,
            c_size_mid,
            c_size_low,
            0x7F,
            0x80,
            0x0A,
            0x40,
            0x00,
            0x01,
        ])
    }

    // The R1 and then a register as a data block: the token, the 16 bytes and the CRC.
    fn respond_register(&mut self, r1: u8, bytes: [u8; REGISTER]) {
        let mut data = vec![DATA_TOKEN];
        data.extend(bytes);
        data.extend([IDLE; DATA_CRC]);
        self.respond(r1, &data);
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
mod tests;
