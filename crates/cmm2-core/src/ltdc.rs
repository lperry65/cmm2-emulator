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
//! A write of IMR to SRCR reloads the shadow registers at once: the bit reads back 0 and
//! ISR.RRIF is set, and the layer registers the display uses are those as of that write. A write
//! of VBR (bit 1) holds the reload back to the next vertical blank, the first line after the
//! active area (AWCR), as the chip does: the bit reads 1 until then, then the display takes the
//! registers as they are at that moment and RRIF is set. A frame that is not running (the
//! controller off) has no blank, so the reload waits for the first one after it is enabled.
//! The firmware uses VBR only once in each MODE command, to complete the layer registers it
//! has cleared; its page flips (PAGE DISPLAY) write the layer's registers from the line
//! interrupt and reload them with IMR. The registers read back as written, whether or not they
//! have been reloaded. A write to CLUTWR goes into the layer's colour table at once. The other
//! flags (FU, TERR) are never set, as nothing is fetched, so the error interrupt (IRQ 89) is
//! never raised. CDSR and CPSR, the position the controller is at, are plain stored registers:
//! the firmware does not read them.
//!
//! The interrupt line is a level on the real chip, but `Irqs` is edge like, so the line is raised
//! when a flag is set that is enabled, and when an enable is written for a flag that is set.

