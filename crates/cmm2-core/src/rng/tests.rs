use super::*;
use crate::Peripheral;
use crate::cmm2_memory_with_rng_seed;

// Core cycles: 192 RNG clocks and 213 AHB clocks for the first set at 48 MHz, and 213 AHB
// clocks for each set after (16 RNG clocks are fewer).
const INIT: u64 = 192 * 10 + 213 * 2;
const REFILL: u64 = 213 * 2;
const ON: RngClockState = RngClockState {
    enabled: true,
    in_reset: false,
    kernel_hz: 48_000_000,
};

struct Bench {
    rng: Rng,
    clock: Clock,
    rcc: RngClock,
    irqs: Irqs,
}

impl Bench {
    /// An RNG with its clocks on, not yet enabled.
    fn new(seed: u64) -> Bench {
        let bench = Bench::gated(seed);
        bench.rcc.set(ON);
        bench
    }

    /// An RNG with its clock gate as a reset leaves it: off.
    fn gated(seed: u64) -> Bench {
        let (clock, irqs) = (Clock::new(), Irqs::new());
        let rcc = RngClock::new(clock.clone(), RngClockState::RESET);
        let rng = Rng::new(clock.clone(), rcc.clone(), irqs.clone(), Some(seed));
        Bench {
            rng,
            clock,
            rcc,
            irqs,
        }
    }

    fn read(&mut self, offset: u32) -> u32 {
        self.rng.read(offset, 4)
    }

    fn write(&mut self, offset: u32, value: u32) {
        self.rng.write(offset, 4, value);
    }

    fn at(&mut self, cycle: u64) {
        self.clock.set(cycle);
    }

    fn drdy(&mut self) -> bool {
        self.read(SR) & DRDY != 0
    }

    /// Runs the clock's wake the way the CPU does.
    fn wake(&mut self) {
        if self.clock.take_due() {
            self.rng.tick();
        }
    }
}

#[test]
fn the_registers_read_their_reset_values() {
    let mut b = Bench::new(1);
    assert_eq!((b.read(CR), b.read(SR), b.read(DR)), (0, 0, 0));
}

#[test]
fn only_ced_ie_and_rngen_can_be_written_in_cr() {
    let mut b = Bench::new(1);
    b.write(CR, 0xFFFF_FFFF);
    assert_eq!(b.read(CR), CED | IE | RNGEN);
}

#[test]
fn sr_cannot_be_set_by_writing_it() {
    let mut b = Bench::new(1);
    b.write(SR, 0xFFFF_FFFF);
    assert_eq!(b.read(SR), 0);
}

#[test]
fn ced_cannot_change_while_the_rng_is_enabled() {
    let mut b = Bench::new(1);
    b.write(CR, RNGEN);
    b.write(CR, RNGEN | CED);
    assert_eq!(b.read(CR), RNGEN);
    b.write(CR, 0);
    b.write(CR, CED);
    b.write(CR, CED | RNGEN);
    b.write(CR, RNGEN);
    assert_eq!(b.read(CR), CED | RNGEN, "kept while enabled");
}

#[test]
fn drdy_rises_after_the_first_set_takes_its_kernel_clocks() {
    let mut b = Bench::new(1);
    b.at(5_000);
    b.write(CR, RNGEN);
    b.at(5_000 + INIT - 1);
    assert!(!b.drdy());
    b.at(5_000 + INIT);
    assert!(b.drdy());
}

#[test]
fn nothing_is_made_until_rngen_is_set() {
    let mut b = Bench::new(1);
    b.at(1_000_000);
    assert!(!b.drdy());
    assert_eq!(b.read(DR), 0);
}

#[test]
fn a_slower_kernel_clock_takes_longer() {
    let mut b = Bench::new(1);
    b.rcc.set(RngClockState {
        kernel_hz: 24_000_000,
        ..ON
    });
    b.write(CR, RNGEN);
    // 192 RNG clocks at 24 MHz are 3840 core cycles; the AHB clock is still over the
    // threshold of 213 x 24 MHz / 64.
    b.at(3_840 + 425);
    assert!(!b.drdy());
    b.at(3_840 + 426);
    assert!(b.drdy());
}

