//! What the LTDC puts on the screen, as pixels the host can look at.
//!
//! The controller model copies the registers that matter here as the firmware writes them, and
//! the machine's memory map builds a `Frame` from that and the emulated RAM. The layer
//! registers are only copied when the firmware reloads them (SRCR), as on the chip, where they
//! are shadowed. The colour table is written straight through.
//!
//! Only what the firmware uses is drawn: layer 1, with an opaque format (L8 through the colour
//! table, RGB565 or RGB888) and no colour key. A frame that needs anything else is reported as
//! unsupported instead of being drawn wrong. Layer 2, alpha blending and the other formats are
//! not drawn.

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
    pub(crate) layer: Layer,
    pub(crate) clut: [u32; 256],
}

impl Default for Scanout {
    fn default() -> Scanout {
        Scanout {
            enabled: false,
            bpcr: 0,
            awcr: 0,
            bccr: 0,
            layer: Layer::default(),
            clut: [0; 256],
        }
    }
}

/// The shadowed registers of layer 1, and the control register of layer 2.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Layer {
    pub(crate) cr: u32,
    pub(crate) whpcr: u32,
    pub(crate) wvpcr: u32,
    pub(crate) pfcr: u32,
    pub(crate) cacr: u32,
    pub(crate) cfbar: u32,
    pub(crate) cfblr: u32,
    pub(crate) layer2_cr: u32,
}

const LEN: u32 = 1 << 0;
const COLKEN: u32 = 1 << 1;
const CLUTEN: u32 = 1 << 4;

const ARGB8888: u32 = 0;
const RGB888: u32 = 1;
const RGB565: u32 = 2;
const L8: u32 = 5;

/// Why there is no frame to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// The controller is not enabled, so nothing is being scanned out.
    Off,
    /// The configuration needs something that is not drawn.
    Unsupported(&'static str),
    /// The framebuffer is not wholly in RAM; the address is the first byte that is not.
    Unreadable(u32),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::Off => f.write_str("the LTDC is not enabled"),
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
        // The registers hold the last position, counted from the start of the sync, so the
        // active area is the part between the back porch and the end of the active width.
        let (h_porch, v_porch) = (s.bpcr >> 16 & 0xFFF, s.bpcr & 0x7FF);
        let width = (s.awcr >> 16 & 0xFFF).saturating_sub(h_porch) as usize;
        let height = (s.awcr & 0x7FF).saturating_sub(v_porch) as usize;
        let background = [(s.bccr >> 16) as u8, (s.bccr >> 8) as u8, s.bccr as u8];
        let mut rgb = background.repeat(width * height);

        let layer = &s.layer;
        if layer.layer2_cr & LEN != 0 {
            return Err(FrameError::Unsupported("layer 2"));
        }
        if layer.cr & LEN == 0 {
            return Ok(Frame { width, height, rgb });
        }
        if layer.cr & COLKEN != 0 {
            return Err(FrameError::Unsupported("colour keying"));
        }
        if layer.cacr & 0xFF != 0xFF {
            return Err(FrameError::Unsupported("constant alpha below 255"));
        }
        let format = layer.pfcr & 7;
        let bytes_per_pixel = match format {
            L8 if layer.cr & CLUTEN == 0 => {
                return Err(FrameError::Unsupported("L8 without the colour table"));
            }
            L8 => 1,
            RGB565 => 2,
            RGB888 => 3,
            ARGB8888 => return Err(FrameError::Unsupported("pixel format ARGB8888")),
            _ => return Err(FrameError::Unsupported("pixel format with alpha")),
        };

        // The layer window is in the same coordinates as the porches: the first active pixel
        // is one past the back porch.
        let x0 = ((layer.whpcr & 0xFFF).saturating_sub(h_porch + 1) as usize).min(width);
        let x1 = (((layer.whpcr >> 16 & 0xFFF) + 1).saturating_sub(h_porch + 1) as usize)
            .clamp(x0, width);
        let y0 = ((layer.wvpcr & 0x7FF).saturating_sub(v_porch + 1) as usize).min(height);
        let y1 = (((layer.wvpcr >> 16 & 0x7FF) + 1).saturating_sub(v_porch + 1) as usize)
            .clamp(y0, height);
        let pitch = layer.cfblr >> 16 & 0x1FFF;
        for y in y0..y1 {
            let addr = layer
                .cfbar
                .wrapping_add(pitch.wrapping_mul((y - y0) as u32));
            let line =
                ram(addr, (x1 - x0) * bytes_per_pixel).ok_or(FrameError::Unreadable(addr))?;
            let row = &mut rgb[3 * (y * width + x0)..3 * (y * width + x1)];
            let (out, _) = row.as_chunks_mut::<3>();
            for (pixel, out) in line.chunks_exact(bytes_per_pixel).zip(out) {
                *out = colour(format, pixel, &s.clut);
            }
        }
        Ok(Frame { width, height, rgb })
    }
}

