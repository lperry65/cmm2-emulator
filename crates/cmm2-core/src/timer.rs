//! A general-purpose counting timer, as TIM16 (16 bits) and TIM5 (32 bits) use it. The
//! firmware waits on the counters, so they count against the machine's clock.
//!
//! Only up-counting is modelled, and the model is instantaneous: the counter is brought up
//! to date from the clock whenever a register is accessed, so nothing runs in between.
//! DIER.UIE is stored but no interrupt is ever raised; timer interrupts need an event
//! scheduler, which is a separate piece of work. The capture/compare registers, the other
//! flags and the other counting modes are plain stored registers.

use crate::clock::Clock;
use crate::peripheral::StubRegisters;
use crate::registers::{Registers, WordAccess};
use crate::svd::Svd;

pub const TIM5_BASE: u32 = 0x4000_0C00;
pub const TIM16_BASE: u32 = 0x4001_4400;
pub const TIMER_SIZE: u32 = 0x400;

// Register offsets and bit positions, as the SVD gives them.
const CR1: u32 = 0x00;
const SR: u32 = 0x10;
const EGR: u32 = 0x14;
const CNT: u32 = 0x24;
const PSC: u32 = 0x28;
const ARR: u32 = 0x2C;

const CEN: u32 = 1 << 0;
const OPM: u32 = 1 << 3;
const UIF: u32 = 1 << 0;
const UG: u32 = 1 << 0;

/// Core cycles for one step of the prescaler. This assumes the core runs at 480 MHz and the
/// timer kernel clock is half of it (240 MHz), as on the board.
const TIMER_CYCLES_PER_PRESCALER_STEP: u64 = 2;

/// The registers of one timer. The ones the model works with are plain fields, because the
/// firmware reads the count in a tight loop; every other register is stored.
///
/// The count is the value it had at `anchor`, a core cycle. The real chip buffers ARR (when
/// ARPE is set) and PSC (always) until the next update event; the model applies a write to
/// either at once, after bringing the count up to date with the old values. UG sets UIF even
/// when URS is set, where the chip would not. `divisor` and `period` are worked out from PSC
/// and ARR when those are written, so that catching up needs no more than one division.
#[derive(Debug)]
pub struct TimerRegisters {
    stored: StubRegisters,
    clock: Clock,
    width_mask: u32,
    cr1: u32,
    sr: u32,
    psc: u32,
    arr: u32,
    divisor: u64,
    period: u64,
    count: u32,
    anchor: u64,
}

impl TimerRegisters {
    /// Moves the count on to the current cycle, with the prescaler and reload as they are.
    /// A wrap, however many there were, sets UIF; in one-pulse mode the first one stops it.
    fn catch_up(&mut self) {
        if self.cr1 & CEN == 0 {
            return;
        }
        let elapsed = self.clock.now().saturating_sub(self.anchor) / self.divisor;
        self.anchor += elapsed * self.divisor;
        let total = u64::from(self.count) + elapsed;
        if total < self.period {
            self.count = total as u32;
            return;
        }
        self.count = (total % self.period) as u32;
        self.sr |= UIF;
        if self.cr1 & OPM != 0 {
            self.count = 0;
            self.cr1 &= !CEN;
        }
    }

    fn restart(&mut self, count: u32) {
        self.count = count;
        self.anchor = self.clock.now();
    }

    fn set_psc(&mut self, psc: u32) {
        self.psc = psc;
        self.divisor = TIMER_CYCLES_PER_PRESCALER_STEP * (u64::from(psc & 0xFFFF) + 1);
    }

    fn set_arr(&mut self, arr: u32) {
        self.arr = arr;
        self.period = u64::from(arr & self.width_mask) + 1;
    }
}

impl Registers for TimerRegisters {
    fn read(&mut self, offset: u32) -> u32 {
        self.catch_up();
        match offset {
            CNT => self.count & self.width_mask,
            CR1 => self.cr1,
            SR => self.sr,
            PSC => self.psc,
            ARR => self.arr,
            EGR => 0,
            _ => self.stored.read(offset),
        }
    }

    fn write(&mut self, offset: u32, value: u32, mask: u32) {
        self.catch_up();
        match offset {
            CR1 => {
                let was_running = self.cr1 & CEN != 0;
                self.cr1 = self.cr1 & !mask | value & mask;
                if !was_running && self.cr1 & CEN != 0 {
                    self.anchor = self.clock.now();
                }
            }
            CNT => self.restart((self.count & !mask | value & mask) & self.width_mask),
            SR => self.sr &= value | !mask,
            PSC => self.set_psc(self.psc & !mask | value & mask),
            ARR => self.set_arr(self.arr & !mask | value & mask),
            EGR => {
                if value & mask & UG != 0 {
                    self.restart(0);
                    self.sr |= UIF;
                }
            }
            _ => self.stored.write(offset, value, mask),
        }
    }
}

pub type Timer = WordAccess<TimerRegisters>;

impl Timer {
    /// `base` is the address of the timer's window and `width` the width of its counter and
    /// reload register, 16 or 32.
    pub fn new(base: u32, width: u32, clock: Clock) -> Timer {
        let width_mask = (u64::MAX >> (64 - width)) as u32;
        let mut stored = StubRegisters::with_reset_values(base, Svd::stm32h743());
        let mut regs = TimerRegisters {
            cr1: stored.read(CR1),
            sr: stored.read(SR),
            psc: 0,
            arr: 0,
            divisor: 0,
            period: 0,
            stored,
            clock,
            width_mask,
            count: 0,
            anchor: 0,
        };
        let psc = regs.stored.read(PSC);
        regs.set_psc(psc);
        // The SVD gives ARR a reset value of 0, but the chip resets it to all ones, and a
        // timer that is enabled without setting ARR counts to the end of its width.
        regs.set_arr(width_mask);
        WordAccess::wrap(regs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Peripheral;

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
}