#[test]
fn below_the_threshold_it_takes_128_rng_clocks_and_426_ahb_clocks() {
    // 213 x fRNG / 64 is over 240 MHz from an fRNG of 72.2 MHz.
    let hz = 100_000_000;
    assert_eq!(Work::Init.cycles(hz), 128 * 480 / 100 + 1 + 852);
}

#[test]
fn each_read_gives_the_next_word_and_the_fourth_empties_the_buffer() {
    let mut b = Bench::new(1);
    b.write(CR, RNGEN);
    b.at(INIT);
    let words: Vec<u32> = (0..4)
        .map(|_| {
            assert!(b.drdy());
            b.read(DR)
        })
        .collect();
    assert!(words.iter().all(|&w| w != 0));
    assert!(words.windows(2).all(|w| w[0] != w[1]));
    assert!(!b.drdy());
    assert_eq!(b.read(DR), 0, "not ready: zero");
}

#[test]
fn the_next_four_words_come_a_refill_after_the_fourth_read() {
    let mut b = Bench::new(1);
    b.write(CR, RNGEN);
    b.at(INIT);
    for _ in 0..4 {
        b.read(DR);
    }
    b.at(INIT + REFILL - 1);
    assert!(!b.drdy());
    b.at(INIT + REFILL);
    assert!(b.drdy());
}

#[test]
fn the_buffer_stays_ready_between_reads_however_long_they_take() {
    let mut b = Bench::new(1);
    b.write(CR, RNGEN);
    b.at(INIT + 1_000_000);
    b.read(DR);
    b.at(INIT + 2_000_000);
    assert!(b.drdy());
}

#[test]
fn a_fast_kernel_clock_cannot_refill_in_under_213_ahb_clocks() {
    assert_eq!(Work::Refill.cycles(48_000_000), REFILL);
    // At a slow kernel clock 16 RNG clocks are the longer time: 16 x 480 MHz / 1.5 MHz.
    assert_eq!(Work::Refill.cycles(1_500_000), 5_120);
}

#[test]
fn values_differ_between_seeds_and_repeat_for_one_seed() {
    let first = |seed| {
        let mut b = Bench::new(seed);
        b.write(CR, RNGEN);
        b.at(INIT);
        b.read(DR)
    };
    assert_ne!(first(1), first(2));
    assert_eq!(first(1), first(1));
}

#[test]
fn without_a_seed_the_values_come_from_the_host() {
    let first = || {
        let clock = Clock::new();
        let rcc = RngClock::new(clock.clone(), RngClockState::RESET);
        let mut rng = Rng::new(clock.clone(), rcc.clone(), Irqs::new(), None);
        rcc.set(ON);
        rng.write(CR, 4, RNGEN);
        clock.set(INIT);
        rng.read(DR, 4)
    };
    assert_ne!(first(), first());
}

#[test]
fn the_interrupt_is_raised_when_data_is_ready_with_ie_set() {
    let mut b = Bench::new(1);
    b.write(CR, IE | RNGEN);
    assert_eq!(b.irqs.take(), None);
    b.at(INIT);
    b.wake();
    assert_eq!(b.irqs.take(), Some([0, 0, 1 << 16, 0, 0, 0, 0, 0])); // IRQ 80
}

#[test]
fn no_interrupt_without_ie() {
    let mut b = Bench::new(1);
    b.write(CR, RNGEN);
    b.at(INIT);
    b.wake();
    assert!(b.drdy());
    assert_eq!(b.irqs.take(), None);
}

#[test]
fn setting_ie_with_data_already_ready_raises_the_interrupt() {
    let mut b = Bench::new(1);
    b.write(CR, RNGEN);
    b.at(INIT);
    b.write(CR, IE | RNGEN);
    assert!(b.irqs.take().is_some());
}

