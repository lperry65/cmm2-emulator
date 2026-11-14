//! Blending a layer over the picture so far, a line at a time.

use super::format::{Argb, Format, div255};
use super::{
    Active, BF1_CONSTANT, BF1_PIXEL_TIMES_CONSTANT, BF2_CONSTANT, BF2_PIXEL_TIMES_CONSTANT, COLKEN,
    FrameError, LEN, Layer,
};

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

/// CFBLL is the number of bytes in a line, plus 7 (RM0433 32.7.23, p. 1229). The F4 and F7 had 3.
const CFBLL_EXTRA: usize = 7;

/// The layer's default colour, ARGB in DCCR.
fn default_colour(layer: &Layer) -> Argb {
    Argb {
        alpha: (layer.dccr >> 24) as u8,
        rgb: [
            (layer.dccr >> 16) as u8,
            (layer.dccr >> 8) as u8,
            layer.dccr as u8,
        ],
    }
}

/// Blends one layer over `rgb`, the picture so far. A layer that is off blends its default colour
/// over the whole of it. Where the window of one that is on does not reach, its default colour
/// stands for its pixel, and so it does for the window's lines past the CFBLNR that the layer
/// fetches.
pub(super) fn composite<'a>(
    layer: &Layer,
    clut: &[u32; 256],
    active: &Active,
    rgb: &mut [u8],
    ram: &impl Fn(u32, usize) -> Option<&'a [u8]>,
) -> Result<(), FrameError> {
    if layer.cr & LEN == 0 {
        // RM0433 32.4.2 (p. 1212): the blending is done even when a layer is disabled, with the
        // default colour in place of the pixels, so the default colour covers the whole active
        // area. Factor codes that the manual reserves are not given a meaning there, and the
        // layer adds nothing.
        if let Ok(blending) = Blending::of(layer) {
            let default = default_colour(layer);
            blending
                .mix(default.alpha)
                .apply_to_all(default.rgb, rgb.as_chunks_mut::<3>().0);
        }
        return Ok(());
    }
    if layer.cr & COLKEN != 0 {
        return Err(FrameError::Unsupported("colour keying"));
    }
    let blending = Blending::of(layer)?;
    let format = Format::of(layer)?;
    let bytes_per_pixel = format.bytes_per_pixel();
    let default = default_colour(layer);
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
