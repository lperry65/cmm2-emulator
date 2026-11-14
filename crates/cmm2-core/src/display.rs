//! What the LTDC puts on the screen, as pixels the host can look at.
//!
//! The controller model copies the registers that matter here as the firmware writes them, and
//! the machine's memory map builds a `Frame` from that and the emulated RAM. The layer
//! registers are only copied when a reload takes them (SRCR), as on the chip, where they are
//! shadowed. The colour tables are written straight through.
//!
//! The picture starts as the background colour, and each layer in turn, layer 1 then layer 2,
//! is blended over what is there, as RM0433 32.4.2 (Layer blending, p. 1211) and 32.7.21 (LxBFCR,
//! p. 1227) give it:
//!
//!   BC = BF1 x C + BF2 x Cs
//!
//! where C is the layer's pixel, Cs is the colour below it (the background, for layer 1), and
//! the two blending factors are chosen in the layer's BFCR: BF1 is 4 (constant alpha) or 6
//! (pixel alpha x constant alpha), and BF2 is 5 (1 - constant alpha) or 7 (1 - pixel alpha x
//! constant alpha); the other codes are reserved. The constant alpha (CACR) is divided by 255
//! by the hardware (32.7.19, p. 1226). A pixel format without an alpha channel has alpha 255.
//! Outside the window of a layer, its default colour (DCCR, which has an alpha of its own) is
//! used for the pixel; so it is when the layer is disabled, and the blending is done then too
//! (32.4.2, p. 1212; 32.7.20, p. 1226), so a disabled layer blends its default colour over the
//! whole active area, with the factors in its BFCR. The manual says to keep a disabled layer's
//! factors at their reset value (0x0607, with a default colour of 0, which is transparent) so
//! that nothing shows.
//!
//! What RM0433 does not say, and this model chooses:
//!
//! - The rounding. The manual says only that the constant alpha is divided by 255; this blends
//!   to the nearest level (the sum is divided by 255 and rounded), so a mid-range level may be
//!   one out on the chip; 0 and full intensity are exact. The manual's own example (32.7.21:
//!   constant alpha 240, C 128, Cs 48, BC 123) comes out the same either way.
//! - A disabled layer with reserved factor codes. The firmware leaves layer 2 like that when it
//!   leaves the 12-bit depth (LEN 0, BFCR 0). The manual gives those codes no meaning, so such a
//!   layer adds nothing.
//! - What shows for the lines of a window past CFBLNR. The manual only says that the prefetch
//!   stops at the end of the frame buffer (32.4.2, p. 1211); the model shows the default colour.
//!
//! The line a layer fetches is CFBLL - 7 bytes: CFBLL is the line length in bytes plus 7 on the
//! H7 (32.7.23, p. 1229; the F4 and F7 had 3), and the firmware writes exactly the window's
//! bytes plus 7 for every mode. A line length longer than the window needs is harmless (the
//! rest is discarded and not displayed, 32.4.2); a shorter one makes the chip raise a FIFO
//! underrun and shows something the manual does not say, so a window wider than the line is
//! not drawn. A window taller than CFBLNR shows its default colour for the lines that
//! are not fetched. A window is fetched from its own first pixel: the part of it before the
//! active area is not shown, so the first pixel shown is further on in the framebuffer.
//!
//! The window registers count from the start of the sync, like the porch registers: the first
//! pixel of the active area is AHBP + 1 and the last is AAW (32.7.15, p. 1223), the first
//! line AVBP + 1 and the last AAH (32.7.16, p. 1224), and a window's WHSPPOS and WVSPPOS are
//! its last pixel and line, not one past. The manual wants a window inside the active area; one
//! that is not is drawn clipped.
//!
//! The pixel formats widen a channel of fewer than 8 bits to 8 by bit replication, the bits
//! repeated until there are more than 8 and the top 8 taken (32.4.2, p. 1209), which is what
//! `widen` does: 5 bits abcde become abcdeabc. ARGB1555's one alpha bit is 0 or 255.
//!
//! Only what the firmware uses is drawn: ARGB8888, RGB888, RGB565, ARGB1555 and ARGB4444, and
//! L8 through the colour table. A frame that needs anything else (colour keying, AL44 and AL88,
//! L8 without the colour table, the factor codes that the manual reserves) is reported as
//! unsupported instead of being drawn wrong.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

mod blend;
mod format;
use blend::composite;

/// The registers the frame depends on, as the controller holds them. The layer ones are those
/// of the last reload.
#[derive(Debug)]
pub(crate) struct Scanout {
    pub(crate) enabled: bool,
    pub(crate) bpcr: u32,
    pub(crate) awcr: u32,
    pub(crate) bccr: u32,
    pub(crate) layers: [Layer; 2],
    /// The colour table of each layer.
    pub(crate) clut: [[u32; 256]; 2],
}

impl Default for Scanout {
    fn default() -> Scanout {
        Scanout {
            enabled: false,
            bpcr: 0,
            awcr: 0,
            bccr: 0,
            layers: [Layer::default(); 2],
            clut: [[0; 256]; 2],
        }
    }
}

/// The shadowed registers of one layer.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Layer {
    pub(crate) cr: u32,
    pub(crate) whpcr: u32,
    pub(crate) wvpcr: u32,
    pub(crate) pfcr: u32,
    pub(crate) cacr: u32,
    pub(crate) dccr: u32,
    pub(crate) bfcr: u32,
    pub(crate) cfbar: u32,
    pub(crate) cfblr: u32,
    pub(crate) cfblnr: u32,
}

