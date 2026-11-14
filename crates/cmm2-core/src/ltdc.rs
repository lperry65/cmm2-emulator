//! The LCD-TFT controller. It keeps the timing the firmware waits on, and the registers that
//! say what is on the screen, which it passes to the display (see `display.rs`) for the host to
//! look at. After every frame's active area the firmware's display code expects a line interrupt (IRQ
//! 88), and the BASIC interpreter's main thread waits for it, so without it the prompt never
//! runs a command.
//!
//! The frame is worked out from the machine's clock, from the total size the firmware writes to
//! TWCR and the pixel clock the firmware has programmed into PLL3 (see `rcc.rs`): 49.5 MHz for
//! 800 x 600, which comes round at 75 Hz, and 31.4 MHz for 640 x 480, which comes round at
//! 74.725 Hz. The board measured 748 ticks in 10 s there, 74.8 +- 0.1 Hz, which agrees with
//! that but does not confirm its decimals. The controller runs from the pixel clock as it is:
//! the frame starts when the controller is enabled and the clock runs, and when the clock
//! starts, stops or changes under an enabled controller the counters keep their position and go
//! on at the new rate from the cycle of the change (the clock asks for a tick when it changes, so
//! the controller hears of it at once). While the clock is stopped the counters stay where they
//! were. Once the controller is enabled (GCR.LTDCEN) the
//! line in LIPCR is reached once a frame, which sets ISR.LIF and, with IER.LIE set, raises the
//! interrupt. ICR clears the flags. The flag is worked out whenever ISR is read, so it does not
//! need the interrupt to be on; the interrupt asks the clock for a tick, because the firmware
//! does not touch the controller while it waits. A line event is flagged once: writing the
//! registers it depends on again in the pixel it was flagged in does not flag it a second time.
//! A tick is never asked for sooner than MIN_WAKE_GAP cycles ahead, so a frame of a few pixels
//! cannot make the CPU stop for the controller at every instruction.
//!
//! Lines are counted from 0 at the first line of the vertical sync (RM0433 32.4.1, p. 1207: when
//! the controller is enabled the timings start at X/Y = 0/0 as the first sync pixel), the way
//! the porch registers count them, so LIPOS = AAH is the last line of the active area and the
//! firmware's AAH + 1 the first line after it. The manual does not say how far into the line the
//! interrupt comes: the model flags it at the start of the line.
//!
//! A write of IMR to SRCR reloads the shadow registers at once: the bit reads back 0 and
//! ISR.RRIF is set, and the layer registers the display uses are those as of that write. A write
//! of VBR (bit 1) holds the reload back to the next vertical blank, the first line after the
//! active area (AWCR), as the chip does: the bit reads 1 until then, then the display takes the
//! registers as they are at that moment and RRIF is set. A frame that is not running (the
//! controller off) has no blank, so the reload waits for the first one after it is enabled.
//! The firmware uses VBR only once in each MODE command, to complete the layer registers it
//! has cleared; its page flips (PAGE DISPLAY) write the layer's registers from the line
//! interrupt and reload them with IMR. The layer registers are shadowed, as RM0433 32.4.1
//! (p. 1208) and 32.7.6 (p. 1218) have it: a write goes to a shadow copy, a read returns the
//! active value, and the new one can only be read after the reload (CLUTWR is the one layer
//! register that is not shadowed). LxCACR resets to 0xFF (32.7.19, p. 1226) although ST's SVD
//! says 0. A write to CLUTWR goes into the layer's colour table at once. The other
//! flags (FU, TERR) are never set, as nothing is fetched, so the error interrupt (IRQ 89) is
//! never raised. CDSR and CPSR, the position the controller is at, are plain stored registers:
//! the firmware does not read them.
//!
//! The interrupt line is a level on the real chip, but `Irqs` is edge like, so the line is raised
//! when a flag is set that is enabled, and when an enable is written for a flag that is set.

use crate::clock::{CORE_HZ, Clock};
use crate::display::{CACR_RESET, Display, Layer, Scanout};
use crate::irq::Irqs;
use crate::peripheral::{ResetCause, StubRegisters};
use crate::rcc::PixelClock;
use crate::registers::{Registers, WordAccess};
use crate::svd::Svd;

