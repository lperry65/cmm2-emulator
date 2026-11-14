use super::*;
use crate::{Peripheral, ResetCause};

const TIM16: u32 = 0x4001_4400;
const TIM5: u32 = 0x4000_0C00;
const CR1: u32 = 0x00;
const DIER: u32 = 0x0C;
const SR: u32 = 0x10;
const EGR: u32 = 0x14;
const CNT: u32 = 0x24;
const PSC: u32 = 0x28;
const ARR: u32 = 0x2C;
const CCR1: u32 = 0x34;

const CEN: u32 = 1;
const OPM: u32 = 1 << 3;
const UIF: u32 = 1;
const UG: u32 = 1;
const UIE: u32 = 1;
const UDIS: u32 = 1 << 1;
const URS: u32 = 1 << 2;
const DIR: u32 = 1 << 4;
const ARPE: u32 = 1 << 7;

fn timer(base: u32, width: u32) -> (Timer, Clock) {
    let clock = Clock::new();
    (Timer::new(base, width, clock.clone()), clock)
}

fn write(t: &mut Timer, offset: u32, value: u32) {
    t.write(offset, 4, value);
}

fn read(t: &mut Timer, offset: u32) -> u32 {
    t.read(offset, 4)
}

// A 16-bit timer counting from 0 at cycle 0, with the prescaler and reload given.
fn running(psc: u32, arr: u32) -> (Timer, Clock) {
    let (mut t, clock) = timer(TIM16, 16);
    write(&mut t, PSC, psc);
    write(&mut t, ARR, arr);
    // The buffers load at an update event; software forces one, as the HAL does.
    write(&mut t, EGR, UG);
    write(&mut t, SR, 0);
    write(&mut t, CR1, CEN);
    (t, clock)
}

#[test]
fn a_stopped_timer_stays_at_zero() {
    let (mut t, clock) = timer(TIM16, 16);
    assert_eq!(read(&mut t, CNT), 0);
    clock.set(1_000_000);
    assert_eq!(read(&mut t, CNT), 0);
}

#[test]
fn with_no_prescaler_it_counts_half_the_cycles() {
    let (mut t, clock) = running(0, 0xFFFF);
    clock.set(1000);
    assert_eq!(read(&mut t, CNT), 500);
}

#[test]
fn the_prescaler_slows_the_count() {
    let (mut t, clock) = running(239, 0xFFFF);
    clock.set(479);
    assert_eq!(read(&mut t, CNT), 0);
    clock.set(480);
    assert_eq!(read(&mut t, CNT), 1);
    clock.set(480 * 100);
    assert_eq!(read(&mut t, CNT), 100);
}

#[test]
fn a_partial_tick_is_not_lost() {
    let (mut t, clock) = running(239, 0xFFFF);
    clock.set(479);
    assert_eq!(read(&mut t, CNT), 0);
    clock.set(481);
    assert_eq!(read(&mut t, CNT), 1);
    clock.set(960);
    assert_eq!(read(&mut t, CNT), 2);
}

#[test]
fn the_count_wraps_at_the_reload_and_sets_uif() {
    let (mut t, clock) = running(0, 9);
    clock.set(50); // 25 ticks
    assert_eq!(read(&mut t, CNT), 5);
    assert_eq!(read(&mut t, SR) & UIF, UIF);
}

#[test]
fn many_wraps_between_reads_leave_the_remainder() {
    let (mut t, clock) = running(0, 99);
    clock.set(2_000_000 + 2 * 37); // 1,000,037 ticks, period 100
    assert_eq!(read(&mut t, CNT), 37);
    assert_eq!(read(&mut t, SR) & UIF, UIF);
}

#[test]
fn uif_stays_clear_without_a_wrap() {
    let (mut t, clock) = running(0, 99);
    clock.set(100); // 50 ticks
    assert_eq!(read(&mut t, CNT), 50);
    assert_eq!(read(&mut t, SR), 0);
}