/// CACR as a reset leaves it (RM0433 32.7.19, p. 1226): a constant alpha of 1.
pub(crate) const CACR_RESET: u32 = 0xFF;

/// BFCR as a reset leaves it: pixel alpha times constant alpha.
const BFCR_RESET: u32 = 0x0607;

impl Default for Layer {
    fn default() -> Layer {
        Layer {
            cr: 0,
            whpcr: 0,
            wvpcr: 0,
            pfcr: 0,
            cacr: CACR_RESET,
            dccr: 0,
            bfcr: BFCR_RESET,
            cfbar: 0,
            cfblr: 0,
            cfblnr: 0,
        }
    }
}

const LEN: u32 = 1 << 0;
const COLKEN: u32 = 1 << 1;
const CLUTEN: u32 = 1 << 4;

// BFCR: BF1 is in bits 10:8 and BF2 in bits 2:0.
const BF1_CONSTANT: u32 = 4;
const BF1_PIXEL_TIMES_CONSTANT: u32 = 6;
const BF2_CONSTANT: u32 = 5;
const BF2_PIXEL_TIMES_CONSTANT: u32 = 7;

/// Why there is no frame to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// The controller is not enabled, so nothing is being scanned out.
    Off,
    /// The active area has no width or no height, so there is no picture.
    Empty,
    /// The configuration needs something that is not drawn.
    Unsupported(&'static str),
    /// The framebuffer is not wholly in RAM; the address is the first byte that is not.
    Unreadable(u32),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::Off => f.write_str("the LTDC is not enabled"),
            FrameError::Empty => f.write_str("the active area is empty"),
            FrameError::Unsupported(what) => write!(f, "unsupported: {what}"),
            FrameError::Unreadable(addr) => write!(f, "framebuffer at {addr:#010X} is not in RAM"),
        }
    }
}

impl std::error::Error for FrameError {}

/// The visible picture, 3 bytes (R, G, B) a pixel, left to right and top to bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}

impl Frame {
    /// The colour of the pixel at (`x`, `y`).
    pub fn pixel(&self, x: usize, y: usize) -> [u8; 3] {
        let at = 3 * (y * self.width + x);
        [self.rgb[at], self.rgb[at + 1], self.rgb[at + 2]]
    }

    /// The frame as a binary PPM (P6) file.
    pub fn to_ppm(&self) -> Vec<u8> {
        let mut out = format!("P6\n{} {}\n255\n", self.width, self.height).into_bytes();
        out.extend_from_slice(&self.rgb);
        out
    }
}

/// The controller's side of the display, shared with the memory map that builds the frame. A
/// cheap handle: clones share the registers.
#[derive(Clone, Debug, Default)]
pub(crate) struct Display(Rc<RefCell<Scanout>>);

/// The active area: its size, and the porches that its coordinates are counted from.
struct Active {
    width: usize,
    height: usize,
    h_porch: u32,
    v_porch: u32,
}

impl Active {
    /// The registers hold the last position, counted from the start of the sync, so the active
    /// area is the part between the back porch and the end of the active width.
    fn of(s: &Scanout) -> Active {
        let (h_porch, v_porch) = (s.bpcr >> 16 & 0xFFF, s.bpcr & 0x7FF);
        Active {
            width: (s.awcr >> 16 & 0xFFF).saturating_sub(h_porch) as usize,
            height: (s.awcr & 0x7FF).saturating_sub(v_porch) as usize,
            h_porch,
            v_porch,
        }
    }

    /// The layer's window in the coordinates of the active area, whose first pixel is (0, 0).
    /// The window is counted from the same coordinates as the porches: the first active pixel is
    /// one past the back porch. It can start before the active area or run past it.
    fn window(&self, layer: &Layer) -> Window {
        let from = |register: u32, mask: u32, porch: u32| {
            i64::from(register & mask) - i64::from(porch) - 1
        };
        Window {
            left: from(layer.whpcr, 0xFFF, self.h_porch),
            right: from(layer.whpcr >> 16, 0xFFF, self.h_porch) + 1,
            top: from(layer.wvpcr, 0x7FF, self.v_porch),
            bottom: from(layer.wvpcr >> 16, 0x7FF, self.v_porch) + 1,
        }
    }
}

/// A layer's window: the first column and row and one past the last, in the active area's
/// coordinates. The ends are not before the starts only if the layer is set up so.
struct Window {
    left: i64,
    right: i64,
    top: i64,
    bottom: i64,
}

impl Display {
    pub(crate) fn update(&self, change: impl FnOnce(&mut Scanout)) {
        change(&mut self.0.borrow_mut());
    }

    /// `ram(addr, len)` is the `len` bytes of RAM at `addr`, or `None` if they are not all
    /// there.
    pub(crate) fn frame<'a>(
        &self,
        ram: impl Fn(u32, usize) -> Option<&'a [u8]>,
    ) -> Result<Frame, FrameError> {
        let s = self.0.borrow();
        if !s.enabled {
            return Err(FrameError::Off);
        }
        let active = Active::of(&s);
        if active.width == 0 || active.height == 0 {
            return Err(FrameError::Empty);
        }
        let background = [(s.bccr >> 16) as u8, (s.bccr >> 8) as u8, s.bccr as u8];
        let mut rgb = background.repeat(active.width * active.height);
        for (layer, clut) in s.layers.iter().zip(&s.clut) {
            composite(layer, clut, &active, &mut rgb, &ram)?;
        }
        Ok(Frame {
            width: active.width,
            height: active.height,
            rgb,
        })
    }
}

#[cfg(test)]
mod tests;