pub const LTDC_BASE: u32 = 0x5000_1000;
pub const LTDC_SIZE: u32 = 0x1000;

/// `LTDC_IRQn`, the global interrupt.
const LTDC_IRQ: u16 = 88;

// Register offsets and bit positions, as the SVD gives them.
const BPCR: u32 = 0x0C;
const AWCR: u32 = 0x10;
const TWCR: u32 = 0x14;
const GCR: u32 = 0x18;
const BCCR: u32 = 0x2C;
const SRCR: u32 = 0x24;
const IER: u32 = 0x34;
const ISR: u32 = 0x38;
const ICR: u32 = 0x3C;
const LIPCR: u32 = 0x40;
// The registers of a layer, counted from its CR. Layer 1's CR is at 0x84, and each layer after
// it is 0x80 on.
const LAYER_CR: u32 = 0x84;
const LAYER_STRIDE: u32 = 0x80;
const LAYERS: usize = 2;
const WHPCR: u32 = 0x04;
const WVPCR: u32 = 0x08;
const PFCR: u32 = 0x10;
const CACR: u32 = 0x14;
const DCCR: u32 = 0x18;
const BFCR: u32 = 0x1C;
const CFBAR: u32 = 0x28;
const CFBLR: u32 = 0x2C;
const CFBLNR: u32 = 0x30;
const CLUTWR: u32 = 0x40;

/// The soonest the controller asks to be woken, in core cycles (about 2 us).
const MIN_WAKE_GAP: u64 = 1000;

const LTDCEN: u32 = 1 << 0;
const LIF: u32 = 1 << 0;
const RRIF: u32 = 1 << 3;
const FLAGS: u32 = 0xF;
const IMR: u32 = 1 << 0;
const VBR: u32 = 1 << 1;
const LIPOS_MASK: u32 = 0x7FF;
const AAH_MASK: u32 = 0x7FF;

/// The registers are stored, except ISR, which is worked out, and the display gets a copy of
/// those it draws from. The layer registers are shadowed (RM0433 32.4.1, p. 1208): `stored`
/// has what was written, `active` what the last reload took, which is what a read returns. `pixel_hz` is the pixel clock the controller last saw. While it is
/// enabled with a pixel clock, `frame_start` is the core cycle at which the frame was at pixel
/// `anchor_pixel` (0 when the controller was enabled, and where the counters were when the clock
/// last changed) and `next_line` the pixel, counted from the start, of the next line event that
/// has not been flagged yet; they are `None` otherwise. `next_line` is also `None` when LIPCR is beyond the last line, which is
/// never reached. `flagged_until` is the pixel before which no line event is flagged again.
/// `reload_pending` is a VBR that has not completed yet, and `next_blank` the pixel it will,
/// which is `None` when the frame is not running.
#[derive(Debug)]
pub struct LtdcRegisters {
    stored: StubRegisters,
    active: StubRegisters,
    clock: Clock,
    irqs: Irqs,
    display: Display,
    pixel_clock: PixelClock,
    isr: u32,
    frame_start: Option<u64>,
    anchor_pixel: u64,
    pixel_hz: u64,
    next_line: Option<u64>,
    flagged_until: u64,
    reload_pending: bool,
    next_blank: Option<u64>,
}

/// Which layer a register is in, and how far into the layer's block (from its CR): `None` for
/// the registers before and after the layers.
fn layer_register(offset: u32) -> Option<(usize, u32)> {
    let from_layer_1 = offset.checked_sub(LAYER_CR)?;
    let layer = (from_layer_1 / LAYER_STRIDE) as usize;
    (layer < LAYERS).then_some((layer, from_layer_1 % LAYER_STRIDE))
}

/// The pixels drawn in `cycles` core cycles at `hz`, whole ones.
fn cycles_to_pixels(cycles: u64, hz: u64) -> u64 {
    (u128::from(cycles) * u128::from(hz) / u128::from(CORE_HZ)) as u64
}