#[test]
fn a_full_period_wraps_to_zero_and_one_tick_short_does_not() {
    let (mut t, clock) = running(0, 9);
    clock.set(18); // 9 ticks: the last count before the wrap
    assert_eq!(read(&mut t, CNT), 9);
    assert_eq!(read(&mut t, SR), 0);
    clock.set(20); // 10 ticks: back to 0 with the flag set
    assert_eq!(read(&mut t, CNT), 0);
    assert_eq!(read(&mut t, SR) & UIF, UIF);
}

#[test]
fn writing_zero_clears_uif_and_one_does_not() {
    let (mut t, clock) = running(0, 9);
    clock.set(50);
    read(&mut t, CNT);
    write(&mut t, SR, UIF);
    assert_eq!(read(&mut t, SR), UIF);
    write(&mut t, SR, 0);
    assert_eq!(read(&mut t, SR), 0);
}

#[test]
fn a_16_bit_timer_masks_the_count() {
    let (mut t, _) = timer(TIM16, 16);
    write(&mut t, CNT, 0x1234_5678);
    assert_eq!(read(&mut t, CNT), 0x5678);
}

#[test]
fn a_32_bit_timer_counts_past_2_pow_31() {
    let (mut t, clock) = timer(TIM5, 32);
    write(&mut t, ARR, 0xFFFF_FFFF);
    write(&mut t, CR1, CEN);
    clock.set(2 * 0x9000_0000);
    assert_eq!(read(&mut t, CNT), 0x9000_0000);
    assert_eq!(read(&mut t, SR), 0);
    clock.set(2 * 0x1_0000_0005);
    assert_eq!(read(&mut t, CNT), 5);
    assert_eq!(read(&mut t, SR), UIF);
}

#[test]
fn clearing_cen_freezes_the_count_and_setting_it_resumes() {
    let (mut t, clock) = running(0, 0xFFFF);
    clock.set(100);
    write(&mut t, CR1, 0);
    clock.set(1000);
    assert_eq!(read(&mut t, CNT), 50);
    write(&mut t, CR1, CEN);
    clock.set(1100);
    assert_eq!(read(&mut t, CNT), 100);
}

#[test]
fn a_count_write_sets_the_count_and_restarts_the_tick() {
    let (mut t, clock) = running(239, 0xFFFF);
    clock.set(400);
    write(&mut t, CNT, 1000);
    clock.set(879); // 479 cycles after the write
    assert_eq!(read(&mut t, CNT), 1000);
    clock.set(880);
    assert_eq!(read(&mut t, CNT), 1001);
}

#[test]
fn one_pulse_mode_stops_at_the_first_wrap() {
    let (mut t, clock) = timer(TIM16, 16);
    write(&mut t, ARR, 9);
    write(&mut t, CR1, CEN | OPM);
    clock.set(50);
    assert_eq!(read(&mut t, CNT), 0);
    assert_eq!(read(&mut t, CR1) & CEN, 0);
    assert_eq!(read(&mut t, SR), UIF);
    clock.set(500);
    assert_eq!(read(&mut t, CNT), 0);
}

#[test]
fn a_prescaler_change_keeps_the_past_count() {
    let (mut t, clock) = running(0, 0xFFFF);
    clock.set(200); // 100 ticks at the old rate
    write(&mut t, PSC, 9);
    write(&mut t, EGR, UG); // loads the new prescaler and restarts the count
    write(&mut t, CNT, 100);
    clock.set(200 + 2 * 10 * 3); // 3 ticks at the new rate
    assert_eq!(read(&mut t, CNT), 103);
}

#[test]
fn ug_zeroes_the_count_and_sets_uif() {
    let (mut t, clock) = running(0, 0xFFFF);
    clock.set(100);
    write(&mut t, EGR, 1);
    assert_eq!(read(&mut t, SR), UIF);
    assert_eq!(read(&mut t, CNT), 0);
    assert_eq!(read(&mut t, EGR), 0);
    clock.set(110);
    assert_eq!(read(&mut t, CNT), 5);
}