#[test]
fn reading_data_with_more_waiting_raises_the_interrupt_again() {
    let mut b = Bench::new(1);
    b.write(CR, IE | RNGEN);
    b.at(INIT);
    b.wake();
    b.irqs.take();
    b.read(DR);
    assert!(b.irqs.take().is_some(), "three words are still ready");
    for _ in 0..3 {
        b.read(DR);
    }
    b.irqs.take();
    b.read(DR);
    assert_eq!(b.irqs.take(), None, "the buffer is empty");
}

#[test]
fn the_interrupt_comes_only_with_the_rng_enabled() {
    // Data made, then RNGEN cleared (34.3.8): still ready, but no interrupt (34.4).
    let mut b = Bench::new(1);
    b.write(CR, RNGEN);
    b.at(INIT);
    b.write(CR, 0);
    b.write(CR, IE);
    assert!(b.drdy());
    assert_eq!(b.irqs.take(), None);
}

#[test]
fn data_stays_readable_with_rngen_clear() {
    let mut b = Bench::new(1);
    b.write(CR, RNGEN);
    b.at(INIT);
    b.write(CR, 0);
    assert!(b.drdy());
    assert_ne!(b.read(DR), 0);
}

#[test]
fn the_work_stops_with_rngen_clear_and_resumes_from_where_it_was() {
    let mut b = Bench::new(1);
    b.write(CR, RNGEN);
    b.at(1_000);
    b.write(CR, 0);
    b.at(1_000_000);
    assert!(!b.drdy());
    b.write(CR, RNGEN);
    b.at(1_000_000 + INIT - 1_000 - 1);
    assert!(!b.drdy());
    b.at(1_000_000 + INIT - 1_000);
    assert!(b.drdy());
}

#[test]
fn registers_read_zero_and_ignore_writes_with_the_clock_off() {
    let mut b = Bench::gated(1);
    b.write(CR, RNGEN);
    assert_eq!(b.read(CR), 0);
    b.rcc.set(ON);
    assert_eq!(b.read(CR), 0, "the write never arrived");
}

#[test]
fn the_work_does_not_run_with_the_clock_off() {
    let mut b = Bench::new(1);
    b.write(CR, RNGEN);
    b.at(1_000);
    b.rcc.set(RngClockState::RESET);
    b.at(1_000_000);
    b.rcc.set(ON);
    b.at(1_000_000 + INIT - 1_000 - 1);
    assert!(!b.drdy());
    b.at(1_000_000 + INIT - 1_000);
    assert!(b.drdy());
}

#[test]
fn the_wake_is_scheduled_for_when_the_data_will_be_ready() {
    let mut b = Bench::new(1);
    b.at(100);
    assert!(
        b.clock.take_due(),
        "the wake that the clock coming on asked for"
    );
    b.write(CR, IE | RNGEN);
    b.at(100 + INIT - 1);
    assert!(!b.clock.take_due());
    b.at(100 + INIT);
    assert!(b.clock.take_due());
    b.rng.tick();
    assert!(b.irqs.take().is_some());
}

#[test]
fn the_wake_for_a_refill_is_scheduled_by_the_read_that_empties_the_buffer() {
    let mut b = Bench::new(1);
    b.write(CR, IE | RNGEN);
    b.at(INIT);
    assert!(b.clock.take_due());
    b.rng.tick();
    for _ in 0..4 {
        b.read(DR);
    }
    b.irqs.take();
    b.at(INIT + REFILL - 1);
    assert!(!b.clock.take_due());
    b.at(INIT + REFILL);
    assert!(b.clock.take_due());
    b.rng.tick();
    assert!(b.irqs.take().is_some());
}

#[test]
fn any_write_to_sr_raises_the_interrupt_again_while_it_is_active() {
    let mut b = Bench::new(1);
    b.write(CR, IE | RNGEN);
    b.at(INIT);
    b.wake();
    b.irqs.take();
    b.write(SR, 0xFFFF_FFFF);
    assert!(b.irqs.take().is_some());
}

