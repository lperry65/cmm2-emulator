//! The LCD-TFT controller. For now it draws nothing; it keeps the timing the firmware waits on.
//! After every frame's active area the firmware's display code expects a line interrupt (IRQ
//! 88), and the BASIC interpreter's main thread waits for it, so without it the prompt never
//! runs a command.
//!
//! The frame is worked out from the machine's clock, from the total size the firmware writes to
//! TWCR and the 49.5 MHz pixel clock, so it comes round at 75 Hz as on the board. Once the
//! controller is enabled (GCR.LTDCEN) the line in LIPCR is reached once a frame, which sets
//! ISR.LIF and, with IER.LIE set, raises the interrupt. ICR clears the flags. The flag is
//! worked out whenever ISR is read, so it does not need the interrupt to be on; the interrupt
//! asks the clock for a tick, because the firmware does not touch the controller while it waits.
//!
//! A write of IMR or VBR to SRCR reloads the shadow registers, which are not shadowed here, so it
//! completes at once: the bit reads back 0 and ISR.RRIF is set. The reload on VBR is not held
//! back to the next vertical blank. The other flags (FU, TERR) are never set, as nothing is
//! fetched, so the error interrupt (IRQ 89) is never raised. CDSR and CPSR, the position the
//! controller is at, are plain stored registers: the firmware does not read them.
//!
//! The interrupt line is a level on the real chip, but `Irqs` is edge like, so the line is raised
//! when a flag is set that is enabled, and when an enable is written for a flag that is set.

use crate::clock::{CORE_HZ, Clock};
use crate::irq::Irqs;
use crate::peripheral::{ResetCause, StubRegisters};
use crate::registers::{Registers, WordAccess};
use crate::svd::Svd;

pub const LTDC_BASE: u32 = 0x5000_1000;
pub const LTDC_SIZE: u32 = 0x1000;

/// `LTDC_IRQn`, the global interrupt.
const LTDC_IRQ: u16 = 88;

/// The pixel clock on the board: PLL3's 792 MHz VCO divided by 16. I take it as a constant;
/// the clock tree is not modelled to that depth.
const PIXEL_HZ: u128 = 49_500_000;

// Register offsets and bit positions, as the SVD gives them.
const TWCR: u32 = 0x14;
const GCR: u32 = 0x18;
const SRCR: u32 = 0x24;
const IER: u32 = 0x34;
const ISR: u32 = 0x38;
const ICR: u32 = 0x3C;
const LIPCR: u32 = 0x40;

const LTDCEN: u32 = 1 << 0;
const LIF: u32 = 1 << 0;
const RRIF: u32 = 1 << 3;
const FLAGS: u32 = 0xF;
const IMR_OR_VBR: u32 = 0b11;
const LIPOS_MASK: u32 = 0x7FF;

/// TWCR, GCR and LIPCR are stored; ISR is worked out. While the controller is enabled,
/// `frame_start` is the core cycle the frame began at, and `next_line` is the pixel, counted
/// from it, of the next line event that has not been flagged yet. Both are `None` otherwise.
/// `next_line` is also `None` when LIPCR is beyond the last line, which is never reached.
#[derive(Debug)]
pub struct LtdcRegisters {
    stored: StubRegisters,
    clock: Clock,
    irqs: Irqs,
    isr: u32,
    frame_start: Option<u64>,
    next_line: Option<u64>,
}

fn cycles_to_pixels(cycles: u64) -> u64 {
    (u128::from(cycles) * PIXEL_HZ / u128::from(CORE_HZ)) as u64
}