#[test]
fn the_board_tim16_counts_microseconds_and_wraps_at_20_hz() {
    let (mut t, clock) = running(239, 49_999);
    clock.set(480 * 1234);
    assert_eq!(read(&mut t, CNT), 1234);
    assert_eq!(read(&mut t, SR), 0);
    clock.set(480 * 50_000 + 480 * 7);
    assert_eq!(read(&mut t, CNT), 7);
    assert_eq!(read(&mut t, SR), UIF);
}

#[test]
fn the_board_tim5_runs_at_the_timer_clock() {
    let (mut t, clock) = timer(TIM5, 32);
    write(&mut t, CR1, 0x81);
    clock.set(1_000_000);
    assert_eq!(read(&mut t, CNT), 500_000);
}

#[test]
fn other_registers_store_writes_and_keep_the_reset_values() {
    let (mut t, _) = timer(TIM16, 16);
    assert_eq!(read(&mut t, ARR), 0xFFFF);
    assert_eq!(read(&mut t, PSC), 0);
    write(&mut t, DIER, 1);
    write(&mut t, CCR1, 0x1234);
    assert_eq!(read(&mut t, DIER), 1);
    assert_eq!(read(&mut t, CCR1), 0x1234);
    let (mut t5, _) = timer(TIM5, 32);
    assert_eq!(read(&mut t5, ARR), 0xFFFF_FFFF);
}

#[test]
fn the_count_reads_by_byte_and_halfword() {
    let (mut t, _) = timer(TIM5, 32);
    write(&mut t, CNT, 0x1234_5678);
    assert_eq!(t.read(CNT, 2), 0x5678);
    assert_eq!(t.read(CNT + 2, 2), 0x1234);
    assert_eq!(t.read(CNT + 1, 1), 0x56);
    t.write(CNT, 2, 0xABCD);
    assert_eq!(read(&mut t, CNT), 0x1234_ABCD);
}

#[test]
fn an_arr_write_changes_the_period() {
    let (mut t, clock) = running(0, 0xFFFF);
    clock.set(100); // 50 ticks
    write(&mut t, ARR, 59);
    clock.set(100 + 2 * 20); // 20 more, past the new period
    assert_eq!(read(&mut t, CNT), 10);
    assert_eq!(read(&mut t, SR), UIF);
    assert_eq!(read(&mut t, ARR), 59);
}

#[test]
fn cr1_keeps_the_bits_the_model_does_not_use() {
    let (mut t, _) = timer(TIM16, 16);
    let arpe_udis_urs = (1 << 7) | (1 << 1) | (1 << 2);
    write(&mut t, CR1, arpe_udis_urs | CEN);
    assert_eq!(read(&mut t, CR1), arpe_udis_urs | CEN);
    write(&mut t, CR1, arpe_udis_urs);
    assert_eq!(read(&mut t, CR1), arpe_udis_urs);
}

#[test]
fn reset_stops_the_timer_and_clears_count_and_flags() {
    let (mut t, clock) = running(0, 99);
    clock.set(1000);
    assert_ne!(read(&mut t, SR) & UIF, 0);
    write(&mut t, DIER, 1);
    t.reset(ResetCause::Software);
    assert_eq!(read(&mut t, CR1) & CEN, 0);
    assert_eq!(read(&mut t, CNT), 0);
    assert_eq!(read(&mut t, SR) & UIF, 0);
    assert_eq!(read(&mut t, PSC), 0);
    assert_eq!(read(&mut t, ARR), 0xFFFF);
    assert_eq!(read(&mut t, DIER), 0);
    clock.set(5000);
    assert_eq!(read(&mut t, CNT), 0);
}