#[test]
fn the_registers_ignore_writes_while_the_block_is_held_in_reset() {
    let mut b = Bench::new(1);
    b.rcc.set(RngClockState {
        in_reset: true,
        ..ON
    });
    b.write(CR, RNGEN);
    assert_eq!(b.read(CR), 0);
    b.rcc.set(ON);
    assert_eq!(b.read(CR), 0, "the write never arrived");
}

#[test]
fn nothing_is_made_without_a_kernel_clock() {
    let mut b = Bench::new(1);
    b.rcc.set(RngClockState { kernel_hz: 0, ..ON });
    b.write(CR, RNGEN);
    b.at(100 * INIT);
    assert!(!b.drdy());
    assert_eq!(
        b.read(SR) & (CECS | CEIS),
        CECS | CEIS,
        "and the clock check complains"
    );
}

#[test]
fn a_refill_that_has_started_finishes_with_rngen_clear_but_the_first_set_does_not() {
    let mut b = Bench::new(1);
    b.write(CR, RNGEN);
    b.at(INIT);
    for _ in 0..4 {
        b.read(DR);
    }
    b.at(INIT + 10);
    b.write(CR, 0);
    b.at(INIT + 10_000);
    assert!(b.drdy(), "DRDY rose with RNGEN clear");
    // A buffer emptied with RNGEN already clear starts nothing.
    for _ in 0..4 {
        b.read(DR);
    }
    b.at(INIT + 1_000_000);
    assert!(!b.drdy());
    b.write(CR, RNGEN);
    b.at(INIT + 1_000_000 + REFILL);
    assert!(b.drdy());
}

#[test]
fn the_cr_value_survives_the_clock_being_gated() {
    let mut b = Bench::new(1);
    b.write(CR, IE | RNGEN);
    b.rcc.set(RngClockState::RESET);
    b.rcc.set(ON);
    assert_eq!(b.read(CR), IE | RNGEN);
}

#[test]
fn rngrst_puts_the_registers_back_and_drops_the_words() {
    let mut b = Bench::new(1);
    b.write(CR, IE | RNGEN);
    b.at(INIT);
    assert!(b.drdy());
    b.rcc.set(RngClockState {
        in_reset: true,
        ..ON
    });
    b.rcc.set(ON);
    assert_eq!((b.read(CR), b.read(SR), b.read(DR)), (0, 0, 0));
    b.write(CR, RNGEN);
    b.at(INIT + INIT - 1);
    assert!(!b.drdy(), "the first set is made again");
    b.at(INIT + INIT);
    assert!(b.drdy());
}

#[test]
fn a_system_reset_puts_the_registers_back() {
    let mut b = Bench::new(1);
    b.write(CR, IE | RNGEN);
    b.at(INIT);
    b.rng.reset(ResetCause::Software);
    b.rcc.set(ON);
    assert_eq!((b.read(CR), b.read(SR)), (0, 0));
}

#[test]
fn a_clock_too_slow_sets_cecs_and_ceis_and_cecs_follows_the_clock() {
    // LSE at 32.768 kHz is under the AHB clock / 32 (7.5 MHz).
    let mut b = Bench::new(1);
    b.rcc.set(RngClockState {
        kernel_hz: 32_768,
        ..ON
    });
    assert_eq!(b.read(SR), 0, "not while disabled");
    b.write(CR, RNGEN);
    assert_eq!(b.read(SR) & (CECS | CEIS), CECS | CEIS);
    b.rcc.set(ON);
    b.at(10);
    assert_eq!(
        b.read(SR) & (CECS | CEIS),
        CEIS,
        "recovered: only the flag stays"
    );
    b.write(SR, !CEIS);
    assert_eq!(b.read(SR) & (CECS | CEIS), 0);
}