/// The first whole cycle at which `pixels` pixels have been drawn, at `hz`.
fn pixels_to_cycles(pixels: u64, hz: u64) -> u64 {
    (u128::from(pixels) * u128::from(CORE_HZ)).div_ceil(u128::from(hz)) as u64
}

/// The registers of a layer's block that are shadowed: all but CLUTWR (RM0433 32.4.1, p. 1208).
fn shadowed(offset: u32) -> bool {
    layer_register(offset).is_some_and(|(_, register)| register != CLUTWR)
}

/// The registers as a reset leaves them. ST's SVD has 0 for LxCACR, where RM0433 32.7.19
/// (p. 1226) gives 0xFF, which the manual wins.
fn reset_values() -> (StubRegisters, StubRegisters) {
    let mut stored = StubRegisters::with_reset_values(LTDC_BASE, Svd::stm32h743());
    let mut active = StubRegisters::with_reset_values(LTDC_BASE, Svd::stm32h743());
    for layer in 0..LAYERS as u32 {
        let cacr = LAYER_CR + LAYER_STRIDE * layer + CACR;
        stored.write(cacr, CACR_RESET, !0);
        active.write(cacr, CACR_RESET, !0);
    }
    (stored, active)
}

impl LtdcRegisters {
    /// The total width and height in pixels and lines. The registers hold them less one.
    fn total_size(&mut self) -> (u64, u64) {
        let twcr = self.stored.read(TWCR);
        (
            u64::from(twcr >> 16 & 0xFFF) + 1,
            u64::from(twcr & 0x7FF) + 1,
        )
    }

    /// The pixel the frame is at, counted from the start of the frame it was enabled in, at core
    /// cycle `cycle`. While the pixel clock is stopped the counters stay where they were.
    fn pixel_at(&self, cycle: u64) -> u64 {
        match self.frame_start {
            Some(start) => {
                self.anchor_pixel + cycles_to_pixels(cycle.saturating_sub(start), self.pixel_hz)
            }
            None => self.anchor_pixel,
        }
    }

    /// The core cycle at which the frame gets to `pixel`, for a frame that is running.
    fn cycle_of(&self, start: u64, pixel: u64) -> u64 {
        start + pixels_to_cycles(pixel.saturating_sub(self.anchor_pixel), self.pixel_hz)
    }

    /// The first pixel from `from` on that is the start of `line`, in this frame or a later
    /// one, or `None` if the frame has no such line.
    fn line_start_from(&mut self, line: u64, from: u64) -> Option<u64> {
        let (width, height) = self.total_size();
        if line >= height {
            return None;
        }
        let frame = width * height;
        let first = line * width;
        Some(if from <= first {
            first
        } else {
            first + (from - first).div_ceil(frame) * frame
        })
    }

    /// The next time from `from` on that the line in LIPCR is reached.
    fn line_event_from(&mut self, from: u64) -> Option<u64> {
        let line = u64::from(self.stored.read(LIPCR) & LIPOS_MASK);
        self.line_start_from(line, from)
    }

    /// The next time from `from` on that the vertical blank starts: the line after the last
    /// of the active area. If the active area ends at or beyond the last line there is no such
    /// line, and I take the blank to be the first line of the next frame (my reading of the
    /// counters wrapping: RM0433 32.7.6 says only "the first line after the active display
    /// area").
    fn blank_event_from(&mut self, from: u64) -> Option<u64> {
        let (_, height) = self.total_size();
        let after = u64::from(self.stored.read(AWCR) & AAH_MASK) + 1;
        self.line_start_from(if after < height { after } else { 0 }, from)
    }

    /// Takes up the changes of the pixel clock, each at the cycle it was made. The counters keep
    /// their position and only the rate changes: from the cycle of the change the frame runs at
    /// the new rate from where it was, or, if the clock has stopped, stays there until it starts
    /// again. What the old rate had reached by then is not lost.
    fn follow_pixel_clock(&mut self) {
        for (at, hz) in self.pixel_clock.take_changes() {
            self.catch_up_to(at);
            let position = self.pixel_at(at);
            self.run_from(at, position, hz);
        }
    }