#[test]
fn a_reset_timer_counts_from_the_time_of_the_reset() {
    let (mut t, clock) = running(0, 0xFFFF);
    clock.set(1000);
    t.reset(ResetCause::Software);
    write(&mut t, CR1, CEN);
    clock.set(1200);
    assert_eq!(read(&mut t, CNT), 100);
}

// The update interrupt, the scheduled wake and the real-chip details of the update event.

fn wired(spec: TimerSpec) -> (Timer, Clock, Irqs) {
    let clock = Clock::new();
    let irqs = Irqs::new();
    (
        Timer::with_interrupt(spec, clock.clone(), irqs.clone()),
        clock,
        irqs,
    )
}

// A wired timer that is set up as the HAL does it: PSC and ARR, an update to load them, the
// flag cleared, UIE on, then CEN. It counts from cycle 0.
fn armed(spec: TimerSpec, psc: u32, arr: u32) -> (Timer, Clock, Irqs) {
    let (mut t, clock, irqs) = wired(spec);
    write(&mut t, PSC, psc);
    write(&mut t, ARR, arr);
    write(&mut t, EGR, UG);
    write(&mut t, SR, 0);
    write(&mut t, DIER, UIE);
    write(&mut t, CR1, CEN);
    irqs.take();
    (t, clock, irqs)
}

fn raised(irqs: &Irqs, line: u16) -> bool {
    irqs.take()
        .is_some_and(|words| words[usize::from(line / 32)] & 1 << (line % 32) != 0)
}

// Runs the timer the way the CPU does: to each wake in turn, ticking it, until the line is
// raised. Returns the cycle that happened at, or None if no wake up to `limit` raised it.
fn first_raise(t: &mut Timer, clock: &Clock, irqs: &Irqs, line: u16, limit: u64) -> Option<u64> {
    while let Some(wake) = clock.next_wake() {
        if wake > limit {
            return None;
        }
        clock.set(wake.max(clock.now()));
        if clock.take_due() {
            t.tick();
            if raised(irqs, line) {
                return Some(clock.now());
            }
        }
    }
    None
}

#[test]
fn the_lines_are_the_h743_vectors() {
    assert_eq!(TIM5_SPEC.irq, 50);
    assert_eq!(TIM16_SPEC.irq, 117);
}

#[test]
fn a_wrap_with_uie_sets_uif_and_raises_the_line() {
    let (mut t, clock, irqs) = armed(TIM16_SPEC, 0, 9);
    clock.set(19);
    t.tick();
    assert!(!raised(&irqs, 117));
    clock.set(20); // 10 ticks: the wrap
    t.tick();
    assert!(raised(&irqs, 117));
    assert_eq!(read(&mut t, SR), UIF);
}

#[test]
fn tim5_raises_its_own_line() {
    let (mut t, clock, irqs) = armed(TIM5_SPEC, 0, 9);
    clock.set(20);
    t.tick();
    assert!(raised(&irqs, 50));
}

#[test]
fn a_wrap_with_dier_clear_sets_uif_and_raises_nothing() {
    let (mut t, clock, irqs) = armed(TIM16_SPEC, 0, 9);
    write(&mut t, DIER, 0);
    clock.set(100);
    t.tick();
    assert!(irqs.take().is_none());
    assert_eq!(read(&mut t, SR), UIF);
    // Nothing is raised later either, once the wake asked for before is spent.
    clock.take_due();
    write(&mut t, SR, 0);
    assert_eq!(first_raise(&mut t, &clock, &irqs, 117, u64::MAX), None);
}

#[test]
fn a_read_that_finds_the_wrap_raises_the_line() {
    let (mut t, clock, irqs) = armed(TIM16_SPEC, 0, 9);
    clock.set(100);
    assert_eq!(read(&mut t, SR), UIF);
    assert!(raised(&irqs, 117));
}