#[test]
fn clearing_ceis_with_the_clock_still_slow_leaves_it_clear() {
    let mut b = Bench::new(1);
    b.rcc.set(RngClockState {
        kernel_hz: 32_768,
        ..ON
    });
    b.write(CR, RNGEN);
    b.write(SR, !CEIS);
    assert_eq!(b.read(SR) & (CECS | CEIS), CECS);
}

#[test]
fn writing_1_to_ceis_or_seis_does_nothing() {
    let mut b = Bench::new(1);
    b.rcc.set(RngClockState {
        kernel_hz: 32_768,
        ..ON
    });
    b.write(CR, RNGEN);
    b.write(SR, 0xFFFF_FFFF);
    assert_eq!(b.read(SR) & (CECS | CEIS), CECS | CEIS);
}

#[test]
fn ced_set_turns_the_clock_check_off() {
    let mut b = Bench::new(1);
    b.rcc.set(RngClockState {
        kernel_hz: 32_768,
        ..ON
    });
    b.write(CR, CED | RNGEN);
    assert_eq!(b.read(SR) & (CECS | CEIS), 0);
}

#[test]
fn a_clock_error_raises_the_interrupt_when_ie_is_set() {
    let mut b = Bench::new(1);
    b.rcc.set(RngClockState {
        kernel_hz: 32_768,
        ..ON
    });
    b.write(CR, IE | RNGEN);
    assert!(b.irqs.take().is_some());
}

#[test]
fn a_seed_error_sets_seis_and_secs_and_stops_the_generator() {
    let mut b = Bench::new(1);
    b.write(CR, RNGEN);
    b.rng.inner_mut().seed_error();
    b.at(10 * INIT);
    assert_eq!(b.read(SR), SEIS | SECS, "no DRDY either");
}

#[test]
fn a_seed_error_clears_with_seis_then_twelve_words() {
    let mut b = Bench::new(1);
    b.write(CR, RNGEN);
    b.rng.inner_mut().seed_error();
    b.write(SR, !SEIS);
    assert_eq!(
        b.read(SR) & (SEIS | SECS),
        SECS,
        "SEIS cleared, SECS not yet"
    );
    let mut now = 0;
    for read in 1..=12 {
        now += INIT;
        b.at(now);
        assert!(b.drdy(), "word {read}");
        b.read(DR);
        let secs = b.read(SR) & SECS;
        assert_eq!(secs != 0, read < 12, "after {read} words");
    }
    assert_eq!(b.read(SR) & (SEIS | SECS), 0);
}

#[test]
fn a_seed_error_raises_the_interrupt_with_ie_set() {
    let mut b = Bench::new(1);
    b.write(CR, IE | RNGEN);
    assert_eq!(b.irqs.take(), None);
    b.rng.inner_mut().seed_error();
    assert!(b.irqs.take().is_some());
}

#[test]
fn the_rng_follows_the_rccs_gate_and_reset_in_the_machine() {
    let mut m = cmm2_memory_with_rng_seed(&Irqs::new(), 1);
    const AHB2ENR: u32 = 0x5802_44DC;
    const C1_AHB2ENR: u32 = 0x5802_453C;
    const AHB2RSTR: u32 = 0x5802_4484;
    const RCC_CR: u32 = 0x5802_4400;
    const RC48ON: u32 = 1 << 12;
    const RNG_CR: u32 = RNG_BASE;
    m.write32(RNG_CR, RNGEN);
    assert_eq!(m.read32(RNG_CR), 0, "no clock, no write");
    // The other offset of the same register turns the clock on too, and shows in the first.
    m.write32(C1_AHB2ENR, 1 << 6);
    assert_eq!(m.read32(AHB2ENR) >> 6 & 1, 1);
    assert_eq!(m.read32(C1_AHB2ENR) >> 6 & 1, 1);
    m.write32(RCC_CR, RC48ON);
    m.write32(RNG_CR, RNGEN);
    m.clock().set(INIT);
    assert_eq!(m.read32(RNG_BASE + SR), DRDY);
    m.write32(AHB2RSTR, 1 << 6);
    m.write32(AHB2RSTR, 0);
    assert_eq!(m.read32(RNG_BASE + SR), 0);
    m.write32(AHB2ENR, 0);
    assert_eq!(m.read32(RNG_CR), 0);
}

