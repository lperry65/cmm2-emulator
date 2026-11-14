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
    m.memory.write32(LTDC_BASE + 0x84, 0x11); // L1CR, shadowed
    assert_eq!(m.memory.read32(LTDC_BASE + 0x08), 0x004F_0002);
    assert_eq!(m.memory.read32(LTDC_BASE + 0x84), 0);
    m.memory.write32(SRCR_ADDR, IMR);
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
fn a_layer_register_reads_back_the_active_value_until_the_reload() {
    // RM0433 32.4.1 (p. 1208) and 32.7.6 (p. 1218): the shadow registers read back the
    // active values, and what was written can be read only after the reload.
    let mut m = Machine::new();
    let cfbar = LTDC_BASE + LAYER_CR + CFBAR;
    assert_eq!(m.memory.read32(cfbar), 0);
    m.memory.write32(cfbar, 0x2400_0000);
    assert_eq!(m.memory.read32(cfbar), 0);
    // A held back reload keeps the old value too.
    m.memory.write32(SRCR_ADDR, VBR);
    assert_eq!(m.memory.read32(cfbar), 0);
    m.memory.write32(SRCR_ADDR, IMR);
    assert_eq!(m.memory.read32(cfbar), 0x2400_0000);
    // Layer 2 has its own, and a write to the same register again before the reload
    // overrides the first.
    let layer_2 = cfbar + LAYER_STRIDE;
    m.memory.write32(layer_2, 0x2400_1000);
    m.memory.write32(layer_2, 0x2400_2000);
    assert_eq!(m.memory.read32(layer_2), 0);
    m.memory.write32(SRCR_ADDR, IMR);
    assert_eq!(m.memory.read32(layer_2), 0x2400_2000);
    assert_eq!(m.memory.read32(cfbar), 0x2400_0000);
    // The registers around the layers are not shadowed.
    m.memory.write32(LIPCR_ADDR, 5);
    assert_eq!(m.memory.read32(LIPCR_ADDR), 5);
}

#[test]
fn the_layer_registers_reset_to_the_reference_manuals_values() {
    // RM0433 32.7.19 (p. 1226) and 32.7.21 (p. 1227): CACR 0xFF and BFCR 0x0607.
    let mut m = Machine::new();
    assert_eq!(m.memory.read32(LTDC_BASE + LAYER_CR + CACR), 0xFF);
    assert_eq!(m.memory.read32(LTDC_BASE + LAYER_CR + BFCR), 0x0607);
    assert_eq!(
        m.memory.read32(LTDC_BASE + LAYER_CR + LAYER_STRIDE + BFCR),
        0x0607
    );
    // And a reload that takes what was never written keeps them.
    m.memory.write32(SRCR_ADDR, IMR);
    assert_eq!(m.memory.read32(LTDC_BASE + LAYER_CR + CACR), 0xFF);
    assert_eq!(
        m.memory.read32(LTDC_BASE + LAYER_CR + LAYER_STRIDE + CACR),
        0xFF
    );
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