#[test]
fn the_wake_is_at_the_next_overflow() {
    // PSC 239, ARR 49999: the board's TIM16 overflows 50,000 microseconds in, which is
    // 24,000,000 core cycles.
    let (_t, clock, _irqs) = armed(TIM16_SPEC, 239, 49_999);
    clock.set(24_000_000 - 1);
    assert!(!clock.take_due());
    clock.set(24_000_000);
    assert!(clock.take_due());
}

#[test]
fn the_tick_asks_for_the_wake_after_that() {
    let (mut t, clock, irqs) = armed(TIM16_SPEC, 239, 49_999);
    clock.set(24_000_000);
    assert!(clock.take_due());
    t.tick();
    assert!(raised(&irqs, 117));
    write(&mut t, SR, 0); // the handler clears the flag
    clock.set(48_000_000 - 1);
    assert!(!clock.take_due());
    clock.set(48_000_000);
    assert!(clock.take_due());
    t.tick();
    assert!(raised(&irqs, 117));
}

#[test]
fn a_32_bit_timer_wraps_at_2_pow_32_and_a_16_bit_one_at_2_pow_16() {
    let (mut t16, clock16, irqs16) = wired(TIM16_SPEC);
    write(&mut t16, DIER, UIE);
    write(&mut t16, CR1, CEN); // ARR is 0xFFFF after the reset
    clock16.set(2 * 0xFFFF);
    t16.tick();
    assert!(irqs16.take().is_none());
    clock16.set(2 * 0x1_0000);
    t16.tick();
    assert!(raised(&irqs16, 117));

    let (mut t32, clock32, irqs32) = wired(TIM5_SPEC);
    write(&mut t32, DIER, UIE);
    write(&mut t32, CR1, CEN);
    clock32.set(2 * 0x1_0000);
    t32.tick();
    assert!(irqs32.take().is_none());
    clock32.set(2 * 0xFFFF_FFFF);
    t32.tick();
    assert!(irqs32.take().is_none());
    clock32.set(2 * 0x1_0000_0000);
    t32.tick();
    assert!(raised(&irqs32, 50));
}

#[test]
fn the_wake_for_a_32_bit_overflow_is_17_9_seconds_out() {
    let (_t, clock, _irqs) = armed(TIM5_SPEC, 0, 0xFFFF_FFFF);
    clock.set(2 * 0x1_0000_0000 - 1);
    assert!(!clock.take_due());
    clock.set(2 * 0x1_0000_0000);
    assert!(clock.take_due());
}

#[test]
fn clearing_uif_drops_the_level_and_nothing_more_is_raised() {
    let (mut t, clock, irqs) = armed(TIM16_SPEC, 0, 9);
    clock.set(20);
    t.tick();
    irqs.take();
    assert!(t.inner().interrupt_active());
    write(&mut t, SR, !UIF); // writing 0 to UIF, as the HAL does
    assert!(!t.inner().interrupt_active());
    assert!(irqs.take().is_none());
    // Writing 1 to it leaves a set flag alone, and the level is still there to re-raise.
    clock.set(40);
    t.tick();
    assert!(raised(&irqs, 117));
    write(&mut t, SR, UIF);
    assert!(raised(&irqs, 117));
}

#[test]
fn setting_uie_with_uif_up_raises_the_line() {
    let (mut t, clock, irqs) = armed(TIM16_SPEC, 0, 9);
    write(&mut t, DIER, 0);
    clock.set(20);
    t.tick();
    assert!(irqs.take().is_none());
    write(&mut t, DIER, UIE);
    assert!(raised(&irqs, 117));
}

#[test]
fn an_arr_write_moves_the_wake() {
    let (mut t, clock, _irqs) = armed(TIM16_SPEC, 0, 999);
    clock.set(100);
    write(&mut t, ARR, 199); // ARPE clear: active at once, 50 ticks in
    clock.set(2 * 200 - 1);
    assert!(!clock.take_due());
    clock.set(2 * 200);
    assert!(clock.take_due());
}