/// The first whole cycle at which `pixels` pixels have been drawn.
fn pixels_to_cycles(pixels: u64) -> u64 {
    (u128::from(pixels) * u128::from(CORE_HZ)).div_ceil(PIXEL_HZ) as u64
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

    fn pixels_now(&self, frame_start: u64) -> u64 {
        cycles_to_pixels(self.clock.now().saturating_sub(frame_start))
    }

    /// The first pixel from `from` on that is the start of the line in LIPCR, in this frame or
    /// a later one.
    fn line_event_from(&mut self, from: u64) -> Option<u64> {
        let (width, height) = self.total_size();
        let line = u64::from(self.stored.read(LIPCR) & LIPOS_MASK);
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

    /// Flags the line event if it has come, and moves on to the next one. However many frames
    /// have passed, the flag is set once.
    fn catch_up(&mut self) {
        let (Some(start), Some(due)) = (self.frame_start, self.next_line) else {
            return;
        };
        let now = self.pixels_now(start);
        if now >= due {
            self.isr |= LIF;
            self.next_line = self.line_event_from(now + 1);
            self.raise_if_enabled();
        }
    }

    /// Works out the next line event again, after the controller or the line changed, and has
    /// the clock tick when it comes if the interrupt is on.
    fn arm(&mut self) {
        let Some(start) = self.frame_start else {
            self.next_line = None;
            return;
        };
        let now = self.pixels_now(start);
        self.next_line = self.line_event_from(now);
        self.schedule();
    }

    // A tick that comes for another device lands here too, as the clock keeps one wake.
    fn schedule(&mut self) {
        if let (Some(start), Some(due), true) = (
            self.frame_start,
            self.next_line,
            self.stored.read(IER) & LIF != 0,
        ) {
            self.clock.wake_at(start + pixels_to_cycles(due));
        }
    }

    fn raise_if_enabled(&mut self) {
        if self.isr & self.stored.read(IER) != 0 {
            self.irqs.raise(LTDC_IRQ);
        }
    }

    fn write_gcr(&mut self, value: u32, mask: u32) {
        let was_enabled = self.stored.read(GCR) & LTDCEN != 0;
        self.stored.write(GCR, value, mask);
        let enabled = self.stored.read(GCR) & LTDCEN != 0;
        if enabled != was_enabled {
            self.frame_start = enabled.then(|| self.clock.now());
            self.arm();
        }
    }
}

impl Registers for LtdcRegisters {
    fn read(&mut self, offset: u32) -> u32 {
        self.catch_up();
        match offset {
            ISR => self.isr,
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
            ICR => self.isr &= !(value & mask & FLAGS),
            SRCR => {
                if value & mask & IMR_OR_VBR != 0 {
                    self.isr |= RRIF;
                    self.raise_if_enabled();
                }
            }
            ISR => {}
            _ => self.stored.write(offset, value, mask),
        }
    }

    fn reset(&mut self, cause: ResetCause) {
        self.stored.reset(cause);
        self.isr = 0;
        self.frame_start = None;
        self.next_line = None;
    }

    fn tick(&mut self) {
        self.catch_up();
        self.schedule();
    }
}

pub type Ltdc = WordAccess<LtdcRegisters>;

impl Ltdc {
    pub fn new(clock: Clock, irqs: Irqs) -> Ltdc {
        WordAccess::wrap(LtdcRegisters {
            stored: StubRegisters::with_reset_values(LTDC_BASE, Svd::stm32h743()),
            clock,
            irqs,
            isr: 0,
            frame_start: None,
            next_line: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryMap, ResetCause};

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

    struct Machine {
        memory: MemoryMap,
        clock: Clock,
        irqs: Irqs,
    }

    impl Machine {
        fn new() -> Machine {
            let mut memory = MemoryMap::new();
            let clock = memory.clock();
            let irqs = Irqs::new();
            memory
                .map_peripheral(
                    LTDC_BASE,
                    LTDC_SIZE,
                    Box::new(Ltdc::new(clock.clone(), irqs.clone())),
                )
                .unwrap();
            Machine {
                memory,
                clock,
                irqs,
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
        m.memory.write32(SRCR_ADDR, 1);
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
    fn a_reload_completes_at_once_and_flags_it() {
        for bit in [1, 2] {
            let mut m = Machine::new();
            m.memory.write32(SRCR_ADDR, bit);
            assert_eq!(m.memory.read32(SRCR_ADDR), 0, "bit {bit}");
            assert_eq!(m.isr(), RRIF, "bit {bit}");
        }
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
}