#[test]
fn rngsel_picks_the_kernel_clock_in_the_machine() {
    let mut m = cmm2_memory_with_rng_seed(&Irqs::new(), 1);
    m.write32(0x5802_44DC, 1 << 6); // RCC_AHB2ENR: RNGEN
    m.write32(0x5802_4454, 2 << 8); // RCC_D2CCIP2R: RNGSEL = lse_ck
    m.write32(0x5802_4470, 1); // RCC_BDCR: LSEON
    m.write32(RNG_BASE, RNGEN);
    m.clock().set(INIT);
    assert_eq!(
        m.read32(RNG_BASE + SR),
        CECS | CEIS,
        "the init takes far longer on lse_ck"
    );
    // 192 clocks of 32.768 kHz, and the 213 AHB clocks.
    m.clock().set(2_812_500 + 425);
    assert_eq!(m.read32(RNG_BASE + SR) & DRDY, 0);
    m.clock().set(2_812_500 + 426);
    assert_eq!(m.read32(RNG_BASE + SR) & DRDY, DRDY);
}

#[test]
fn rngsel_lsi_is_32_khz() {
    let mut m = cmm2_memory_with_rng_seed(&Irqs::new(), 1);
    m.write32(0x5802_44DC, 1 << 6);
    m.write32(0x5802_4454, 3 << 8); // RNGSEL = lsi_ck
    m.write32(0x5802_4474, 1); // RCC_CSR: LSION
    m.write32(RNG_BASE, RNGEN);
    m.clock().set(192 * 15_000 + 425);
    assert_eq!(m.read32(RNG_BASE + SR) & DRDY, 0);
    m.clock().set(192 * 15_000 + 426);
    assert_eq!(m.read32(RNG_BASE + SR) & DRDY, DRDY);
}

#[test]
fn no_kernel_clock_while_the_oscillator_rngsel_picks_is_off() {
    let mut m = cmm2_memory_with_rng_seed(&Irqs::new(), 1);
    m.write32(0x5802_44DC, 1 << 6);
    m.write32(RNG_BASE, RNGEN);
    m.clock().set(10 * INIT);
    assert_eq!(m.read32(RNG_BASE + SR) & DRDY, 0, "hsi48 is off");
    m.write32(0x5802_4400, 1 << 12); // RCC_CR: RC48ON
    m.clock().set(10 * INIT + INIT);
    assert_eq!(m.read32(RNG_BASE + SR) & DRDY, DRDY);
}

#[test]
fn a_system_reset_takes_the_clock_gate_back_to_off() {
    let mut m = cmm2_memory_with_rng_seed(&Irqs::new(), 1);
    m.write32(0x5802_44DC, 1 << 6);
    m.write32(0x5802_4400, 1 << 12);
    m.write32(RNG_BASE, RNGEN);
    assert_eq!(m.read32(RNG_BASE), RNGEN);
    m.reset_peripherals(ResetCause::Software);
    // RCC_AHB2ENR is clear again, so the RNG's registers do not answer or take writes.
    m.write32(RNG_BASE, RNGEN);
    assert_eq!(m.read32(RNG_BASE), 0);
    // The firmware turns the clock on again and the RNG works, with the first write kept.
    m.write32(0x5802_44DC, 1 << 6);
    m.write32(0x5802_4400, 1 << 12);
    m.write32(RNG_BASE, RNGEN);
    m.clock().set(100_000);
    assert_eq!(m.read32(RNG_BASE + SR), DRDY);
}