#[test]
fn a_cnt_write_moves_the_wake() {
    let (mut t, clock, _irqs) = armed(TIM16_SPEC, 0, 999);
    clock.set(100);
    write(&mut t, CNT, 990); // 10 ticks short of the wrap, from cycle 100
    clock.set(100 + 2 * 10 - 1);
    assert!(!clock.take_due());
    clock.set(100 + 2 * 10);
    assert!(clock.take_due());
}

#[test]
fn a_psc_write_with_an_update_moves_the_wake() {
    let (mut t, clock, irqs) = armed(TIM16_SPEC, 0, 999);
    write(&mut t, PSC, 9);
    write(&mut t, EGR, UG); // counts from 0 again, ten times slower
    write(&mut t, SR, 0);
    irqs.take(); // the update's own
    assert_eq!(
        first_raise(&mut t, &clock, &irqs, 117, u64::MAX),
        Some(2 * 10 * 1000)
    );
}
#[test]
fn starting_the_timer_schedules_the_wake() {
    let (mut t, clock, _irqs) = wired(TIM16_SPEC);
    write(&mut t, ARR, 9);
    write(&mut t, EGR, UG);
    write(&mut t, SR, 0);
    write(&mut t, DIER, UIE);
    clock.set(1000);
    write(&mut t, CR1, CEN);
    clock.set(1000 + 19);
    assert!(!clock.take_due());
    clock.set(1000 + 20);
    assert!(clock.take_due());
}

#[test]
fn a_psc_write_waits_for_the_next_update() {
    let (mut t, clock) = running(0, 0xFFFF);
    write(&mut t, PSC, 9);
    clock.set(2 * 100);
    assert_eq!(read(&mut t, CNT), 100); // still at the old rate
    assert_eq!(read(&mut t, PSC), 9); // the register itself reads back
    write(&mut t, EGR, UG);
    clock.set(2 * 100 + 2 * 10 * 5);
    assert_eq!(read(&mut t, CNT), 5);
}

#[test]
fn a_psc_write_takes_effect_at_the_next_overflow() {
    let (mut t, clock) = running(0, 9);
    write(&mut t, PSC, 9);
    clock.set(2 * 10); // the wrap, at the old rate
    assert_eq!(read(&mut t, CNT), 0);
    clock.set(2 * 10 + 2 * 10 * 3);
    assert_eq!(read(&mut t, CNT), 3);
}

#[test]
fn an_arr_write_with_arpe_set_waits_for_the_next_update() {
    let (mut t, clock) = running(0, 99);
    write(&mut t, CR1, CEN | ARPE);
    write(&mut t, ARR, 9);
    clock.set(2 * 50);
    assert_eq!(read(&mut t, CNT), 50); // the old period of 100 still holds
    assert_eq!(read(&mut t, SR), 0);
    clock.set(2 * 100);
    assert_eq!(read(&mut t, SR), UIF); // the wrap, which loads the 10
    clock.set(2 * 100 + 2 * 3);
    assert_eq!(read(&mut t, CNT), 3);
    clock.set(2 * 100 + 2 * 10);
    assert_eq!(read(&mut t, CNT), 0); // the second wrap came 10 ticks on
}

#[test]
fn many_wraps_with_a_buffered_arr_use_the_new_period() {
    let (mut t, clock) = running(0, 99);
    write(&mut t, CR1, CEN | ARPE);
    write(&mut t, ARR, 9);
    clock.set(2 * (100 + 10 * 1000 + 4));
    assert_eq!(read(&mut t, CNT), 4);
}

#[test]
fn clearing_arpe_makes_the_buffered_arr_active() {
    let (mut t, clock) = running(0, 99);
    write(&mut t, CR1, CEN | ARPE);
    write(&mut t, ARR, 9);
    write(&mut t, CR1, CEN);
    clock.set(2 * 10);
    assert_eq!(read(&mut t, SR), UIF);
}

