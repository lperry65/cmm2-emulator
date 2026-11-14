//! The pixel formats that are drawn, and how a pixel of each becomes 8 bits a channel.

use super::{CLUTEN, FrameError, Layer};

/// A pixel as the blender gets it: its colour and its alpha, each in 8 bits.
#[derive(Clone, Copy)]
pub(super) struct Argb {
    pub(super) alpha: u8,
    pub(super) rgb: [u8; 3],
}

/// The pixel formats that are drawn, by their number in PFCR.
#[derive(Clone, Copy)]
pub(super) enum Format {
    Argb8888,
    Rgb888,
    Rgb565,
    Argb1555,
    Argb4444,
    L8,
}

impl Format {
    pub(super) fn of(layer: &Layer) -> Result<Format, FrameError> {
        match layer.pfcr & 7 {
            0 => Ok(Format::Argb8888),
            1 => Ok(Format::Rgb888),
            2 => Ok(Format::Rgb565),
            3 => Ok(Format::Argb1555),
            4 => Ok(Format::Argb4444),
            5 if layer.cr & CLUTEN == 0 => {
                Err(FrameError::Unsupported("L8 without the colour table"))
            }
            5 => Ok(Format::L8),
            _ => Err(FrameError::Unsupported("pixel format AL44 or AL88")),
        }
    }

    #[inline(always)]
    pub(super) fn bytes_per_pixel(self) -> usize {
        match self {
            Format::L8 => 1,
            Format::Rgb565 | Format::Argb1555 | Format::Argb4444 => 2,
            Format::Rgb888 => 3,
            Format::Argb8888 => 4,
        }
    }

    /// The pixel as stored in memory, with each channel widened to 8 bits by repeating its high
    /// bits into the low ones, so full intensity is 255. A format with no alpha channel is
    /// opaque.
    #[inline(always)]
    pub(super) fn argb(self, pixel: &[u8], clut: &[u32; 256]) -> Argb {
        let opaque = |rgb| Argb { alpha: 255, rgb };
        let halfword = || u16::from_le_bytes([pixel[0], pixel[1]]);
        match self {
            Format::L8 => {
                let c = clut[usize::from(pixel[0])];
                opaque([(c >> 16) as u8, (c >> 8) as u8, c as u8])
            }
            Format::Rgb888 => opaque([pixel[2], pixel[1], pixel[0]]),
            Format::Rgb565 => {
                let v = halfword();
                opaque([
                    widen(v >> 11, 5),
                    widen(v >> 5 & 0x3F, 6),
                    widen(v & 0x1F, 5),
                ])
            }
            Format::Argb1555 => {
                let v = halfword();
                Argb {
                    alpha: if v >> 15 != 0 { 255 } else { 0 },
                    rgb: [
                        widen(v >> 10 & 0x1F, 5),
                        widen(v >> 5 & 0x1F, 5),
                        widen(v & 0x1F, 5),
                    ],
                }
            }
            Format::Argb4444 => {
                let v = halfword();
                Argb {
                    alpha: widen(v >> 12, 4),
                    rgb: [
                        widen(v >> 8 & 0xF, 4),
                        widen(v >> 4 & 0xF, 4),
                        widen(v & 0xF, 4),
                    ],
                }
            }
            Format::Argb8888 => Argb {
                alpha: pixel[3],
                rgb: [pixel[2], pixel[1], pixel[0]],
            },
        }
    }
}

/// A channel of `bits` bits (4 to 6) as 8 bits, its high bits repeated into the low ones.
fn widen(value: u16, bits: u32) -> u8 {
    (value << (8 - bits) | value >> (2 * bits - 8)) as u8
}

/// `x` / 255 to the nearest whole number. The divisor is a constant, so the compiler turns the
/// division into a multiplication; it is exact for every `x` that a blend can reach (at most
/// 510 x 255), which a test runs through.
pub(super) fn div255(x: u32) -> u32 {
    (x + 127) / 255
}