    /// Starts a frame at core cycle `at`, if the controller is enabled and has a pixel clock, and
    /// ends the one there is.
    fn restart(&mut self, at: u64) {
        self.flagged_until = 0;
        self.run_from(at, 0, self.pixel_clock.hz());
    }

    /// The frame is at pixel `position` at core cycle `at`, and runs at the
    /// pixel clock `hz`, if it is enabled and the clock is going.
    fn run_from(&mut self, at: u64, position: u64, hz: u64) {
        self.pixel_hz = hz;
        let enabled = self.stored.read(GCR) & LTDCEN != 0;
        self.frame_start = (enabled && self.pixel_hz != 0).then_some(at);
        self.anchor_pixel = position;
        self.arm_from(position);
    }

    fn catch_up(&mut self) {
        self.follow_pixel_clock();
        self.catch_up_to(self.clock.now());
    }

    /// Flags the line event if it has come by core cycle `cycle`, and moves on to the next one,
    /// and does the same for a reload that was held back to the blank. However many frames have
    /// passed, a flag is set once.
    fn catch_up_to(&mut self, cycle: u64) {
        if self.frame_start.is_none() {
            return;
        }
        let now = self.pixel_at(cycle);
        if self.next_line.is_some_and(|due| now >= due) {
            self.isr |= LIF;
            // Nothing up to this pixel is flagged again, however the registers are written.
            self.flagged_until = now + 1;
            self.next_line = self.line_event_from(now + 1);
            self.raise_if_enabled();
        }
        if self.next_blank.is_some_and(|due| now >= due) {
            self.reload();
            self.reload_pending = false;
            self.next_blank = None;
        }
    }

    /// Works out the next line and blank events again, after the controller or what they depend
    /// on changed, and has the clock tick when they come.
    fn arm(&mut self) {
        let from = self.pixel_at(self.clock.now());
        self.arm_from(from);
    }

    /// The same, for the events from pixel `from` of the frame on. The line events that have been
    /// flagged do not count.
    fn arm_from(&mut self, from: u64) {
        if self.frame_start.is_none() {
            self.next_line = None;
            self.next_blank = None;
            return;
        }
        self.next_line = self.line_event_from(from.max(self.flagged_until));
        self.next_blank = self
            .reload_pending
            .then(|| self.blank_event_from(from))
            .flatten();
        self.schedule();
    }

    // A tick that comes for another device lands here too, as the clock keeps one wake. The
    // line only needs one if its interrupt is on, but a reload that is held back does, so that
    // the display has the registers by the time the frame is looked at. No wake is asked for
    // sooner than MIN_WAKE_GAP from now: a frame of a few pixels, which the firmware can program
    // but which no display could show, would otherwise have a wake for every instruction. The
    // flags are worked out as they are read, so this only delays the interrupt.
    fn schedule(&mut self) {
        let Some(start) = self.frame_start else {
            return;
        };
        let soonest = self.clock.now() + MIN_WAKE_GAP;
        let line = self.next_line.filter(|_| self.stored.read(IER) & LIF != 0);
        for due in line.into_iter().chain(self.next_blank) {
            self.clock.wake_at(self.cycle_of(start, due).max(soonest));
        }
    }

    fn raise_if_enabled(&mut self) {
        if self.isr & self.stored.read(IER) != 0 {
            self.irqs.raise(LTDC_IRQ);
        }
    }