#[test]
fn ug_with_urs_set_loads_the_buffers_and_restarts_but_leaves_uif_clear() {
    let (mut t, clock) = running(0, 0xFFFF);
    write(&mut t, CR1, CEN | URS | ARPE);
    write(&mut t, ARR, 9);
    clock.set(100);
    write(&mut t, EGR, UG);
    assert_eq!(read(&mut t, SR), 0);
    assert_eq!(read(&mut t, CNT), 0);
    clock.set(100 + 2 * 10);
    assert_eq!(read(&mut t, SR), UIF); // the loaded ARR of 9 now applies
}

#[test]
fn ug_with_urs_clear_sets_uif_and_raises_the_line() {
    let (mut t, _clock, irqs) = armed(TIM16_SPEC, 0, 0xFFFF);
    write(&mut t, EGR, UG);
    assert_eq!(read(&mut t, SR), UIF);
    assert!(raised(&irqs, 117));
}

#[test]
fn ug_with_urs_set_raises_nothing() {
    let (mut t, _clock, irqs) = armed(TIM16_SPEC, 0, 0xFFFF);
    write(&mut t, CR1, CEN | URS);
    write(&mut t, EGR, UG);
    assert!(irqs.take().is_none());
}

#[test]
fn udis_stops_the_update_flag_and_the_interrupt_but_not_the_counting() {
    let (mut t, clock, irqs) = armed(TIM16_SPEC, 0, 9);
    write(&mut t, CR1, CEN | UDIS);
    clock.set(2 * 25);
    t.tick();
    assert_eq!(read(&mut t, SR), 0);
    assert!(irqs.take().is_none());
    assert_eq!(read(&mut t, CNT), 5);
    assert_eq!(first_raise(&mut t, &clock, &irqs, 117, u64::MAX), None);
}
#[test]
fn udis_keeps_the_buffers_from_loading_and_ug_only_restarts_the_count() {
    let (mut t, clock) = running(0, 0xFFFF);
    write(&mut t, CR1, CEN | UDIS | ARPE);
    write(&mut t, ARR, 9);
    clock.set(100);
    write(&mut t, EGR, UG);
    assert_eq!(read(&mut t, SR), 0);
    assert_eq!(read(&mut t, CNT), 0);
    clock.set(100 + 2 * 20);
    assert_eq!(read(&mut t, CNT), 20); // the old period of 65536 is still the one in use
}

#[test]
fn one_pulse_mode_raises_the_interrupt_then_stops_and_schedules_nothing() {
    let (mut t, clock, irqs) = armed(TIM16_SPEC, 0, 9);
    write(&mut t, CR1, CEN | OPM);
    clock.set(20);
    assert!(clock.take_due());
    t.tick();
    assert!(raised(&irqs, 117));
    assert_eq!(read(&mut t, CR1) & CEN, 0);
    write(&mut t, SR, 0);
    clock.set(10_000);
    assert!(!clock.take_due());
}

#[test]
fn tim5_counts_down_from_arr_and_underflows_to_it() {
    let (mut t, clock, irqs) = armed(TIM5_SPEC, 0, 9);
    write(&mut t, CR1, CEN | DIR);
    write(&mut t, CNT, 9);
    clock.set(2 * 3);
    assert_eq!(read(&mut t, CNT), 6);
    clock.set(2 * 9);
    assert_eq!(read(&mut t, CNT), 0);
    assert_eq!(read(&mut t, SR), 0);
    clock.set(2 * 10);
    assert_eq!(read(&mut t, CNT), 9);
    assert_eq!(read(&mut t, SR), UIF);
    assert!(raised(&irqs, 50));
}