/// The colour of one pixel of `format`, as stored in memory.
fn colour(format: u32, pixel: &[u8], clut: &[u32; 256]) -> [u8; 3] {
    match format {
        L8 => {
            let c = clut[usize::from(pixel[0])];
            [(c >> 16) as u8, (c >> 8) as u8, c as u8]
        }
        RGB565 => {
            // The low bits are filled from the high ones, so full intensity is 255.
            let v = u16::from_le_bytes([pixel[0], pixel[1]]);
            let (r, g, b) = (v >> 11, v >> 5 & 0x3F, v & 0x1F);
            [
                (r << 3 | r >> 2) as u8,
                (g << 2 | g >> 4) as u8,
                (b << 3 | b >> 2) as u8,
            ]
        }
        _ => [pixel[2], pixel[1], pixel[0]],
    }
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

    fn board(format: u32, cr: u32) -> Display {
        let display = Display::default();
        display.update(|s| {
            s.enabled = true;
            s.bpcr = BPCR;
            s.awcr = AWCR;
            s.layer = Layer {
                cr,
                whpcr: WHPCR,
                wvpcr: WVPCR,
                pfcr: format,
                cacr: 0xFF,
                cfbar: BASE,
                cfblr: 800 << 16 | 807,
                layer2_cr: 0,
            };
        });
        display
    }

    // A RAM that holds `bytes` from `BASE` on and nothing else.
    fn ram<'a>(bytes: &'a [u8]) -> impl Fn(u32, usize) -> Option<&'a [u8]> {
        move |addr, len| {
            let start = addr.checked_sub(BASE)? as usize;
            bytes.get(start..start + len)
        }
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
            s.clut[1] = 0x0011_2233;
            s.clut[255] = 0x00AA_BBCC;
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
    fn the_pitch_steps_from_line_to_line() {
        let display = board(L8, LEN | CLUTEN);
        display.update(|s| {
            s.clut[7] = 0x0000_00FF;
            s.layer.cfblr = 1024 << 16 | 807;
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
            s.clut[1] = 0x00FF_FFFF;
            s.layer.whpcr = 0xF3 << 16 | 0xF0;
            s.layer.wvpcr = 0x19 << 16 | 0x18;
            s.layer.cfblr = 5000 << 16;
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
            s.clut[9] = 0x0000_FF00;
            s.bccr = 0x0010_2030;
            s.layer.whpcr = (0xF0 + 13) << 16 | (0xF0 + 10);
            s.layer.wvpcr = (0x18 + 21) << 16 | (0x18 + 20);
            s.layer.cfblr = 4 << 16 | 7;
        });
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
                s.clut[1] = 0x00FF_FFFF;
                s.layer.whpcr = whpcr;
                s.layer.wvpcr = wvpcr;
            });
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
        display.update(|s| s.layer.cfblr = 1600 << 16 | 1603);
        let mut memory = vec![0; 1600 * 600];
        memory[..2].copy_from_slice(&0xF800u16.to_le_bytes());
        memory[2..4].copy_from_slice(&0x07E0u16.to_le_bytes());
        memory[4..6].copy_from_slice(&0x001Fu16.to_le_bytes());
        let frame = display.frame(ram(&memory)).unwrap();
        assert_eq!(frame.pixel(0, 0), [255, 0, 0]);
        assert_eq!(frame.pixel(1, 0), [0, 255, 0]);
        assert_eq!(frame.pixel(2, 0), [0, 0, 255]);
    }

    #[test]
    fn rgb888_is_stored_blue_first() {
        let display = board(RGB888, LEN);
        display.update(|s| s.layer.cfblr = 2400 << 16 | 2403);
        let mut memory = vec![0; 2400 * 600];
        memory[..3].copy_from_slice(&[0x33, 0x22, 0x11]);
        let frame = display.frame(ram(&memory)).unwrap();
        assert_eq!(frame.pixel(0, 0), [0x11, 0x22, 0x33]);
    }

    #[test]
    fn formats_that_are_not_drawn_are_reported() {
        for (format, name) in [
            (ARGB8888, "pixel format ARGB8888"),
            (3, "pixel format with alpha"),
            (4, "pixel format with alpha"),
            (6, "pixel format with alpha"),
            (7, "pixel format with alpha"),
        ] {
            let display = board(format, LEN);
            assert_eq!(
                display.frame(ram(&[])),
                Err(FrameError::Unsupported(name)),
                "format {format}"
            );
        }
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
    fn colour_keying_blending_and_layer_2_are_reported() {
        let display = board(L8, LEN | CLUTEN | COLKEN);
        assert_eq!(
            display.frame(ram(&[])),
            Err(FrameError::Unsupported("colour keying"))
        );
        let display = board(L8, LEN | CLUTEN);
        display.update(|s| s.layer.cacr = 0x80);
        assert_eq!(
            display.frame(ram(&[])),
            Err(FrameError::Unsupported("constant alpha below 255"))
        );
        let display = board(L8, LEN | CLUTEN);
        display.update(|s| s.layer.layer2_cr = LEN);
        assert_eq!(
            display.frame(ram(&[])),
            Err(FrameError::Unsupported("layer 2"))
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
    fn the_ppm_has_a_header_and_the_pixels() {
        let frame = Frame {
            width: 2,
            height: 1,
            rgb: vec![1, 2, 3, 4, 5, 6],
        };
        assert_eq!(frame.to_ppm(), b"P6\n2 1\n255\n\x01\x02\x03\x04\x05\x06");
    }
}