use crate::clock::{CORE_HZ, Clock};
use crate::display::{Display, Layer, Scanout};
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
/// those it draws from. `pixel_hz` is the pixel clock the controller last saw. While it is
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
    /// counters wrapping, not checked against the chip).
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
        let layers = std::array::from_fn(|layer| {
            let at = |register| LAYER_CR + LAYER_STRIDE * layer as u32 + register;
            Layer {
                cr: self.stored.read(at(0)),
                whpcr: self.stored.read(at(WHPCR)),
                wvpcr: self.stored.read(at(WVPCR)),
                pfcr: self.stored.read(at(PFCR)),
                cacr: self.stored.read(at(CACR)),
                dccr: self.stored.read(at(DCCR)),
                bfcr: self.stored.read(at(BFCR)),
                cfbar: self.stored.read(at(CFBAR)),
                cfblr: self.stored.read(at(CFBLR)),
                cfblnr: self.stored.read(at(CFBLNR)),
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

    fn reset(&mut self, cause: ResetCause) {
        self.stored.reset(cause);
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
        WordAccess::wrap(LtdcRegisters {
            stored: StubRegisters::with_reset_values(LTDC_BASE, Svd::stm32h743()),
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
mod tests {
    use super::*;
    use crate::rcc::{RCC_BASE, RCC_SIZE, Rcc};
    use crate::{FrameError, MemoryMap, ResetCause};

    const GCR_ADDR: u32 = LTDC_BASE + GCR;
    const TWCR_ADDR: u32 = LTDC_BASE + TWCR;
    const SRCR_ADDR: u32 = LTDC_BASE + SRCR;
    const IER_ADDR: u32 = LTDC_BASE + IER;
    const ISR_ADDR: u32 = LTDC_BASE + ISR;
    const ICR_ADDR: u32 = LTDC_BASE + ICR;
    const LIPCR_ADDR: u32 = LTDC_BASE + LIPCR;

    // The board's 1056 x 625 total at 49.5 MHz: a line is 10,240 core cycles and a frame
    // 6,400,000, which is 75 Hz.
    const BOARD_TWCR: u32 = 0x041F_0270;
    const LINE: u64 = 10_240;
    const FRAME: u64 = 6_400_000;
    // What the firmware programs: the line after the active area.
    const LAST_LINE: u32 = 0x270;
    const LTDC_BIT: u32 = 1 << (LTDC_IRQ - 64);

    // The board's pixel clock for 800 x 600, and for 640 x 480 with its total of 840 x 500: a
    // line is 12,848 core cycles and a frame 6,423,530, which is 74.725 Hz. The game measured
    // 748 ticks in 10 s on the board, 74.8 +- 0.1 Hz, which agrees with that without confirming
    // the decimals. The line interrupt is line 499 there.
    const PIXEL_HZ: u64 = 49_500_000;
    const PIXEL_HZ_640: u64 = 31_384_615;
    const TWCR_640: u32 = 0x0347_01F3;
    const LINE_499_640: u64 = 6_410_683;
    const LINE_499_NEXT_FRAME_640: u64 = 12_834_212;

    struct Machine {
        memory: MemoryMap,
        clock: Clock,
        irqs: Irqs,
        pixel_clock: PixelClock,
    }

    impl Machine {
        fn new() -> Machine {
            Machine::with_pixel_clock(PIXEL_HZ)
        }

        fn with_pixel_clock(hz: u64) -> Machine {
            Machine::around(|_, clock| PixelClock::running_at(clock.clone(), hz))
        }

        // The pixel clock is PLL3 of an RCC that is mapped too, which the firmware programs.
        fn with_rcc() -> Machine {
            Machine::around(|memory, clock| {
                let rcc = Rcc::new(clock.clone());
                let pixel_clock = rcc.pixel_clock();
                memory
                    .map_peripheral(RCC_BASE, RCC_SIZE, Box::new(rcc))
                    .unwrap();
                pixel_clock
            })
        }

        fn around(pixel_clock: impl FnOnce(&mut MemoryMap, &Clock) -> PixelClock) -> Machine {
            let mut memory = MemoryMap::new();
            let clock = memory.clock();
            let irqs = Irqs::new();
            let pixel_clock = pixel_clock(&mut memory, &clock);
            memory
                .map_peripheral(
                    LTDC_BASE,
                    LTDC_SIZE,
                    Box::new(Ltdc::new(
                        clock.clone(),
                        memory.display(),
                        irqs.clone(),
                        pixel_clock.clone(),
                    )),
                )
                .unwrap();
            Machine {
                memory,
                clock,
                irqs,
                pixel_clock,
            }
        }

        // The board's timing, the line interrupt programmed at the last line, the controller
        // enabled: the order the firmware does it in.
        fn running() -> Machine {
            let mut m = Machine::new();
            m.memory.write32(TWCR_ADDR, BOARD_TWCR);
            m.memory.write32(LIPCR_ADDR, LAST_LINE);
            m.memory.write32(GCR_ADDR, LTDCEN);
            m
        }

        fn at(&mut self, cycle: u64) {
            self.clock.set(cycle);
        }

        // What the CPU does before an instruction.
        fn tick_if_due(&mut self) {
            if self.clock.take_due() {
                self.memory.tick();
            }
        }

        fn isr(&mut self) -> u32 {
            self.memory.read32(ISR_ADDR)
        }

        fn raised(&self) -> bool {
            match self.irqs.take() {
                None => false,
                Some(bits) => {
                    assert_eq!(bits, [0, 0, LTDC_BIT, 0, 0, 0, 0, 0]);
                    true
                }
            }
        }
    }

    #[test]
    fn registers_read_their_reset_values() {
        let mut m = Machine::new();
        assert_eq!(m.memory.read32(GCR_ADDR), 0x2220);
        assert_eq!(m.memory.read32(IER_ADDR), 0);
        assert_eq!(m.isr(), 0);
        assert_eq!(m.memory.read32(LIPCR_ADDR), 0);
    }

    #[test]
    fn the_other_registers_store_what_is_written() {
        let mut m = Machine::new();
        m.memory.write32(LTDC_BASE + 0x08, 0x004F_0002); // SSCR
        m.memory.write32(LTDC_BASE + 0x84, 0x11); // L1CR
        assert_eq!(m.memory.read32(LTDC_BASE + 0x08), 0x004F_0002);
        assert_eq!(m.memory.read32(LTDC_BASE + 0x84), 0x11);
    }

    #[test]
    fn ier_keeps_only_its_four_enables() {
        let mut m = Machine::new();
        m.memory.write32(IER_ADDR, 0xFFFF_FFFF);
        assert_eq!(m.memory.read32(IER_ADDR), 0xF);
    }

    #[test]
    fn the_line_is_flagged_when_the_frame_reaches_it() {
        let mut m = Machine::running();
        m.at(624 * LINE - 1);
        assert_eq!(m.isr(), 0);
        m.at(624 * LINE);
        assert_eq!(m.isr(), LIF);
    }

    #[test]
    fn the_line_comes_round_every_frame() {
        let mut m = Machine::running();
        m.at(624 * LINE);
        m.memory.write32(ICR_ADDR, LIF);
        assert_eq!(m.isr(), 0);
        m.at(FRAME + 624 * LINE - 1);
        assert_eq!(m.isr(), 0);
        m.at(FRAME + 624 * LINE);
        assert_eq!(m.isr(), LIF);
    }

    #[test]
    fn the_frame_starts_when_the_controller_is_enabled() {
        let mut m = Machine::new();
        m.memory.write32(TWCR_ADDR, BOARD_TWCR);
        m.memory.write32(LIPCR_ADDR, 10);
        m.at(1_000_000);
        m.memory.write32(GCR_ADDR, LTDCEN);
        m.at(1_000_000 + 10 * LINE - 1);
        assert_eq!(m.isr(), 0);
        m.at(1_000_000 + 10 * LINE);
        assert_eq!(m.isr(), LIF);
    }

    #[test]
    fn nothing_is_flagged_while_the_controller_is_disabled() {
        let mut m = Machine::new();
        m.memory.write32(TWCR_ADDR, BOARD_TWCR);
        m.memory.write32(LIPCR_ADDR, LAST_LINE);
        m.memory.write32(IER_ADDR, LIF);
        m.at(10 * FRAME);
        assert_eq!(m.isr(), 0);
        m.tick_if_due();
        assert!(!m.raised());
    }

    #[test]
    fn disabling_the_controller_stops_the_line() {
        let mut m = Machine::running();
        m.memory.write32(GCR_ADDR, 0);
        m.at(10 * FRAME);
        assert_eq!(m.isr(), 0);
    }

    #[test]
    fn enabling_again_starts_a_new_frame() {
        let mut m = Machine::running();
        m.at(FRAME);
        m.memory.write32(ICR_ADDR, LIF);
        m.memory.write32(GCR_ADDR, 0);
        m.memory.write32(GCR_ADDR, LTDCEN);
        m.at(FRAME + 624 * LINE - 1);
        assert_eq!(m.isr(), 0);
        m.at(FRAME + 624 * LINE);
        assert_eq!(m.isr(), LIF);
    }

    #[test]
    fn the_frame_follows_the_total_size() {
        // 256 x 128: a line is 256 pixels, which is 2482.4 cycles, so line 1 is reached on
        // the 2483rd.
        let mut m = Machine::new();
        m.memory.write32(TWCR_ADDR, 0x00FF_007F);
        m.memory.write32(LIPCR_ADDR, 1);
        m.memory.write32(GCR_ADDR, LTDCEN);
        m.at(2482);
        assert_eq!(m.isr(), 0);
        m.at(2483);
        assert_eq!(m.isr(), LIF);
        m.memory.write32(ICR_ADDR, LIF);
        // A frame of 256 x 128 pixels is 317,750.3 cycles, so line 1 of the next is reached
        // at 320,232.7.
        m.at(320_232);
        assert_eq!(m.isr(), 0);
        m.at(320_233);
        assert_eq!(m.isr(), LIF);
    }

    #[test]
    fn a_line_beyond_the_last_is_never_reached() {
        let mut m = Machine::new();
        m.memory.write32(TWCR_ADDR, BOARD_TWCR);
        m.memory.write32(LIPCR_ADDR, 625);
        m.memory.write32(GCR_ADDR, LTDCEN);
        m.at(10 * FRAME);
        assert_eq!(m.isr(), 0);
    }

    #[test]
    fn moving_the_line_aims_at_the_new_one() {
        let mut m = Machine::running();
        m.at(100 * LINE);
        m.memory.write32(LIPCR_ADDR, 200);
        m.at(200 * LINE - 1);
        assert_eq!(m.isr(), 0);
        m.at(200 * LINE);
        assert_eq!(m.isr(), LIF);
    }

    #[test]
    fn moving_the_line_to_one_already_past_waits_for_the_next_frame() {
        let mut m = Machine::running();
        m.at(100 * LINE);
        m.memory.write32(LIPCR_ADDR, 50);
        m.at(FRAME + 50 * LINE - 1);
        assert_eq!(m.isr(), 0);
        m.at(FRAME + 50 * LINE);
        assert_eq!(m.isr(), LIF);
    }

    #[test]
    fn the_flag_is_set_once_however_many_frames_pass() {
        let mut m = Machine::running();
        m.at(100 * FRAME + 624 * LINE + 5);
        assert_eq!(m.isr(), LIF);
        m.memory.write32(ICR_ADDR, LIF);
        assert_eq!(m.isr(), 0);
        m.at(101 * FRAME + 624 * LINE);
        assert_eq!(m.isr(), LIF);
    }

    #[test]
    fn icr_clears_only_the_flags_written() {
        let mut m = Machine::running();
        m.memory.write32(SRCR_ADDR, IMR);
        m.at(624 * LINE);
        assert_eq!(m.isr(), LIF | RRIF);
        m.memory.write32(ICR_ADDR, RRIF);
        assert_eq!(m.isr(), LIF);
        m.memory.write32(ICR_ADDR, 0);
        assert_eq!(m.isr(), LIF);
        m.memory.write32(ICR_ADDR, 0xFFFF_FFFF);
        assert_eq!(m.isr(), 0);
    }

    #[test]
    fn icr_reads_zero_and_isr_cannot_be_written() {
        let mut m = Machine::running();
        m.at(624 * LINE);
        m.memory.write32(ICR_ADDR, 0xFF);
        assert_eq!(m.memory.read32(ICR_ADDR), 0);
        m.memory.write32(ISR_ADDR, 0xF);
        assert_eq!(m.isr(), 0);
    }

    #[test]
    fn an_immediate_reload_completes_at_once_and_flags_it() {
        let mut m = Machine::new();
        m.memory.write32(SRCR_ADDR, IMR);
        assert_eq!(m.memory.read32(SRCR_ADDR), 0);
        assert_eq!(m.isr(), RRIF);
    }

    #[test]
    fn a_write_of_neither_reload_bit_does_not_flag() {
        let mut m = Machine::new();
        m.memory.write32(SRCR_ADDR, 0xFFFF_FFFC);
        assert_eq!(m.isr(), 0);
    }

    #[test]
    fn the_line_interrupt_is_raised_when_it_is_enabled_and_the_line_comes() {
        let mut m = Machine::running();
        m.memory.write32(IER_ADDR, LIF);
        m.at(624 * LINE - 1);
        m.tick_if_due();
        assert!(!m.raised());
        m.at(624 * LINE);
        m.tick_if_due();
        assert!(m.raised());
        assert_eq!(m.isr(), LIF);
    }

    #[test]
    fn the_interrupt_is_raised_without_the_firmware_touching_the_controller() {
        let mut m = Machine::running();
        m.memory.write32(IER_ADDR, LIF);
        // Every frame, with nothing but the clock moving.
        for frame in 0..3 {
            m.at(frame * FRAME + 624 * LINE);
            m.tick_if_due();
            assert!(m.raised(), "frame {frame}");
        }
    }

    #[test]
    fn nothing_is_raised_while_the_line_enable_is_off() {
        let mut m = Machine::running();
        m.memory.write32(IER_ADDR, 0b0110);
        m.at(5 * FRAME);
        m.tick_if_due();
        assert!(!m.raised());
        // The flag still shows.
        assert_eq!(m.isr(), LIF);
    }

    #[test]
    fn the_handlers_sequence_re_arms_the_interrupt_for_the_next_frame() {
        let mut m = Machine::running();
        m.memory.write32(IER_ADDR, 0b0111);
        m.at(624 * LINE);
        m.tick_if_due();
        assert!(m.raised());
        // What the HAL does: line enable off, flag cleared, then the callback programs the
        // same line again and turns the enable back on.
        m.at(624 * LINE + 100);
        m.memory.write32(IER_ADDR, 0b0110);
        m.memory.write32(ICR_ADDR, LIF);
        m.memory.write32(IER_ADDR, 0b0110);
        m.memory.write32(LIPCR_ADDR, LAST_LINE);
        m.memory.write32(IER_ADDR, 0b0111);
        assert!(!m.raised());
        m.at(FRAME + 624 * LINE - 1);
        m.tick_if_due();
        assert!(!m.raised());
        m.at(FRAME + 624 * LINE);
        m.tick_if_due();
        assert!(m.raised());
    }

    #[test]
    fn enabling_a_flag_that_is_already_set_raises_the_interrupt() {
        let mut m = Machine::running();
        m.at(624 * LINE);
        assert_eq!(m.isr(), LIF);
        assert!(!m.raised());
        m.memory.write32(IER_ADDR, LIF);
        assert!(m.raised());
    }

    #[test]
    fn a_reload_raises_the_interrupt_only_when_it_is_enabled() {
        let mut m = Machine::new();
        m.memory.write32(SRCR_ADDR, 1);
        assert!(!m.raised());
        m.memory.write32(ICR_ADDR, RRIF);
        m.memory.write32(IER_ADDR, RRIF);
        m.memory.write32(SRCR_ADDR, 1);
        assert!(m.raised());
    }

    #[test]
    fn nothing_asks_for_a_tick_unless_the_line_interrupt_is_on() {
        let mut m = Machine::running();
        m.at(10 * FRAME);
        assert!(!m.clock.take_due());
    }

    #[test]
    fn the_wake_is_on_the_first_cycle_the_line_is_reached() {
        // Line 1 of 256 pixels is reached on cycle 2483, not on 2482 where it is 2482.4.
        let mut m = Machine::new();
        m.memory.write32(TWCR_ADDR, 0x00FF_007F);
        m.memory.write32(LIPCR_ADDR, 1);
        m.memory.write32(GCR_ADDR, LTDCEN);
        m.memory.write32(IER_ADDR, LIF);
        m.at(2482);
        assert!(!m.clock.take_due());
        m.at(2483);
        assert!(m.clock.take_due());
    }

    #[test]
    fn a_tick_for_another_device_keeps_the_wake() {
        let mut m = Machine::running();
        m.memory.write32(IER_ADDR, LIF);
        m.memory.tick();
        m.at(624 * LINE);
        assert!(m.clock.take_due());
    }

    #[test]
    fn a_system_reset_stops_the_controller_and_clears_its_registers() {
        for cause in [ResetCause::Software, ResetCause::PowerOn] {
            let mut m = Machine::running();
            m.memory.write32(IER_ADDR, LIF);
            m.memory.write32(SRCR_ADDR, 1);
            m.memory.reset_peripherals(cause);
            assert_eq!(m.memory.read32(IER_ADDR), 0);
            assert_eq!(m.memory.read32(GCR_ADDR), 0x2220);
            assert_eq!(m.isr(), 0);
            m.at(10 * FRAME);
            assert_eq!(m.isr(), 0);
        }
    }

    #[test]
    fn the_window_ends_at_ltdc_size() {
        let mut m = Machine::new();
        m.memory.write32(LTDC_BASE + LTDC_SIZE - 4, 5);
        assert_eq!(m.memory.read32(LTDC_BASE + LTDC_SIZE - 4), 5);
        assert!(m.memory.take_unmapped().is_empty());
    }

    #[test]
    fn narrow_writes_reach_the_registers() {
        let mut m = Machine::running();
        m.memory.write8(IER_ADDR, 1);
        m.at(624 * LINE);
        m.tick_if_due();
        assert!(m.raised());
        m.memory.write8(ICR_ADDR, 1);
        assert_eq!(m.isr(), 0);
    }

    // What the firmware writes to bring up layer 1: L8, 800 x 600 at the start of AXI SRAM,
    // and the colour table. The controller is enabled first, as the firmware does.
    fn configure_layer(m: &mut Machine) {
        m.memory.write32(LTDC_BASE + BPCR, 0x00EF_0017);
        m.memory.write32(LTDC_BASE + AWCR, 0x040F_026F);
        m.memory.write32(TWCR_ADDR, BOARD_TWCR);
        m.memory.write32(GCR_ADDR, LTDCEN);
        m.memory.write32(LTDC_BASE + LAYER_CR + WHPCR, 0x040F_00F0);
        m.memory.write32(LTDC_BASE + LAYER_CR + WVPCR, 0x026F_0018);
        m.memory.write32(LTDC_BASE + LAYER_CR + PFCR, 5);
        m.memory.write32(LTDC_BASE + LAYER_CR + CACR, 0xFF);
        m.memory.write32(LTDC_BASE + LAYER_CR + CFBAR, 0x2400_0000);
        m.memory.write32(LTDC_BASE + LAYER_CR + CFBLR, 0x0320_0327);
        m.memory.write32(LTDC_BASE + LAYER_CR + CFBLNR, 600);
        m.memory.write32(LTDC_BASE + LAYER_CR + CLUTWR, 0x0000_0000);
        m.memory.write32(LTDC_BASE + LAYER_CR + CLUTWR, 0x0100_00FF);
        m.memory.write32(LTDC_BASE + LAYER_CR + CLUTWR, 0x0200_FF00);
    }

    #[test]
    fn the_frame_is_what_the_layer_registers_describe_once_they_are_reloaded() {
        let mut m = Machine::new();
        configure_layer(&mut m);
        m.memory.write8(0x2400_0000, 1);
        m.memory.write8(0x2400_0001, 2);
        m.memory.write32(LTDC_BASE + LAYER_CR, 0x11);
        m.memory.write32(SRCR_ADDR, 1);
        let frame = m.memory.frame().unwrap();
        assert_eq!((frame.width, frame.height), (800, 600));
        assert_eq!(frame.pixel(0, 0), [0, 0, 0xFF]);
        assert_eq!(frame.pixel(1, 0), [0, 0xFF, 0]);
        assert_eq!(frame.pixel(2, 0), [0, 0, 0]);
    }

    #[test]
    fn the_layer_registers_only_count_from_the_reload() {
        let mut m = Machine::new();
        configure_layer(&mut m);
        m.memory.write8(0x2400_0000, 1);
        m.memory.write32(LTDC_BASE + LAYER_CR, 0x11);
        m.memory.write32(SRCR_ADDR, 1);
        // Moved to the second pixel and disabled, but not reloaded: the frame is as before.
        m.memory.write32(LTDC_BASE + LAYER_CR + CFBAR, 0x2400_0001);
        m.memory.write32(LTDC_BASE + LAYER_CR, 0);
        assert_eq!(m.memory.frame().unwrap().pixel(0, 0), [0, 0, 0xFF]);
        m.memory.write32(SRCR_ADDR, IMR);
        assert_eq!(m.memory.frame().unwrap().pixel(0, 0), [0, 0, 0]);
    }

    #[test]
    fn the_colour_table_counts_at_once() {
        let mut m = Machine::new();
        configure_layer(&mut m);
        m.memory.write8(0x2400_0000, 1);
        m.memory.write32(LTDC_BASE + LAYER_CR, 0x11);
        m.memory.write32(SRCR_ADDR, 1);
        m.memory.write32(LTDC_BASE + LAYER_CR + CLUTWR, 0x0111_2233);
        assert_eq!(m.memory.frame().unwrap().pixel(0, 0), [0x11, 0x22, 0x33]);
    }

    #[test]
    fn the_active_area_and_the_background_count_at_once() {
        let mut m = Machine::new();
        configure_layer(&mut m);
        m.memory
            .write32(LTDC_BASE + AWCR, 0x00EF_0017 + 0x0010_0004);
        m.memory.write32(LTDC_BASE + BCCR, 0x0001_0203);
        let frame = m.memory.frame().unwrap();
        assert_eq!((frame.width, frame.height), (16, 4));
        assert_eq!(frame.pixel(0, 0), [1, 2, 3]);
    }

    #[test]
    fn there_is_no_frame_while_the_controller_is_disabled() {
        let mut m = Machine::new();
        configure_layer(&mut m);
        m.memory.write32(GCR_ADDR, 0);
        assert_eq!(m.memory.frame(), Err(FrameError::Off));
    }

    #[test]
    fn a_system_reset_blanks_the_display() {
        let mut m = Machine::new();
        configure_layer(&mut m);
        m.memory.write32(LTDC_BASE + LAYER_CR, 0x11);
        m.memory.write32(SRCR_ADDR, 1);
        m.memory.reset_peripherals(ResetCause::Software);
        assert_eq!(m.memory.frame(), Err(FrameError::Off));
        // The colour table and the layer are gone too, not only the enable.
        m.memory.write32(GCR_ADDR, LTDCEN);
        m.memory.write32(LTDC_BASE + BPCR, 0x00EF_0017);
        m.memory.write32(LTDC_BASE + AWCR, 0x040F_026F);
        let frame = m.memory.frame().unwrap();
        assert_eq!(frame.pixel(0, 0), [0, 0, 0]);
    }

    #[test]
    fn the_framebuffer_can_be_in_sdram() {
        let mut m = Machine::new();
        configure_layer(&mut m);
        m.memory.write8(0xD000_0000, 2);
        m.memory.write32(LTDC_BASE + LAYER_CR + CFBAR, 0xD000_0000);
        m.memory.write32(LTDC_BASE + LAYER_CR, 0x11);
        m.memory.write32(SRCR_ADDR, 1);
        assert_eq!(m.memory.frame().unwrap().pixel(0, 0), [0, 0xFF, 0]);
    }

    #[test]
    fn a_framebuffer_that_runs_off_the_end_of_ram_is_reported() {
        let mut m = Machine::new();
        configure_layer(&mut m);
        // AXI SRAM is 512 KiB; this leaves room for the first line only.
        let last_line = 0x2400_0000 + 512 * 1024 - 800;
        m.memory.write32(LTDC_BASE + LAYER_CR + CFBAR, last_line);
        m.memory.write32(LTDC_BASE + LAYER_CR, 0x11);
        m.memory.write32(SRCR_ADDR, 1);
        assert_eq!(
            m.memory.frame(),
            Err(FrameError::Unreadable(last_line + 800))
        );
    }

    fn configure_640(m: &mut Machine) {
        m.memory.write32(TWCR_ADDR, TWCR_640);
        m.memory.write32(LIPCR_ADDR, 499);
        m.memory.write32(GCR_ADDR, LTDCEN);
    }

    #[test]
    fn the_frame_runs_at_the_pixel_clock_the_firmware_programs() {
        let mut m = Machine::with_pixel_clock(PIXEL_HZ_640);
        configure_640(&mut m);
        // A line is 12,848 cycles, not the 10,240 of the 800 x 600 mode.
        m.at(LINE_499_640 - 1);
        assert_eq!(m.isr(), 0);
        m.at(LINE_499_640);
        assert_eq!(m.isr(), LIF);
        m.memory.write32(ICR_ADDR, LIF);
        m.at(LINE_499_NEXT_FRAME_640 - 1);
        assert_eq!(m.isr(), 0);
        m.at(LINE_499_NEXT_FRAME_640);
        assert_eq!(m.isr(), LIF);
    }

    // What the firmware writes to RCC for 640 x 480: the crystal as the source with DIVM3 of 1,
    // PLL3DIVR for 8 MHz x 102 / 26, the PLL3 configuration, and PLL3 on.
    fn program_pll3_for_640(m: &mut Machine) {
        m.memory.write32(RCC_BASE + 0x28, 0x0010_1012);
        m.memory.write32(RCC_BASE + 0x40, 0x1919_3265);
        m.memory.write32(RCC_BASE + 0x2C, 0x013F_0CDD);
        m.memory.write32(RCC_BASE, 0x1501_1001);
    }

    // The cycles at which the line interrupt comes, stepping the clock by `stride` cycles the way
    // the CPU's instructions would, until there are `count` of them.
    fn line_interrupts(m: &mut Machine, stride: u64, count: usize) -> Vec<u64> {
        let mut at = Vec::new();
        let mut cycle = m.clock.now();
        while at.len() < count {
            cycle += stride;
            m.at(cycle);
            m.tick_if_due();
            if m.raised() {
                at.push(cycle);
                m.memory.write32(ICR_ADDR, LIF);
            }
        }
        at
    }

    #[test]
    fn the_firmwares_pll3_settings_for_640_x_480_come_round_at_74_7_hz_through_the_model() {
        // The RCC works the pixel clock out, and the line interrupt's period is measured over 20
        // frames. 840 x 500 pixels at 8 MHz x 102 / 26 is 74.725 Hz.
        let mut m = Machine::with_rcc();
        program_pll3_for_640(&mut m);
        configure_640(&mut m);
        m.memory.write32(IER_ADDR, LIF);
        let at = line_interrupts(&mut m, 1000, 21);
        let period = (at[20] - at[0]) as f64 / 20.0;
        let hz = CORE_HZ as f64 / period;
        let expected = (8e6 * 102.0 / 26.0) / (840.0 * 500.0);
        assert!((hz - expected).abs() < 0.001, "{hz} against {expected}");
        assert!((hz - 74.725).abs() < 0.001, "{hz}");
    }

    #[test]
    fn a_controller_enabled_with_no_pixel_clock_starts_its_frame_when_the_clock_starts() {
        let mut m = Machine::with_rcc();
        configure_640(&mut m);
        m.memory.write32(IER_ADDR, LIF);
        m.at(5_000);
        program_pll3_for_640(&mut m);
        // The CPU's next instruction takes the tick that the change of clock asked for.
        m.tick_if_due();
        m.at(5_000 + LINE_499_640 - 1);
        m.tick_if_due();
        assert_eq!(m.isr(), 0);
        m.at(5_000 + LINE_499_640);
        m.tick_if_due();
        assert!(m.raised());
        assert_eq!(m.isr(), LIF);
    }

    #[test]
    fn stopping_the_pixel_clock_under_a_running_controller_stops_the_frame() {
        let mut m = Machine::running();
        m.memory.write32(IER_ADDR, LIF);
        m.at(100 * LINE);
        m.pixel_clock.set(0);
        m.at(10 * FRAME);
        m.tick_if_due();
        assert_eq!(m.isr(), 0);
        assert!(!m.raised());
        assert!(!m.clock.take_due());
    }

    #[test]
    fn changing_the_pixel_clock_under_a_running_controller_changes_the_frame_rate() {
        // The 800 x 600 timing (1056 x 625) at the 640 x 480 clock. The counters are at line 100
        // when the clock changes and go on from there: line 624 is 524 lines of 1056 pixels on,
        // at 480 MHz / 31,384,615 Hz cycles each.
        let mut m = Machine::running();
        m.memory.write32(IER_ADDR, LIF);
        m.at(100 * LINE);
        m.pixel_clock.set(PIXEL_HZ_640);
        m.tick_if_due();
        let line_624 = 100 * LINE + (524u64 * 1056 * 480_000_000).div_ceil(31_384_615);
        m.at(line_624 - 1);
        m.tick_if_due();
        assert_eq!(m.isr(), 0);
        m.at(line_624);
        m.tick_if_due();
        assert!(m.raised());
        assert_eq!(m.isr(), LIF);
    }

    #[test]
    fn the_new_frame_starts_at_the_change_of_the_clock_not_when_the_controller_notices() {
        // Nothing touches the controller for 5000 cycles after the change.
        let mut m = Machine::running();
        m.at(100 * LINE);
        m.pixel_clock.set(PIXEL_HZ_640);
        m.at(100 * LINE + 5_000);
        let line_624 = 100 * LINE + (524u64 * 1056 * 480_000_000).div_ceil(31_384_615);
        m.at(line_624 - 1);
        assert_eq!(m.isr(), 0);
        m.at(line_624);
        assert_eq!(m.isr(), LIF);
    }

    #[test]
    fn a_pixel_clock_that_stops_freezes_the_counters_and_when_it_starts_they_go_on() {
        let mut m = Machine::running();
        m.memory.write32(IER_ADDR, LIF);
        m.at(100 * LINE);
        m.pixel_clock.set(0);
        m.tick_if_due();
        m.at(10 * FRAME);
        m.pixel_clock.set(PIXEL_HZ);
        m.tick_if_due();
        // 524 lines more at 49.5 MHz from the line it stopped at.
        let line_624 = 10 * FRAME + 524 * LINE;
        m.at(line_624 - 1);
        m.tick_if_due();
        assert_eq!(m.isr(), 0);
        m.at(line_624);
        m.tick_if_due();
        assert_eq!(m.isr(), LIF);
    }

    #[test]
    fn a_stop_and_restart_of_the_clock_at_the_same_rate_is_seen_without_a_tick() {
        // Nothing steps the machine between the two writes, as with a debugger's memory write.
        let mut m = Machine::running();
        m.at(100 * LINE);
        m.pixel_clock.set(0);
        m.at(10 * FRAME);
        m.pixel_clock.set(PIXEL_HZ);
        let line_624 = 10 * FRAME + 524 * LINE;
        m.at(line_624 - 1);
        assert_eq!(m.isr(), 0);
        m.at(line_624);
        assert_eq!(m.isr(), LIF);
    }

    #[test]
    fn a_reload_waits_for_the_next_frame_when_the_active_area_ends_beyond_the_last_line() {
        // AAH of 700 on a frame of 625 lines: there is no line after the active area, and I take
        // the blank to be the first line of the next frame.
        let mut m = layer_with_a_held_back_reload();
        m.at(LINE);
        m.memory.write32(LTDC_BASE + AWCR, 0x040F_02BC);
        m.at(FRAME - 1);
        assert_eq!(m.memory.read32(SRCR_ADDR), VBR);
        m.at(FRAME);
        assert_eq!(m.memory.read32(SRCR_ADDR), 0);
    }

    #[test]
    fn a_reload_waits_for_the_next_frame_when_the_active_area_runs_to_the_end_of_the_frame() {
        // The active area ends on the last line (AAH + 1 is the total height), so the blank is the
        // first line of the next frame.
        let mut m = layer_with_a_held_back_reload();
        m.at(LINE);
        m.memory.write32(LTDC_BASE + AWCR, 0x040F_0270);
        assert_eq!(m.memory.read32(SRCR_ADDR), VBR);
        m.at(FRAME - 1);
        assert_eq!(m.memory.read32(SRCR_ADDR), VBR);
        m.at(FRAME);
        assert_eq!(m.memory.read32(SRCR_ADDR), 0);
        assert_eq!(m.isr() & RRIF, RRIF);
    }

    #[test]
    fn the_line_that_came_before_the_pixel_clock_changed_is_flagged() {
        // The controller is only told of the change on its next tick; the line event that the
        // old rate reached before then is not lost.
        let mut m = Machine::running();
        m.at(624 * LINE + 10);
        m.pixel_clock.set(PIXEL_HZ_640);
        m.at(624 * LINE + 11);
        assert_eq!(m.isr(), LIF);
    }

    #[test]
    fn a_line_flagged_in_this_pixel_is_not_flagged_again_by_rewriting_the_registers() {
        // The line comes, the handler clears the flag and programs the same line again, all in
        // the pixel the line started in. The next time is next frame.
        for rewrite in [
            (LIPCR_ADDR, 624),
            (TWCR_ADDR, BOARD_TWCR),
            (LTDC_BASE + BPCR, 0),
            (LTDC_BASE + AWCR, 0),
            (LTDC_BASE + BCCR, 0),
            (SRCR_ADDR, VBR),
        ] {
            let mut m = Machine::running();
            m.at(624 * LINE);
            assert_eq!(m.isr(), LIF);
            m.memory.write32(ICR_ADDR, LIF);
            m.memory.write32(rewrite.0, rewrite.1);
            assert_eq!(m.isr() & LIF, 0, "{rewrite:x?}");
            m.at(FRAME + 624 * LINE);
            assert_eq!(m.isr() & LIF, LIF, "{rewrite:x?}");
        }
    }

    #[test]
    fn a_frame_of_a_few_pixels_does_not_wake_the_cpu_for_every_instruction() {
        // 1 x 1 at 148.8 MHz is a frame every 3 cycles, and a line interrupt in each. The wakes
        // come no closer than the least gap, however often the CPU looks.
        let mut m = Machine::with_pixel_clock(148_800_000);
        m.memory.write32(TWCR_ADDR, 0);
        m.memory.write32(LIPCR_ADDR, 0);
        m.memory.write32(IER_ADDR, LIF);
        m.memory.write32(GCR_ADDR, LTDCEN);
        let mut wakes = 0;
        for cycle in (0..100_000).step_by(2) {
            m.at(cycle);
            if m.clock.take_due() {
                wakes += 1;
                m.memory.tick();
            }
        }
        assert!(wakes <= 100_000 / MIN_WAKE_GAP + 1, "{wakes} wakes");
        assert!(wakes > 1, "{wakes} wakes");
    }

    #[test]
    fn without_a_pixel_clock_the_frame_does_not_start() {
        let mut m = Machine::with_pixel_clock(0);
        m.memory.write32(TWCR_ADDR, BOARD_TWCR);
        m.memory.write32(LIPCR_ADDR, LAST_LINE);
        m.memory.write32(IER_ADDR, LIF);
        m.memory.write32(GCR_ADDR, LTDCEN);
        m.at(10 * FRAME);
        assert_eq!(m.isr(), 0);
        m.tick_if_due();
        assert!(!m.raised());
    }

    // Layer 1 as the firmware's 8-bit depth has it, with a blue pixel, and the reload that
    // takes it held back with VBR. The frame starts at cycle 0 and its blank at line 624.
    fn layer_with_a_held_back_reload() -> Machine {
        let mut m = Machine::new();
        configure_layer(&mut m);
        m.memory.write8(0x2400_0000, 1);
        m.memory.write32(LTDC_BASE + LAYER_CR, 0x11);
        m.memory.write32(SRCR_ADDR, VBR);
        m
    }

    #[test]
    fn a_vertical_blanking_reload_waits_for_the_blank() {
        let mut m = layer_with_a_held_back_reload();
        assert_eq!(m.memory.read32(SRCR_ADDR), VBR);
        m.at(624 * LINE - 1);
        assert_eq!(m.memory.frame().unwrap().pixel(0, 0), [0, 0, 0]);
        assert_eq!(m.memory.read32(SRCR_ADDR), VBR);
        assert_eq!(m.isr() & RRIF, 0);
        m.at(624 * LINE);
        assert_eq!(m.memory.read32(SRCR_ADDR), 0);
        assert_eq!(m.isr() & RRIF, RRIF);
        assert_eq!(m.memory.frame().unwrap().pixel(0, 0), [0, 0, 0xFF]);
    }

    #[test]
    fn the_display_has_the_reload_without_the_firmware_touching_the_controller() {
        let mut m = layer_with_a_held_back_reload();
        m.at(624 * LINE);
        m.tick_if_due();
        assert_eq!(m.memory.frame().unwrap().pixel(0, 0), [0, 0, 0xFF]);
    }

    #[test]
    fn the_reload_takes_the_registers_as_they_are_at_the_blank() {
        let mut m = layer_with_a_held_back_reload();
        // The framebuffer moves on a pixel before the blank comes.
        m.memory.write8(0x2400_0001, 2);
        m.memory.write32(LTDC_BASE + LAYER_CR + CFBAR, 0x2400_0001);
        m.at(624 * LINE);
        m.tick_if_due();
        assert_eq!(m.memory.frame().unwrap().pixel(0, 0), [0, 0xFF, 0]);
    }

    #[test]
    fn a_reload_asked_for_in_the_blank_waits_for_the_next_frame() {
        let mut m = Machine::new();
        configure_layer(&mut m);
        // A pixel or more into the blank, so the blank has begun.
        m.at(624 * LINE + 100);
        m.memory.write32(SRCR_ADDR, VBR);
        assert_eq!(m.memory.read32(SRCR_ADDR), VBR);
        m.at(FRAME + 624 * LINE - 1);
        assert_eq!(m.memory.read32(SRCR_ADDR), VBR);
        m.at(FRAME + 624 * LINE);
        assert_eq!(m.memory.read32(SRCR_ADDR), 0);
    }

    #[test]
    fn the_blank_is_the_line_after_the_active_area() {
        let mut m = layer_with_a_held_back_reload();
        // The active area ends at line 99, so the blank is line 100.
        m.memory.write32(LTDC_BASE + AWCR, 0x040F_0063);
        m.at(100 * LINE - 1);
        assert_eq!(m.memory.read32(SRCR_ADDR), VBR);
        m.at(100 * LINE);
        assert_eq!(m.memory.read32(SRCR_ADDR), 0);
    }

    #[test]
    fn a_reload_asked_for_with_the_controller_off_waits_for_the_first_blank_after_it_is_on() {
        let mut m = Machine::new();
        m.memory.write32(TWCR_ADDR, BOARD_TWCR);
        m.memory.write32(LTDC_BASE + AWCR, 0x040F_026F);
        m.memory.write32(SRCR_ADDR, VBR);
        m.at(10 * FRAME);
        assert_eq!(m.memory.read32(SRCR_ADDR), VBR);
        assert_eq!(m.isr(), 0);
        m.memory.write32(GCR_ADDR, LTDCEN);
        m.at(10 * FRAME + 624 * LINE - 1);
        assert_eq!(m.memory.read32(SRCR_ADDR), VBR);
        m.at(10 * FRAME + 624 * LINE);
        assert_eq!(m.memory.read32(SRCR_ADDR), 0);
        assert_eq!(m.isr() & RRIF, RRIF);
    }

    #[test]
    fn a_reload_that_is_held_back_raises_the_interrupt_when_it_completes() {
        let mut m = layer_with_a_held_back_reload();
        m.memory.write32(IER_ADDR, RRIF);
        // Enabling the flag raises nothing yet: it is not set.
        assert!(!m.raised());
        m.at(624 * LINE - 1);
        m.tick_if_due();
        assert!(!m.raised());
        m.at(624 * LINE);
        m.tick_if_due();
        assert!(m.raised());
    }

    #[test]
    fn immediate_and_vertical_blanking_reloads_can_be_asked_for_together() {
        let mut m = layer_with_a_held_back_reload();
        m.memory.write32(SRCR_ADDR, IMR | VBR);
        assert_eq!(m.memory.read32(SRCR_ADDR), VBR);
        assert_eq!(m.memory.frame().unwrap().pixel(0, 0), [0, 0, 0xFF]);
        assert_eq!(m.isr() & RRIF, RRIF);
    }

    #[test]
    fn a_system_reset_cancels_a_held_back_reload() {
        let mut m = layer_with_a_held_back_reload();
        m.memory.reset_peripherals(ResetCause::Software);
        assert_eq!(m.memory.read32(SRCR_ADDR), 0);
        m.memory.write32(TWCR_ADDR, BOARD_TWCR);
        m.memory.write32(GCR_ADDR, LTDCEN);
        m.at(10 * FRAME);
        assert_eq!(m.isr() & RRIF, 0);
    }

    #[test]
    fn layer_2_and_its_colour_table_come_from_its_own_registers() {
        let mut m = Machine::new();
        configure_layer(&mut m);
        m.memory.write8(0x2400_0000, 1);
        m.memory.write32(LTDC_BASE + LAYER_CR, 0x11);
        // Layer 2: the same window, one pixel on in the framebuffer, opaque.
        for (register, value) in [
            (WHPCR, 0x040F_00F0),
            (WVPCR, 0x026F_0018),
            (PFCR, 5),
            (CACR, 0xFF),
            (CFBAR, 0x2400_0001),
            (CFBLR, 0x0320_0327),
            (CFBLNR, 600),
        ] {
            m.memory
                .write32(LTDC_BASE + LAYER_CR + LAYER_STRIDE + register, value);
        }
        m.memory.write8(0x2400_0001, 1);
        m.memory
            .write32(LTDC_BASE + LAYER_CR + LAYER_STRIDE + CLUTWR, 0x0100_FF00);
        m.memory.write32(LTDC_BASE + LAYER_CR + LAYER_STRIDE, 0x11);
        m.memory.write32(SRCR_ADDR, IMR);
        let frame = m.memory.frame().unwrap();
        // Layer 2's table has green at 1 and layer 1's blue, so layer 2 shows over layer 1
        // where its pixel is not transparent: all of it.
        assert_eq!(frame.pixel(0, 0), [0, 0xFF, 0]);
    }
}