#[test]
fn the_down_counting_wake_is_at_the_underflow() {
    let (mut t, clock, irqs) = armed(TIM5_SPEC, 0, 999);
    write(&mut t, CR1, CEN | DIR);
    write(&mut t, CNT, 4); // underflows after 5 ticks
    assert_eq!(
        first_raise(&mut t, &clock, &irqs, 50, u64::MAX),
        Some(2 * 5)
    );
}
#[test]
fn many_down_wraps_leave_the_remainder() {
    let (mut t, clock, _irqs) = armed(TIM5_SPEC, 0, 99);
    write(&mut t, CR1, CEN | DIR);
    write(&mut t, CNT, 99);
    clock.set(2 * (1_000_000 + 30));
    assert_eq!(read(&mut t, CNT), 69);
}

#[test]
fn tim16_has_no_direction_bit() {
    let (mut t, clock, _irqs) = armed(TIM16_SPEC, 0, 0xFFFF);
    write(&mut t, CR1, CEN | DIR);
    assert_eq!(read(&mut t, CR1), CEN);
    clock.set(20);
    assert_eq!(read(&mut t, CNT), 10);
}

#[test]
fn reset_clears_the_enable_and_the_level() {
    let (mut t, clock, irqs) = armed(TIM16_SPEC, 0, 9);
    clock.set(20);
    t.tick();
    irqs.take();
    t.reset(ResetCause::Software);
    assert!(!t.inner().interrupt_active());
    assert_eq!(read(&mut t, DIER), 0);
    clock.set(10_000);
    t.tick();
    assert!(irqs.take().is_none());
}

#[test]
fn a_timer_with_uie_clear_asks_for_no_wake() {
    // Not from a state where an earlier wake or a set UIF could be the reason.
    let (mut t, clock, _irqs) = wired(TIM16_SPEC);
    write(&mut t, ARR, 9);
    write(&mut t, EGR, UG);
    write(&mut t, SR, 0);
    write(&mut t, CR1, CEN);
    assert_eq!(clock.next_wake(), None);
    write(&mut t, DIER, UIE);
    assert_eq!(clock.next_wake(), Some(20));
}

#[test]
fn a_timer_with_uif_already_up_asks_for_no_wake() {
    let (mut t, clock, irqs) = armed(TIM16_SPEC, 0, 9);
    clock.set(20);
    assert!(clock.take_due());
    t.tick();
    assert!(raised(&irqs, 117));
    // The level is up and waiting for the handler: another overflow would add nothing.
    assert_eq!(clock.next_wake(), None);
    write(&mut t, SR, !UIF);
    assert_eq!(clock.next_wake(), Some(40));
}

#[test]
fn udis_keeps_the_buffers_from_loading_on_an_overflow_too() {
    let (mut t, clock) = running(0, 99);
    write(&mut t, CR1, CEN | UDIS | ARPE);
    write(&mut t, ARR, 9);
    write(&mut t, PSC, 9);
    clock.set(2 * 100 + 2 * 25); // past the wrap, 25 ticks on at the old rate
    assert_eq!(read(&mut t, CNT), 25);
    assert_eq!(read(&mut t, SR), 0);
}

#[test]
fn reserved_bits_read_back_as_zero() {
    let (mut t16, _) = timer(TIM16, 16);
    write(&mut t16, PSC, 0x1_2345);
    assert_eq!(read(&mut t16, PSC), 0x2345);
    write(&mut t16, ARR, 0xFFFF_FFFF);
    assert_eq!(read(&mut t16, ARR), 0xFFFF);
    write(&mut t16, DIER, 0xFFFF_FFFF);
    assert_eq!(read(&mut t16, DIER), 0x03A3);
    let (mut t5, _) = timer(TIM5, 32);
    write(&mut t5, PSC, 0xFFFF_FFFF);
    assert_eq!(read(&mut t5, PSC), 0xFFFF);
    write(&mut t5, ARR, 0xFFFF_FFFF);
    assert_eq!(read(&mut t5, ARR), 0xFFFF_FFFF);
}
