//! What the LTDC puts on the screen, as pixels the host can look at.
//!
//! The controller model copies the registers that matter here as the firmware writes them, and
//! the machine's memory map builds a `Frame` from that and the emulated RAM. The layer
//! registers are only copied when a reload takes them (SRCR), as on the chip, where they are
//! shadowed. The colour tables are written straight through.
//!
//! The picture starts as the background colour, and each layer in turn, layer 1 then layer 2,
//! is blended over what is there. This is the layer blending as I remember it from RM0433, and
//! I have not checked any of it against the manual yet (it is not on this machine), so all of
//! it is to be checked against RM0433:
//!
//!   result = BF1 x C + BF2 x Cs
//!
//! where C is the layer's pixel, Cs is the colour below it (the background, for layer 1), and
//! the two blending factors are chosen in the layer's BFCR. BF1 is the constant alpha (CACR)
//! or the pixel's alpha times it, and BF2 is one minus whichever the layer uses for BF1,
//! as the firmware programs it (the other pairings are worked out as written). A pixel
//! format without an alpha channel has alpha 255. Outside the window of a layer that is on, its
//! default colour (DCCR, which has an alpha of its own) stands for its pixel; so it does for the
//! window's lines past the CFBLNR the layer fetches.
//!
//! Three things in that are my choices, not what I have read in the manual:
//!
//! - A layer that is off adds nothing. That is my reading, and I chose it because of what the
//!   firmware does: leaving the 12-bit depth it sets layer 2's LEN to 0 and its BFCR to 0, which
//!   is a reserved code, so it cannot expect a layer that is off to be blended. RM0433's wording
//!   for DCCR may be that the default colour is used outside the defined layer window or when
//!   the layer is disabled, which would mean a disabled layer still blends its default colour.
//! - The blend rounds to the nearest level (the sum is divided by 255 and rounded). The chip may
//!   round another way, so a mid-range level can be one out; 0 and full intensity are exact.
//! - The line a layer fetches is CFBLL - 7 bytes. That is the H7 HAL's rule (CFBLL is the line
//!   length plus 7, where the F4 and F7 had 3), and the firmware writes exactly the window's
//!   bytes plus 7 for every mode; I have not checked it against RM0433. A window wider than
//!   that is not drawn. A window taller than CFBLNR shows its default colour for the lines that
//!   are not fetched. A window is fetched from its own first pixel: the part of it before the
//!   active area is not shown, so the first pixel shown is further on in the framebuffer.
//!
//! Only what the firmware uses is drawn: ARGB8888, RGB888, RGB565, ARGB1555 and ARGB4444, and
//! L8 through the colour table. A frame that needs anything else (colour keying, AL44 and AL88,
//! L8 without the colour table, blending factors that BFCR can name but the firmware never
//! sets) is reported as unsupported instead of being drawn wrong.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

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

/// BFCR as a reset leaves it: pixel alpha times constant alpha.
const BFCR_RESET: u32 = 0x0607;

