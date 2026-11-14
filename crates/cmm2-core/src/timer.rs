//! A general-purpose counting timer, as TIM16 (16 bits) and TIM5 (32 bits) use it. The
//! firmware waits on the counters, so they count against the machine's clock.
//!
//! Only up-counting is modelled, and the model is instantaneous: the counter is brought up
//! to date from the clock whenever a register is accessed, so nothing runs in between.
//! DIER.UIE is stored but no interrupt is ever raised; timer interrupts need an event
//! scheduler, which is a separate piece of work. The capture/compare registers, the other
//! flags and the other counting modes are plain stored registers.

use crate::clock::Clock;
use crate::peripheral::{ResetCause, StubRegisters};
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

    // A system reset stops the timer and puts every register back, count included.
    fn reset(&mut self, cause: ResetCause) {
        self.stored.reset(cause);
        self.cr1 = self.stored.read(CR1);
        self.sr = self.stored.read(SR);
        let psc = self.stored.read(PSC);
        self.set_psc(psc);
        // The SVD gives ARR a reset value of 0, but the chip resets it to all ones, and a
        // timer that is enabled without setting ARR counts to the end of its width.
        self.set_arr(self.width_mask);
        self.restart(0);
    }
}

pub type Timer = WordAccess<TimerRegisters>;

impl Timer {
    /// `base` is the address of the timer's window and `width` the width of its counter and
    /// reload register, 16 or 32.
    pub fn new(base: u32, width: u32, clock: Clock) -> Timer {
        let width_mask = (u64::MAX >> (64 - width)) as u32;
        let mut regs = TimerRegisters {
            stored: StubRegisters::with_reset_values(base, Svd::stm32h743()),
            clock,
            width_mask,
            cr1: 0,
            sr: 0,
            psc: 0,
            arr: 0,
            divisor: 0,
            period: 0,
            count: 0,
            anchor: 0,
        };
        regs.reset(ResetCause::PowerOn);
        WordAccess::wrap(regs)
    }
}

#[cfg(test)]
mod tests;