    /// The layer registers take effect: the display uses them from now on. The reload is
    /// done, which sets RRIF.
    fn reload(&mut self) {
        for offset in (LAYER_CR..LAYER_CR + LAYER_STRIDE * LAYERS as u32).step_by(4) {
            if shadowed(offset) {
                let value = self.stored.read(offset);
                self.active.write(offset, value, !0);
            }
        }
        let layers = std::array::from_fn(|layer| {
            let at = |register| LAYER_CR + LAYER_STRIDE * layer as u32 + register;
            Layer {
                cr: self.active.read(at(0)),
                whpcr: self.active.read(at(WHPCR)),
                wvpcr: self.active.read(at(WVPCR)),
                pfcr: self.active.read(at(PFCR)),
                cacr: self.active.read(at(CACR)),
                dccr: self.active.read(at(DCCR)),
                bfcr: self.active.read(at(BFCR)),
                cfbar: self.active.read(at(CFBAR)),
                cfblr: self.active.read(at(CFBLR)),
                cfblnr: self.active.read(at(CFBLNR)),
            }
        });
        self.display.update(|s| s.layers = layers);
        self.isr |= RRIF;
        self.raise_if_enabled();
    }

    fn write_gcr(&mut self, value: u32, mask: u32) {
        let was_enabled = self.stored.read(GCR) & LTDCEN != 0;
        self.stored.write(GCR, value, mask);
        let enabled = self.stored.read(GCR) & LTDCEN != 0;
        self.display.update(|s| s.enabled = enabled);
        if enabled != was_enabled {
            self.restart(self.clock.now());
        }
    }
}

impl Registers for LtdcRegisters {
    fn read(&mut self, offset: u32) -> u32 {
        self.catch_up();
        match offset {
            ISR => self.isr,
            SRCR if self.reload_pending => VBR,
            SRCR => 0,
            _ if shadowed(offset) => self.active.read(offset),
            _ => self.stored.read(offset),
        }
    }

    fn write(&mut self, offset: u32, value: u32, mask: u32) {
        self.catch_up();
        match offset {
            GCR => self.write_gcr(value, mask),
            TWCR | LIPCR => {
                self.stored.write(offset, value, mask);
                self.arm();
            }
            IER => {
                self.stored.write(offset, value, mask & FLAGS);
                self.schedule();
                self.raise_if_enabled();
            }
            BPCR | AWCR | BCCR => {
                self.stored.write(offset, value, mask);
                let (bpcr, awcr, bccr) = (
                    self.stored.read(BPCR),
                    self.stored.read(AWCR),
                    self.stored.read(BCCR),
                );
                self.display.update(|s| {
                    s.bpcr = bpcr;
                    s.awcr = awcr;
                    s.bccr = bccr;
                });
                // The blank is the line after the active area.
                self.arm();
            }
            ICR => self.isr &= !(value & mask & FLAGS),
            SRCR => {
                let requested = value & mask;
                if requested & IMR != 0 {
                    self.reload();
                }
                if requested & VBR != 0 {
                    self.reload_pending = true;
                    self.arm();
                }
            }
            ISR => {}
            _ => {
                self.stored.write(offset, value, mask);
                if let Some((layer, CLUTWR)) = layer_register(offset) {
                    // The colour table takes the entry at once: the index is in the top byte.
                    let entry = self.stored.read(offset);
                    self.display
                        .update(|s| s.clut[layer][(entry >> 24) as usize] = entry & 0xFF_FFFF);
                }
            }
        }
    }

    fn reset(&mut self, _cause: ResetCause) {
        (self.stored, self.active) = reset_values();
        self.display.update(|s| *s = Scanout::default());
        self.isr = 0;
        self.frame_start = None;
        self.anchor_pixel = 0;
        self.next_line = None;
        self.flagged_until = 0;
        self.reload_pending = false;
        self.next_blank = None;
    }

    fn tick(&mut self) {
        self.catch_up();
        self.schedule();
    }
}

pub type Ltdc = WordAccess<LtdcRegisters>;

impl Ltdc {
    pub(crate) fn new(clock: Clock, display: Display, irqs: Irqs, pixel_clock: PixelClock) -> Ltdc {
        let pixel_hz = pixel_clock.hz();
        let (stored, active) = reset_values();
        WordAccess::wrap(LtdcRegisters {
            stored,
            active,
            clock,
            irqs,
            display,
            pixel_clock,
            isr: 0,
            frame_start: None,
            anchor_pixel: 0,
            pixel_hz,
            next_line: None,
            flagged_until: 0,
            reload_pending: false,
            next_blank: None,
        })
    }
}

#[cfg(test)]
mod tests;