impl Default for Layer {
    fn default() -> Layer {
        Layer {
            cr: 0,
            whpcr: 0,
            wvpcr: 0,
            pfcr: 0,
            cacr: 0,
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

/// A pixel as the blender gets it: its colour and its alpha, each in 8 bits.
#[derive(Clone, Copy)]
struct Argb {
    alpha: u8,
    rgb: [u8; 3],
}

/// The pixel formats that are drawn, by their number in PFCR.
#[derive(Clone, Copy)]
enum Format {
    Argb8888,
    Rgb888,
    Rgb565,
    Argb1555,
    Argb4444,
    L8,
}

impl Format {
    fn of(layer: &Layer) -> Result<Format, FrameError> {
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
    fn bytes_per_pixel(self) -> usize {
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
    fn argb(self, pixel: &[u8], clut: &[u32; 256]) -> Argb {
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
fn div255(x: u32) -> u32 {
    (x + 127) / 255
}

/// How a layer is blended with what is below it: BC = BF1 x C + BF2 x Cs, where each factor is
/// the layer's constant alpha or the pixel's alpha times it (BF2 is one minus that).
struct Blending {
    constant_alpha: u32,
    bf1_uses_pixel_alpha: bool,
    bf2_uses_pixel_alpha: bool,
}

/// What a blend comes to for a pixel with a given alpha. The two ends need no arithmetic: a
/// pixel that replaces what is below it (BF1 = 255, BF2 = 0), and one that leaves it alone (the
/// other way round, as a transparent default colour does).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mix {
    Replace,
    Keep,
    Weighted { bf1: u32, bf2: u32 },
}

impl Blending {
    fn of(layer: &Layer) -> Result<Blending, FrameError> {
        let uses_pixel_alpha = |bits, constant, with_pixel| match bits {
            b if b == constant => Ok(false),
            b if b == with_pixel => Ok(true),
            _ => Err(FrameError::Unsupported("blending factors")),
        };
        Ok(Blending {
            constant_alpha: layer.cacr & 0xFF,
            bf1_uses_pixel_alpha: uses_pixel_alpha(
                layer.bfcr >> 8 & 7,
                BF1_CONSTANT,
                BF1_PIXEL_TIMES_CONSTANT,
            )?,
            bf2_uses_pixel_alpha: uses_pixel_alpha(
                layer.bfcr & 7,
                BF2_CONSTANT,
                BF2_PIXEL_TIMES_CONSTANT,
            )?,
        })
    }

    /// The blend of a pixel whose alpha is `alpha`.
    fn mix(&self, alpha: u8) -> Mix {
        let pixel_times_constant = div255(u32::from(alpha) * self.constant_alpha);
        let factor = |uses_pixel_alpha| {
            if uses_pixel_alpha {
                pixel_times_constant
            } else {
                self.constant_alpha
            }
        };
        let (bf1, bf2) = (
            factor(self.bf1_uses_pixel_alpha),
            255 - factor(self.bf2_uses_pixel_alpha),
        );
        match (bf1, bf2) {
            (255, 0) => Mix::Replace,
            (0, 255) => Mix::Keep,
            _ => Mix::Weighted { bf1, bf2 },
        }
    }
}

impl Mix {
    /// Blends `pixel` over `below`, which becomes the result.
    fn apply(self, pixel: [u8; 3], below: &mut [u8; 3]) {
        match self {
            Mix::Replace => *below = pixel,
            Mix::Keep => {}
            Mix::Weighted { bf1, bf2 } => {
                for (below, above) in below.iter_mut().zip(pixel) {
                    *below =
                        div255(bf1 * u32::from(above) + bf2 * u32::from(*below)).min(255) as u8;
                }
            }
        }
    }

    /// Blends one colour over every pixel of `pixels`.
    fn apply_to_all(self, colour: [u8; 3], pixels: &mut [[u8; 3]]) {
        match self {
            Mix::Keep => {}
            Mix::Replace => pixels.fill(colour),
            Mix::Weighted { .. } => {
                for below in pixels {
                    self.apply(colour, below);
                }
            }
        }
    }
}

/// Blends the pixels of one line of the framebuffer over the `window` of the picture.
#[inline(always)]
fn blend_line(
    format: Format,
    line: &[u8],
    clut: &[u32; 256],
    mixes: &[Mix; 256],
    window: &mut [[u8; 3]],
) {
    for (out, pixel) in window
        .iter_mut()
        .zip(line.chunks_exact(format.bytes_per_pixel()))
    {
        let pixel = format.argb(pixel, clut);
        mixes[usize::from(pixel.alpha)].apply(pixel.rgb, out);
    }
}

/// CFBLL is the number of bytes in a line, plus 7. That is the H7 HAL's rule, which the firmware
/// follows for every mode (the F4 and F7 had 3); to be checked against RM0433.
const CFBLL_EXTRA: usize = 7;

/// Blends one layer over `rgb`, the picture so far. A layer that is off adds nothing. Where the
/// window of one that is on does not reach, its default colour stands for its pixel, and so it
/// does for the window's lines past the CFBLNR that the layer fetches.
fn composite<'a>(
    layer: &Layer,
    clut: &[u32; 256],
    active: &Active,
    rgb: &mut [u8],
    ram: &impl Fn(u32, usize) -> Option<&'a [u8]>,
) -> Result<(), FrameError> {
    if layer.cr & LEN == 0 {
        return Ok(());
    }
    if layer.cr & COLKEN != 0 {
        return Err(FrameError::Unsupported("colour keying"));
    }
    let blending = Blending::of(layer)?;
    let format = Format::of(layer)?;
    let bytes_per_pixel = format.bytes_per_pixel();
    let default = Argb {
        alpha: (layer.dccr >> 24) as u8,
        rgb: [
            (layer.dccr >> 16) as u8,
            (layer.dccr >> 8) as u8,
            layer.dccr as u8,
        ],
    };
    let window = active.window(layer);
    // What of the window is in the active area, and the part of it before the active area that is
    // fetched but not shown.
    let (width, height) = (active.width as i64, active.height as i64);
    let x0 = window.left.clamp(0, width);
    let x1 = window.right.clamp(x0, width);
    let y0 = window.top.clamp(0, height);
    // Only CFBLNR lines of the window are fetched.
    let lines_end = window
        .bottom
        .min(window.top + i64::from(layer.cfblnr & 0x7FF));
    let y1 = lines_end.clamp(y0, height);
    let skip_bytes = ((x0 - window.left).max(0) as usize) * bytes_per_pixel;
    let skip_lines = (y0 - window.top).max(0) as u32;
    let (x0, x1, y0, y1) = (x0 as usize, x1 as usize, y0 as usize, y1 as usize);
    let line_bytes = (x1 - x0) * bytes_per_pixel;
    let window_bytes = (window.right - window.left).max(0) as usize * bytes_per_pixel;
    let fetched = (layer.cfblr as usize & 0x1FFF).saturating_sub(CFBLL_EXTRA);
    let fetches = x1 > x0 && y1 > y0;
    if fetches && window_bytes > fetched {
        return Err(FrameError::Unsupported(
            "window wider than the fetched line",
        ));
    }
    let pitch = layer.cfblr >> 16 & 0x1FFF;
    // The blend for each alpha a pixel can have, so no pixel has to work its factors out.
    let mixes: [Mix; 256] = std::array::from_fn(|alpha| blending.mix(alpha as u8));
    let default_mix = mixes[usize::from(default.alpha)];
    for (y, row) in rgb.chunks_exact_mut(3 * active.width).enumerate() {
        let row = row.as_chunks_mut::<3>().0;
        if !fetches || !(y0..y1).contains(&y) {
            default_mix.apply_to_all(default.rgb, row);
            continue;
        }
        let addr = layer
            .cfbar
            .wrapping_add(pitch.wrapping_mul((y - y0) as u32 + skip_lines))
            .wrapping_add(skip_bytes as u32);
        let line = ram(addr, line_bytes).ok_or(FrameError::Unreadable(addr))?;
        let (before, rest) = row.split_at_mut(x0);
        let (window, after) = rest.split_at_mut(x1 - x0);
        default_mix.apply_to_all(default.rgb, before);
        // Each format has its own copy of the loop, so that the format is not looked at for
        // every pixel.
        match format {
            Format::Argb8888 => blend_line(Format::Argb8888, line, clut, &mixes, window),
            Format::Rgb888 => blend_line(Format::Rgb888, line, clut, &mixes, window),
            Format::Rgb565 => blend_line(Format::Rgb565, line, clut, &mixes, window),
            Format::Argb1555 => blend_line(Format::Argb1555, line, clut, &mixes, window),
            Format::Argb4444 => blend_line(Format::Argb4444, line, clut, &mixes, window),
            Format::L8 => blend_line(Format::L8, line, clut, &mixes, window),
        }
        default_mix.apply_to_all(default.rgb, after);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The board's active area: 800 x 600, with the porches the firmware programs.
    const BPCR: u32 = 0x00EF_0017;
    const AWCR: u32 = 0x040F_026F;
    const WHPCR: u32 = 0x040F_00F0;
    const WVPCR: u32 = 0x026F_0018;
    const BASE: u32 = 0x2400_0000;

    // The pixel formats, as PFCR numbers them.
    const ARGB8888: u32 = 0;
    const RGB888: u32 = 1;
    const RGB565: u32 = 2;
    const ARGB1555: u32 = 3;
    const ARGB4444: u32 = 4;
    const L8: u32 = 5;

    // What the firmware programs in BFCR: constant alpha, or pixel alpha times it.
    const BF_CONSTANT: u32 = 0x405;
    const BF_PIXEL: u32 = 0x607;

    // The firmware's layer 1 on the board: opaque black as the default colour, constant alpha
    // 255 and blending with the constant alpha.
    fn layer(format: u32, cr: u32, pitch: u32) -> Layer {
        Layer {
            cr,
            whpcr: WHPCR,
            wvpcr: WVPCR,
            pfcr: format,
            cacr: 0xFF,
            dccr: 0xFF00_0000,
            bfcr: BF_CONSTANT,
            cfbar: BASE,
            cfblr: pitch << 16 | (pitch + 7),
            cfblnr: 600,
        }
    }

    fn board(format: u32, cr: u32) -> Display {
        let display = Display::default();
        display.update(|s| {
            s.enabled = true;
            s.bpcr = BPCR;
            s.awcr = AWCR;
            // Layer 2 is off, as the firmware leaves it until it is used.
            s.layers = [layer(format, cr, 800), Layer::default()];
        });
        display
    }

    // The board with a second layer that shows the same part of the screen from `BASE`, the
    // way the firmware sets up the 12-bit depth: blended by pixel alpha, a transparent
    // default colour.
    fn two_layers(format1: u32, format2: u32) -> Display {
        let display = board(format1, LEN);
        display.update(|s| {
            s.layers[1] = Layer {
                dccr: 0,
                bfcr: BF_PIXEL,
                ..layer(format2, LEN, 800)
            };
        });
        display
    }

    // Layer 1 shows through where it has nothing: a transparent default colour, blended by pixel
    // alpha.
    fn transparent_default(display: &Display) {
        display.update(|s| {
            s.layers[0].dccr = 0;
            s.layers[0].bfcr = BF_PIXEL;
        });
    }

    // A RAM that holds `bytes` from `BASE` on and nothing else.
    fn ram<'a>(bytes: &'a [u8]) -> impl Fn(u32, usize) -> Option<&'a [u8]> {
        move |addr, len| {
            let start = addr.checked_sub(BASE)? as usize;
            bytes.get(start..start + len)
        }
    }

    // A RAM of 16-bit pixels, the first of which is `first`, and then `rest`.
    fn pixels16(first: u16, rest: u16) -> Vec<u8> {
        let mut memory = rest.to_le_bytes().repeat(1600 * 600 / 2);
        memory[..2].copy_from_slice(&first.to_le_bytes());
        memory
    }

    fn pitch16(display: &Display) {
        display.update(|s| {
            for layer in &mut s.layers {
                layer.cfblr = 1600 << 16 | 1607;
            }
        });
    }

    #[test]
    fn nothing_is_shown_while_the_controller_is_off() {
        let display = Display::default();
        assert_eq!(display.frame(ram(&[])), Err(FrameError::Off));
    }

    #[test]
    fn the_frame_is_the_active_area() {
        let display = board(L8, LEN | CLUTEN);
        let frame = display.frame(ram(&vec![0; 800 * 600])).unwrap();
        assert_eq!((frame.width, frame.height), (800, 600));
        assert_eq!(frame.rgb.len(), 800 * 600 * 3);
    }

    #[test]
    fn l8_looks_each_pixel_up_in_the_colour_table() {
        let display = board(L8, LEN | CLUTEN);
        display.update(|s| {
            s.clut[0][1] = 0x0011_2233;
            s.clut[0][255] = 0x00AA_BBCC;
        });
        let mut memory = vec![0; 800 * 600];
        memory[0] = 1;
        memory[799] = 255;
        memory[800 * 599 + 5] = 1;
        let frame = display.frame(ram(&memory)).unwrap();
        assert_eq!(frame.pixel(0, 0), [0x11, 0x22, 0x33]);
        assert_eq!(frame.pixel(1, 0), [0, 0, 0]);
        assert_eq!(frame.pixel(799, 0), [0xAA, 0xBB, 0xCC]);
        assert_eq!(frame.pixel(5, 599), [0x11, 0x22, 0x33]);
    }

    #[test]
    fn each_layer_has_its_own_colour_table() {
        let display = two_layers(L8, L8);
        display.update(|s| {
            s.layers[1].cr |= CLUTEN;
            s.layers[0].cr |= CLUTEN;
            s.clut[0][1] = 0x0011_2233;
            s.clut[1][1] = 0x0044_5566;
            // Layer 2's window covers only the first 2 pixels of a row, layer 1 shows elsewhere.
            s.layers[1].whpcr = (0xF0 + 1) << 16 | 0xF0;
        });
        let frame = display.frame(ram(&vec![1; 800 * 600])).unwrap();
        assert_eq!(frame.pixel(0, 0), [0x44, 0x55, 0x66]);
        assert_eq!(frame.pixel(1, 0), [0x44, 0x55, 0x66]);
        assert_eq!(frame.pixel(2, 0), [0x11, 0x22, 0x33]);
    }

    #[test]
    fn the_pitch_steps_from_line_to_line() {
        let display = board(L8, LEN | CLUTEN);
        display.update(|s| {
            s.clut[0][7] = 0x0000_00FF;
            s.layers[0].cfblr = 1024 << 16 | 807;
        });
        let mut memory = vec![0; 1024 * 600];
        memory[1024 * 3 + 2] = 7;
        let frame = display.frame(ram(&memory)).unwrap();
        assert_eq!(frame.pixel(2, 3), [0, 0, 0xFF]);
        assert_eq!(frame.pixel(2, 2), [0, 0, 0]);
    }

    #[test]
    fn the_pitch_is_thirteen_bits() {
        let display = board(L8, LEN | CLUTEN);
        display.update(|s| {
            s.clut[0][1] = 0x00FF_FFFF;
            s.layers[0].whpcr = 0xF3 << 16 | 0xF0;
            s.layers[0].wvpcr = 0x19 << 16 | 0x18;
            s.layers[0].cfblr = 5000 << 16 | 11;
        });
        let mut memory = vec![0; 5004];
        memory[5000] = 1;
        let frame = display.frame(ram(&memory)).unwrap();
        assert_eq!(frame.pixel(0, 0), [0, 0, 0]);
        assert_eq!(frame.pixel(0, 1), [255, 255, 255]);
    }

    #[test]
    fn the_layer_window_is_placed_inside_the_active_area() {
        // A 4 x 2 window whose top left pixel is at (10, 20) of the active area.
        let display = board(L8, LEN | CLUTEN);
        display.update(|s| {
            s.clut[0][9] = 0x0000_FF00;
            s.bccr = 0x0010_2030;
            s.layers[0].whpcr = (0xF0 + 13) << 16 | (0xF0 + 10);
            s.layers[0].wvpcr = (0x18 + 21) << 16 | (0x18 + 20);
            s.layers[0].cfblr = 4 << 16 | 11;
        });
        transparent_default(&display);
        let frame = display.frame(ram(&[9; 8])).unwrap();
        assert_eq!(frame.pixel(9, 20), [0x10, 0x20, 0x30]);
        assert_eq!(frame.pixel(10, 20), [0, 0xFF, 0]);
        assert_eq!(frame.pixel(13, 21), [0, 0xFF, 0]);
        assert_eq!(frame.pixel(14, 21), [0x10, 0x20, 0x30]);
        assert_eq!(frame.pixel(10, 22), [0x10, 0x20, 0x30]);
        assert_eq!(frame.pixel(10, 19), [0x10, 0x20, 0x30]);
    }

    #[test]
    fn a_window_that_misses_the_active_area_or_ends_before_it_starts_draws_nothing() {
        for (whpcr, wvpcr) in [
            (0x0600_0500, WVPCR), // right of the active area
            (0x0100_0200, WVPCR), // ends before it starts
            (WHPCR, 0x0600_0500), // below it
            (WHPCR, 0x0100_0200),
        ] {
            let display = board(L8, LEN | CLUTEN);
            display.update(|s| {
                s.clut[0][1] = 0x00FF_FFFF;
                s.layers[0].whpcr = whpcr;
                s.layers[0].wvpcr = wvpcr;
            });
            transparent_default(&display);
            let frame = display.frame(ram(&[1; 800 * 600])).unwrap();
            assert!(frame.rgb.iter().all(|&b| b == 0), "{whpcr:#x} {wvpcr:#x}");
        }
    }

    #[test]
    fn a_disabled_layer_shows_the_background() {
        let display = board(L8, CLUTEN);
        display.update(|s| s.bccr = 0x0001_0203);
        let frame = display.frame(ram(&[])).unwrap();
        assert_eq!(frame.pixel(0, 0), [1, 2, 3]);
        assert_eq!(frame.pixel(799, 599), [1, 2, 3]);
    }

    #[test]
    fn rgb565_is_widened_to_eight_bits() {
        let display = board(RGB565, LEN);
        pitch16(&display);
        let mut memory = vec![0; 1600 * 600];
        memory[..2].copy_from_slice(&0xF800u16.to_le_bytes());
        memory[2..4].copy_from_slice(&0x07E0u16.to_le_bytes());
        memory[4..6].copy_from_slice(&0x001Fu16.to_le_bytes());
        memory[6..8].copy_from_slice(&0x8410u16.to_le_bytes());
        let frame = display.frame(ram(&memory)).unwrap();
        assert_eq!(frame.pixel(0, 0), [255, 0, 0]);
        assert_eq!(frame.pixel(1, 0), [0, 255, 0]);
        assert_eq!(frame.pixel(2, 0), [0, 0, 255]);
        // 10000 100000 10000: the high bits fill the low ones.
        assert_eq!(frame.pixel(3, 0), [0x84, 0x82, 0x84]);
    }

    #[test]
    fn rgb888_is_stored_blue_first() {
        let display = board(RGB888, LEN);
        display.update(|s| s.layers[0].cfblr = 2400 << 16 | 2407);
        let mut memory = vec![0; 2400 * 600];
        memory[..3].copy_from_slice(&[0x33, 0x22, 0x11]);
        let frame = display.frame(ram(&memory)).unwrap();
        assert_eq!(frame.pixel(0, 0), [0x11, 0x22, 0x33]);
    }

    #[test]
    fn argb8888_is_stored_blue_first_with_the_alpha_last() {
        let display = board(ARGB8888, LEN);
        display.update(|s| s.layers[0].cfblr = 3200 << 16 | 3207);
        let mut memory = vec![0; 3200 * 600];
        memory[..4].copy_from_slice(&[0x33, 0x22, 0x11, 0xFF]);
        // The constant alpha is the blending factor here, so the pixel's own alpha does not
        // count.
        memory[4..8].copy_from_slice(&[0x66, 0x55, 0x44, 0x00]);
        let frame = display.frame(ram(&memory)).unwrap();
        assert_eq!(frame.pixel(0, 0), [0x11, 0x22, 0x33]);
        assert_eq!(frame.pixel(1, 0), [0x44, 0x55, 0x66]);
    }

    #[test]
    fn argb4444_widens_each_nibble_by_repeating_it() {
        let display = board(ARGB4444, LEN);
        pitch16(&display);
        let memory = pixels16(0xF123, 0xFFFF);
        let frame = display.frame(ram(&memory)).unwrap();
        assert_eq!(frame.pixel(0, 0), [0x11, 0x22, 0x33]);
        assert_eq!(frame.pixel(1, 0), [0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn argb1555_widens_the_colours_like_rgb565_and_the_alpha_to_0_or_255() {
        let display = board(ARGB1555, LEN);
        pitch16(&display);
        let memory = pixels16(0x8000 | 31 << 10 | 16 << 5 | 1, 0);
        let frame = display.frame(ram(&memory)).unwrap();
        // 31 is 255, 16 is 10000 which fills to 10000100, 1 is 00001 which fills to 00001000.
        assert_eq!(frame.pixel(0, 0), [255, 0x84, 0x08]);
    }

    #[test]
    fn the_alpha_of_argb1555_is_all_or_nothing() {
        let display = board(ARGB1555, LEN);
        pitch16(&display);
        display.update(|s| {
            s.bccr = 0x0001_0203;
            s.layers[0].bfcr = BF_PIXEL;
            s.layers[0].dccr = 0;
        });
        let memory = pixels16(0x8000 | 31 << 10, 31 << 10);
        let frame = display.frame(ram(&memory)).unwrap();
        assert_eq!(frame.pixel(0, 0), [255, 0, 0]);
        assert_eq!(frame.pixel(1, 0), [1, 2, 3]);
    }

    #[test]
    fn constant_alpha_blends_the_layer_with_the_background() {
        // What the firmware does to 16 and 32-bit colour: constant alpha 0xE0 over black.
        let display = board(RGB565, LEN);
        pitch16(&display);
        display.update(|s| s.layers[0].cacr = 0xE0);
        let memory = pixels16(0xFFFF, 0xF800);
        let frame = display.frame(ram(&memory)).unwrap();
        assert_eq!(frame.pixel(0, 0), [0xE0, 0xE0, 0xE0]);
        assert_eq!(frame.pixel(1, 0), [0xE0, 0, 0]);
    }

    // The tests that round pin this model's rounding (to the nearest level), which is my choice
    // and not measured on the chip: RM0433 is to be checked for it.
    #[test]
    fn blending_rounds_to_the_nearest_level() {
        // 128/255 of 255 over 100 is 128 + 127/255 x 100 = 177.8.
        let display = board(RGB565, LEN);
        pitch16(&display);
        display.update(|s| {
            s.bccr = 0x0064_6464;
            s.layers[0].cacr = 0x80;
            s.layers[0].dccr = 0;
        });
        let memory = pixels16(0xFFFF, 0);
        let frame = display.frame(ram(&memory)).unwrap();
        assert_eq!(frame.pixel(0, 0), [178, 178, 178]);
        // And 128/255 of 0 over 100 is 49.8.
        assert_eq!(frame.pixel(1, 0), [50, 50, 50]);
    }

    #[test]
    fn pixel_alpha_times_constant_alpha_blends_with_what_is_below() {
        let display = board(ARGB4444, LEN);
        pitch16(&display);
        display.update(|s| {
            s.bccr = 0x0000_0064;
            s.layers[0].bfcr = BF_PIXEL;
            s.layers[0].dccr = 0;
        });
        // Alpha 8 is 136, so 136/255 of the red over 136/255 of the blue below: the blue is
        // 100 x 119/255 = 46.7.
        let memory = pixels16(0x8F00, 0x0F00);
        let frame = display.frame(ram(&memory)).unwrap();
        assert_eq!(frame.pixel(0, 0), [136, 0, 47]);
        // Alpha 0 shows the background whole.
        assert_eq!(frame.pixel(1, 0), [0, 0, 100]);
    }

    #[test]
    fn the_constant_alpha_scales_the_pixel_alpha() {
        let display = board(ARGB4444, LEN);
        pitch16(&display);
        display.update(|s| {
            s.layers[0].bfcr = BF_PIXEL;
            s.layers[0].cacr = 0x80;
            s.layers[0].dccr = 0;
        });
        // 136 x 128 / 255 = 68.3.
        let memory = pixels16(0x8F00, 0);
        let frame = display.frame(ram(&memory)).unwrap();
        assert_eq!(frame.pixel(0, 0), [68, 0, 0]);
    }

    #[test]
    fn the_product_of_the_alphas_is_rounded_to_the_nearest_level() {
        // 136 x 129 / 255 is 68.8.
        let display = board(ARGB4444, LEN);
        pitch16(&display);
        display.update(|s| {
            s.layers[0].bfcr = BF_PIXEL;
            s.layers[0].cacr = 0x81;
        });
        let memory = pixels16(0x8F00, 0);
        let frame = display.frame(ram(&memory)).unwrap();
        assert_eq!(frame.pixel(0, 0), [69, 0, 0]);
    }

    #[test]
    fn layer_2_is_blended_over_layer_1() {
        let display = two_layers(RGB565, ARGB4444);
        display.update(|s| {
            // Layer 2's pixels are the next page along: 800 x 2 bytes on from layer 1's.
            s.layers[0].cfblr = 1600 << 16 | 1607;
            s.layers[1].cfbar = BASE + 1600 * 600;
            s.layers[1].cfblr = 1600 << 16 | 1607;
        });
        let mut memory = vec![0; 2 * 1600 * 600];
        // Layer 1 is red, layer 2 half transparent green, then opaque, then nothing.
        for x in 0..3 {
            memory[2 * x..2 * x + 2].copy_from_slice(&0xF800u16.to_le_bytes());
        }
        let page2 = 1600 * 600;
        memory[page2..page2 + 2].copy_from_slice(&0x80F0u16.to_le_bytes());
        memory[page2 + 2..page2 + 4].copy_from_slice(&0xF0F0u16.to_le_bytes());
        let frame = display.frame(ram(&memory)).unwrap();
        // Alpha 8 of ARGB4444 is 8 x 17 = 136 of 255, so layer 2's green counts 136/255 and the
        // red of layer 1 below it 119/255. Both products come to a whole level (136 x 255 / 255
        // and 119 x 255 / 255), so no rounding is pinned here.
        let alpha = 8 * 17;
        assert_eq!(frame.pixel(0, 0), [255 - alpha, alpha, 0]);
        assert_eq!(frame.pixel(1, 0), [0, 0xFF, 0]);
        assert_eq!(frame.pixel(2, 0), [255, 0, 0]);
    }

    #[test]
    fn layer_2_alone_shows_over_the_background() {
        // The 12-bit depth with page 0 transparent: the background colour shows through.
        let display = two_layers(ARGB4444, ARGB4444);
        pitch16(&display);
        display.update(|s| {
            s.bccr = 0x0010_2030;
            s.layers[0].dccr = 0;
            s.layers[0].bfcr = BF_PIXEL;
        });
        let memory = pixels16(0, 0);
        let frame = display.frame(ram(&memory)).unwrap();
        assert_eq!(frame.pixel(0, 0), [0x10, 0x20, 0x30]);
    }

    #[test]
    fn a_layer_that_is_off_adds_nothing_whatever_its_other_registers_say() {
        // The firmware's layer 2 after the 12-bit depth: everything zero, blending factors too.
        let display = board(L8, CLUTEN);
        display.update(|s| {
            s.bccr = 0x0001_0203;
            s.layers[0].dccr = 0xFF10_2030;
            s.layers[1] = Layer {
                bfcr: 0,
                ..Layer::default()
            };
        });
        let frame = display.frame(ram(&[])).unwrap();
        assert_eq!(frame.pixel(0, 0), [1, 2, 3]);
        assert_eq!(frame.pixel(799, 599), [1, 2, 3]);
    }

    #[test]
    fn the_default_colour_fills_what_the_window_leaves_and_is_blended_by_its_alpha() {
        let display = board(RGB565, LEN);
        display.update(|s| {
            s.bccr = 0x00FF_FFFF;
            s.layers[0].bfcr = BF_PIXEL;
            s.layers[0].dccr = 0x8010_2030;
            s.layers[0].whpcr = (0xF0 + 1) << 16 | 0xF0;
            s.layers[0].cfblr = 1600 << 16 | 11;
        });
        let memory = pixels16(0xF800, 0xF800);
        let frame = display.frame(ram(&memory)).unwrap();
        assert_eq!(frame.pixel(1, 0), [255, 0, 0]);
        // Alpha 128 of (0x10, 0x20, 0x30) over white.
        assert_eq!(frame.pixel(2, 0), [135, 143, 151]);
    }

    #[test]
    fn a_layer_that_is_off_with_the_reset_blending_leaves_the_picture_alone() {
        let display = two_layers(RGB565, RGB565);
        pitch16(&display);
        display.update(|s| s.layers[1].cr = 0);
        let memory = pixels16(0xF800, 0xF800);
        let frame = display.frame(ram(&memory)).unwrap();
        assert_eq!(frame.pixel(0, 0), [255, 0, 0]);
    }

    #[test]
    fn blending_factors_that_the_firmware_does_not_use_are_reported() {
        // Neither a factor of 0 nor of 1, nor the products of the layer's own alpha, which are
        // other values of BF1 and BF2.
        for bfcr in [0x0000, 0x0405 ^ 0x0100, 0x0405 ^ 0x0001, 0x0707, 0x0506] {
            let display = board(RGB565, LEN);
            display.update(|s| s.layers[0].bfcr = bfcr);
            assert_eq!(
                display.frame(ram(&[])),
                Err(FrameError::Unsupported("blending factors")),
                "{bfcr:#x}"
            );
        }
    }

    #[test]
    fn the_blending_factors_can_be_paired_either_way() {
        for (bf1, bf2) in [(4, 5), (4, 7), (6, 5), (6, 7)] {
            let display = board(RGB565, LEN);
            pitch16(&display);
            display.update(|s| s.layers[0].bfcr = bf1 << 8 | bf2);
            assert!(display.frame(ram(&pixels16(0, 0))).is_ok(), "{bf1} {bf2}");
        }
    }

    #[test]
    fn formats_that_are_not_drawn_are_reported() {
        for format in [6, 7] {
            let display = board(format, LEN);
            assert_eq!(
                display.frame(ram(&[])),
                Err(FrameError::Unsupported("pixel format AL44 or AL88")),
                "format {format}"
            );
        }
    }

    #[test]
    fn a_layer_that_is_off_is_not_held_to_its_format() {
        let display = board(7, 0);
        assert!(display.frame(ram(&[])).is_ok());
    }

    #[test]
    fn l8_needs_the_colour_table() {
        let display = board(L8, LEN);
        assert_eq!(
            display.frame(ram(&[])),
            Err(FrameError::Unsupported("L8 without the colour table"))
        );
    }

    #[test]
    fn colour_keying_is_reported_for_either_layer() {
        let display = board(L8, LEN | CLUTEN | COLKEN);
        assert_eq!(
            display.frame(ram(&[])),
            Err(FrameError::Unsupported("colour keying"))
        );
        let display = two_layers(RGB565, RGB565);
        pitch16(&display);
        display.update(|s| s.layers[1].cr |= COLKEN);
        assert_eq!(
            display.frame(ram(&pixels16(0, 0))),
            Err(FrameError::Unsupported("colour keying"))
        );
    }

    #[test]
    fn a_framebuffer_outside_ram_is_reported_at_its_first_missing_line() {
        let display = board(L8, LEN | CLUTEN);
        // Two whole lines are there, the third is not.
        assert_eq!(
            display.frame(ram(&[0; 1600])),
            Err(FrameError::Unreadable(BASE + 1600))
        );
    }

    #[test]
    fn an_active_area_with_no_width_or_no_height_is_not_a_picture() {
        // The end of the active width at or before the back porch, or the same for the height,
        // with a layer on and with none: nothing is scanned out, and nothing panics.
        for (bpcr, awcr) in [
            (BPCR, BPCR),
            (BPCR, BPCR & 0xFFFF_0000 | (AWCR & 0x7FF)),
            (BPCR, AWCR & 0xFFFF_0000 | (BPCR & 0x7FF)),
            (BPCR, 0),
        ] {
            for cr in [LEN | CLUTEN, 0] {
                let display = board(L8, cr);
                display.update(|s| {
                    s.bpcr = bpcr;
                    s.awcr = awcr;
                });
                assert_eq!(
                    display.frame(ram(&vec![0; 800 * 600])),
                    Err(FrameError::Empty),
                    "{awcr:#x} {cr:#x}"
                );
            }
        }
    }

    #[test]
    fn a_window_wider_than_the_line_the_layer_fetches_is_reported() {
        // CFBLL is the bytes of a line plus 7, which is what the firmware writes; one byte less
        // is not drawn, and neither is the F4's plus 3.
        for (cfbll, drawn) in [(800 + 7, true), (800 + 6, false), (800 + 3, false)] {
            let display = board(L8, LEN | CLUTEN);
            display.update(|s| s.layers[0].cfblr = 800 << 16 | cfbll);
            let frame = display.frame(ram(&vec![0; 800 * 600]));
            if drawn {
                assert!(frame.is_ok(), "{cfbll}");
            } else {
                assert_eq!(
                    frame,
                    Err(FrameError::Unsupported(
                        "window wider than the fetched line"
                    )),
                    "{cfbll}"
                );
            }
        }
    }

    #[test]
    fn a_window_is_fetched_from_its_own_first_pixel_and_what_is_before_the_active_area_is_not_shown()
     {
        // The window starts 2 columns and 2 lines before the active area (WHSTPOS and WVSTPOS are
        // 2 less than the porch plus 1), so the framebuffer's pixel 2 of line 2 is the first
        // one shown. The window is 802 pixels wide and 602 lines.
        let display = board(L8, LEN | CLUTEN);
        display.update(|s| {
            s.clut[0][1] = 0x0011_2233;
            s.layers[0].whpcr = 0x040F_00EE;
            s.layers[0].wvpcr = 0x026F_0016;
            s.layers[0].cfblr = 800 << 16 | (802 + 7);
            s.layers[0].cfblnr = 602;
        });
        let mut memory = vec![0; 800 * 603];
        memory[0] = 1;
        memory[2] = 1;
        memory[800 * 2] = 1;
        memory[800 * 2 + 2] = 1;
        let frame = display.frame(ram(&memory)).unwrap();
        assert_eq!(frame.pixel(0, 0), [0x11, 0x22, 0x33]);
        assert_eq!(frame.pixel(1, 0), [0, 0, 0]);
        assert_eq!(frame.pixel(0, 1), [0, 0, 0]);
    }

    #[test]
    fn the_layer_fetches_only_the_lines_it_is_told_to() {
        // 3 lines of 800 are in RAM and CFBLNR says 3: the rest of the window is not fetched, so
        // it is not looked for either, and shows what is outside a layer.
        let display = board(L8, LEN | CLUTEN);
        display.update(|s| {
            s.bccr = 0x0001_0203;
            s.clut[0][1] = 0x00FF_FFFF;
            s.layers[0].cfblnr = 3;
            s.layers[0].dccr = 0;
            s.layers[0].bfcr = BF_PIXEL;
        });
        let frame = display.frame(ram(&vec![1; 800 * 3])).unwrap();
        assert_eq!(frame.pixel(0, 2), [255, 255, 255]);
        assert_eq!(frame.pixel(0, 3), [1, 2, 3]);
        assert_eq!(frame.pixel(799, 599), [1, 2, 3]);
    }

    #[test]
    fn a_window_with_no_width_fetches_nothing() {
        let display = board(L8, LEN | CLUTEN);
        display.update(|s| {
            s.layers[0].whpcr = 0x00EF_00F0;
            s.layers[0].dccr = 0;
            s.layers[0].bfcr = BF_PIXEL;
        });
        assert!(display.frame(ram(&[])).is_ok());
    }

    #[test]
    fn dividing_by_255_rounds_to_the_nearest_for_every_product_there_can_be() {
        // The most a blend adds up is 510 x 255.
        for x in 0..=130_050u32 {
            assert_eq!(div255(x), (f64::from(x) / 255.0).round() as u32, "{x}");
        }
    }

    // A small xorshift, so the random layers are the same every run.
    struct Random(u64);

    impl Random {
        fn below(&mut self, n: u32) -> u32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % u64::from(n)) as u32
        }
    }

    // A channel of `bits` bits as 8, the pattern repeated to fill the byte from the top.
    fn repeated(value: u32, bits: u32) -> u8 {
        (0..8).fold(0, |byte, i| {
            byte | ((value >> (bits - 1 - i % bits) & 1) as u8) << (7 - i)
        })
    }

    // What a pixel is by the book, one at a time, from the raw register values and nothing of the
    // code that builds a frame: where the window is, whether its line is fetched, the address of
    // the pixel, its decoding by PFCR, and the blend, with the sum divided by 255 to the nearest.
    // None if a layer's window needs more than the line it fetches.
    fn by_the_book(s: &Scanout, memory: &[u8]) -> Option<Vec<u8>> {
        let (h_porch, v_porch) = (i64::from(s.bpcr >> 16 & 0xFFF), i64::from(s.bpcr & 0x7FF));
        let width = i64::from(s.awcr >> 16 & 0xFFF) - h_porch;
        let height = i64::from(s.awcr & 0x7FF) - v_porch;
        let mut rgb = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let mut c = [(s.bccr >> 16) as u8, (s.bccr >> 8) as u8, s.bccr as u8];
                for (layer, clut) in s.layers.iter().zip(&s.clut) {
                    if layer.cr & 1 == 0 {
                        continue;
                    }
                    let left = i64::from(layer.whpcr & 0xFFF) - (h_porch + 1);
                    let right = i64::from(layer.whpcr >> 16 & 0xFFF) - h_porch;
                    let top = i64::from(layer.wvpcr & 0x7FF) - (v_porch + 1);
                    let bottom = i64::from(layer.wvpcr >> 16 & 0x7FF) - v_porch;
                    let bytes = [4, 3, 2, 2, 2, 1][(layer.pfcr & 7) as usize];
                    let inside = (left..right).contains(&x)
                        && (top..bottom).contains(&y)
                        && y - top < i64::from(layer.cfblnr & 0x7FF);
                    let (alpha, colour) = if inside {
                        if (right - left) * bytes > i64::from(layer.cfblr & 0x1FFF) - 7 {
                            return None;
                        }
                        let at = i64::from(layer.cfbar - BASE)
                            + i64::from(layer.cfblr >> 16 & 0x1FFF) * (y - top)
                            + (x - left) * bytes;
                        let raw = &memory[at as usize..(at + bytes) as usize];
                        let v =
                            u32::from(raw[0]) | u32::from(raw.get(1).copied().unwrap_or(0)) << 8;
                        match layer.pfcr & 7 {
                            0 => (raw[3], [raw[2], raw[1], raw[0]]),
                            1 => (255, [raw[2], raw[1], raw[0]]),
                            2 => (
                                255,
                                [
                                    repeated(v >> 11, 5),
                                    repeated(v >> 5 & 63, 6),
                                    repeated(v & 31, 5),
                                ],
                            ),
                            3 => (
                                if v >> 15 == 1 { 255 } else { 0 },
                                [
                                    repeated(v >> 10 & 31, 5),
                                    repeated(v >> 5 & 31, 5),
                                    repeated(v & 31, 5),
                                ],
                            ),
                            4 => (
                                repeated(v >> 12, 4),
                                [
                                    repeated(v >> 8 & 15, 4),
                                    repeated(v >> 4 & 15, 4),
                                    repeated(v & 15, 4),
                                ],
                            ),
                            _ => {
                                let entry = clut[raw[0] as usize];
                                (255, [(entry >> 16) as u8, (entry >> 8) as u8, entry as u8])
                            }
                        }
                    } else {
                        (
                            (layer.dccr >> 24) as u8,
                            [
                                (layer.dccr >> 16) as u8,
                                (layer.dccr >> 8) as u8,
                                layer.dccr as u8,
                            ],
                        )
                    };
                    let constant = u64::from(layer.cacr & 0xFF);
                    let times_pixel = (u64::from(alpha) * constant * 2 + 255) / 510;
                    let bf1 = if layer.bfcr >> 8 & 7 == 6 {
                        times_pixel
                    } else {
                        constant
                    };
                    let bf2 = 255
                        - if layer.bfcr & 7 == 7 {
                            times_pixel
                        } else {
                            constant
                        };
                    for (below, above) in c.iter_mut().zip(colour) {
                        let sum = bf1 * u64::from(above) + bf2 * u64::from(*below);
                        *below = ((sum * 2 + 255) / 510).min(255) as u8;
                    }
                }
                rgb.extend_from_slice(&c);
            }
        }
        Some(rgb)
    }

    #[test]
    fn a_frame_is_what_the_pixels_are_by_the_book_whatever_the_layers() {
        let mut random = Random(0x2545_F491_4F6C_DD1D);
        let (h_porch, v_porch) = (4, 3);
        let (mut drawn, mut rejected) = (0, 0);
        for trial in 0..600 {
            let (width, height) = (1 + random.below(23), 1 + random.below(9));
            let memory: Vec<u8> = (0..0x10000).map(|_| random.below(256) as u8).collect();
            let mut layers = [Layer::default(); 2];
            for (n, layer) in layers.iter_mut().enumerate() {
                let format = random.below(6);
                let bytes = [4, 3, 2, 2, 2, 1][format as usize];
                // The whole of the active area two times in five, else a window that starts up to
                // 3 before it and runs up to 2 past it.
                let full = random.below(5) < 2;
                let (left, right, top, bottom) = if full {
                    (0, width as i64, 0, height as i64)
                } else {
                    let left = random.below(width + 4) as i64 - 3;
                    let top = random.below(height + 4) as i64 - 3;
                    (
                        left,
                        left + random.below((width as i64 + 3 - left) as u32) as i64,
                        top,
                        top + random.below((height as i64 + 3 - top) as u32) as i64,
                    )
                };
                let window_bytes = (right - left) as u32 * bytes;
                let pitch = window_bytes + random.below(9);
                // The line length is enough nearly always, as the firmware writes it, and short now
                // and then.
                let cfbll = if random.below(4) == 0 {
                    random.below(window_bytes + 8)
                } else {
                    window_bytes + 7 + random.below(5)
                };
                let lines = (bottom - top) as u32;
                *layer = Layer {
                    cr: if random.below(10) == 0 { 0 } else { LEN }
                        | if format == L8 { CLUTEN } else { 0 },
                    whpcr: ((h_porch as i64 + right) as u32) << 16
                        | (h_porch as i64 + 1 + left) as u32,
                    wvpcr: ((v_porch as i64 + bottom) as u32) << 16
                        | (v_porch as i64 + 1 + top) as u32,
                    pfcr: format,
                    cacr: [0, 0x80, 0xE0, 0xFF, 1 + random.below(255)][random.below(5) as usize],
                    dccr: random.below(1 << 30) << 2 | random.below(4) << 30,
                    bfcr: [0x405, 0x407, 0x605, 0x607][random.below(4) as usize],
                    cfbar: BASE + n as u32 * 0x8000,
                    cfblr: pitch << 16 | cfbll,
                    cfblnr: (lines + random.below(3)).saturating_sub(random.below(2)),
                };
            }
            let display = Display::default();
            display.update(|s| {
                s.enabled = true;
                s.bpcr = h_porch << 16 | v_porch;
                s.awcr = (h_porch + width) << 16 | (v_porch + height);
                s.bccr = random.below(1 << 24);
                s.layers = layers;
                for clut in &mut s.clut {
                    *clut = std::array::from_fn(|_| random.below(1 << 24));
                }
            });
            let expected = by_the_book(&display.0.borrow(), &memory);
            match (display.frame(ram(&memory)), expected) {
                (Ok(frame), Some(expected)) => {
                    drawn += 1;
                    assert!(frame.rgb == expected, "trial {trial}: {layers:?}");
                }
                (Err(FrameError::Unsupported("window wider than the fetched line")), None) => {
                    rejected += 1
                }
                (got, expected) => {
                    panic!(
                        "trial {trial}: {got:?} against {:?}: {layers:?}",
                        expected.is_some()
                    )
                }
            }
        }
        assert!(
            drawn > 100 && rejected > 50,
            "{drawn} drawn, {rejected} rejected"
        );
    }

    // How long a frame takes to build, for the biggest frame of each depth that the firmware
    // makes and for the prompt; the two ARGB4444 layers are drawn (runs of opaque and clear
    // pixels) or have random alpha. Run in release:
    // cargo test -p cmm2-core --release compositing_speed -- --ignored --nocapture
    #[test]
    #[ignore = "a timing, not a check: see the comment"]
    fn compositing_speed() {
        const PORCH: (u32, u32) = (16, 8);
        let set_up = |width: u32, height: u32, format: u32, layers: usize, random_alpha: bool| {
            let (h, v) = PORCH;
            let display = Display::default();
            let pitch = width * [4, 3, 2, 2, 2, 1][format as usize];
            display.update(|s| {
                s.enabled = true;
                s.bpcr = h << 16 | v;
                s.awcr = (h + width) << 16 | (v + height);
                s.clut[0] = std::array::from_fn(|i| i as u32 * 0x0001_0101);
                for (n, layer) in s.layers.iter_mut().take(layers).enumerate() {
                    *layer = Layer {
                        cr: LEN | if format == L8 { CLUTEN } else { 0 },
                        whpcr: (h + width) << 16 | (h + 1),
                        wvpcr: (v + height) << 16 | (v + 1),
                        pfcr: format,
                        cacr: if format == ARGB4444 || format == L8 {
                            0xFF
                        } else {
                            0xE0
                        },
                        dccr: if n == 0 && layers == 1 {
                            0xFF00_0000
                        } else {
                            0
                        },
                        bfcr: if format == ARGB4444 {
                            BF_PIXEL
                        } else {
                            BF_CONSTANT
                        },
                        cfbar: BASE + n as u32 * pitch * height,
                        cfblr: pitch << 16 | (pitch + 7),
                        cfblnr: height,
                    };
                }
            });
            // Drawn pixels are opaque or clear, in runs of 64 as drawing leaves them; random ones
            // have every alpha, which is the worst case for the blend.
            let hash = |i: u32| (i.wrapping_mul(2_654_435_761) >> 13) as u16;
            let memory: Vec<u8> = if random_alpha || format != ARGB4444 {
                (0..layers as u32 * pitch * height)
                    .map(|i| hash(i) as u8)
                    .collect()
            } else {
                (0..layers as u32 * pitch * height / 2)
                    .flat_map(|i| {
                        let v = hash(i);
                        (if i / 64 % 4 == 0 { v | 0xF000 } else { 0 }).to_le_bytes()
                    })
                    .collect()
            };
            (display, memory)
        };
        // The biggest frames this firmware makes in each depth (MODE 16, 11,16, 10,32 and 18,12),
        // which are the worst cases: it has no 1920 x 1080 in the 12-bit depth.
        for (name, width, height, format, layers, random_alpha) in [
            ("1920 x 1080, L8, one layer", 1920, 1080, L8, 1, false),
            ("1280 x 720, RGB565, one layer", 1280, 720, RGB565, 1, false),
            (
                "848 x 480, ARGB8888, one layer",
                848,
                480,
                ARGB8888,
                1,
                false,
            ),
            (
                "1024 x 600, two ARGB4444 layers",
                1024,
                600,
                ARGB4444,
                2,
                false,
            ),
            (
                "1024 x 600, two ARGB4444 layers, random alpha",
                1024,
                600,
                ARGB4444,
                2,
                true,
            ),
            (
                "800 x 600, L8, one layer (the prompt)",
                800,
                600,
                L8,
                1,
                false,
            ),
        ] {
            let (display, memory) = set_up(width, height, format, layers, random_alpha);
            let ram = |addr: u32, len: usize| {
                let start = addr.checked_sub(BASE)? as usize;
                memory.get(start..start + len)
            };
            display.frame(ram).unwrap();
            let best = (0..40)
                .map(|_| {
                    let start = std::time::Instant::now();
                    std::hint::black_box(display.frame(ram).unwrap());
                    start.elapsed().as_secs_f64() * 1000.0
                })
                .fold(f64::MAX, f64::min);
            println!("{name}: {best:.2} ms (best of 40)");
        }
    }

    #[test]
    fn the_ppm_has_a_header_and_the_pixels() {
        let frame = Frame {
            width: 2,
            height: 1,
            rgb: vec![1, 2, 3, 4, 5, 6],
        };
        assert_eq!(frame.to_ppm(), b"P6\n2 1\n255\n\x01\x02\x03\x04\x05\x06");
    }
}