#[test]
fn a_write_of_the_same_rcc_bits_after_a_reset_is_not_lost() {
    // Only RNGEN is set before the reset, so the RCC's state after the reset (clock off) and
    // after the write of the same value (on, with no kernel clock) are two different states,
    // and the RNG has to have heard of both. If the RCC kept its old state across the reset
    // and the RNG took "off", the write would log no change and the RNG would never hear.
    let mut m = cmm2_memory_with_rng_seed(&Irqs::new(), 1);
    m.write32(0x5802_44DC, 1 << 6);
    m.reset_peripherals(ResetCause::Software);
    m.write32(0x5802_44DC, 1 << 6);
    m.write32(RNG_BASE, RNGEN);
    assert_eq!(m.read32(RNG_BASE), RNGEN);
}

#[test]
fn each_oscillator_must_be_on_for_rngsel_to_give_a_kernel_clock() {
    // RNGSEL, the register that turns the oscillator on, and its bit.
    for (sel, on_register, on_bit) in [
        (1u32, 0x5802_4400u32, 1u32 << 24), // pll1_q, RCC_CR PLL1ON
        (2, 0x5802_4470, 1),                // lse_ck, RCC_BDCR LSEON
        (3, 0x5802_4474, 1),                // lsi_ck, RCC_CSR LSION
    ] {
        let mut m = cmm2_memory_with_rng_seed(&Irqs::new(), 1);
        m.write32(0x5802_44DC, 1 << 6);
        m.write32(0x5802_4454, sel << 8);
        m.write32(RNG_BASE, RNGEN);
        m.clock().set(10_000_000);
        assert_eq!(
            m.read32(RNG_BASE + SR) & DRDY,
            0,
            "RNGSEL {sel} with its oscillator off"
        );
        m.write32(on_register, on_bit);
        // pll1_q is 48 MHz as I take it, so the first set takes what it does on hsi48.
        let done = if sel == 1 {
            10_000_000 + INIT
        } else {
            20_000_000
        };
        m.clock().set(done);
        assert_eq!(
            m.read32(RNG_BASE + SR) & DRDY,
            DRDY,
            "RNGSEL {sel} with it on"
        );
        if sel == 1 {
            // And not a cycle sooner: a slower pll1_q would take longer, a faster one less.
            let mut m = cmm2_memory_with_rng_seed(&Irqs::new(), 1);
            m.write32(0x5802_44DC, 1 << 6);
            m.write32(0x5802_4454, 1 << 8);
            m.write32(0x5802_4400, 1 << 24);
            m.write32(RNG_BASE, RNGEN);
            m.clock().set(INIT - 1);
            assert_eq!(m.read32(RNG_BASE + SR) & DRDY, 0);
            m.clock().set(INIT);
            assert_eq!(m.read32(RNG_BASE + SR) & DRDY, DRDY);
        }
    }
}

#[test]
fn with_no_kernel_clock_enabling_with_ie_raises_the_interrupt_for_the_clock_error() {
    let mut b = Bench::new(1);
    b.rcc.set(RngClockState { kernel_hz: 0, ..ON });
    b.write(CR, IE | RNGEN);
    assert_eq!(b.irqs.take(), Some([0, 0, 1 << 16, 0, 0, 0, 0, 0])); // IRQ 80
}

#[test]
fn a_refill_waits_while_the_kernel_clock_is_off_and_resumes() {
    let mut b = Bench::new(1);
    b.write(CR, RNGEN);
    b.at(INIT);
    for _ in 0..4 {
        b.read(DR);
    }
    // 100 cycles of the 426 are done when the kernel clock goes.
    b.at(INIT + 100);
    b.rcc.set(RngClockState { kernel_hz: 0, ..ON });
    b.at(INIT + 100_000);
    assert!(!b.drdy());
    b.rcc.set(ON);
    assert!(!b.drdy(), "it does not finish at once");
    b.at(INIT + 100_000 + REFILL - 100 - 1);
    assert!(!b.drdy());
    b.at(INIT + 100_000 + REFILL - 100);
    assert!(b.drdy());
}
