//! The general-purpose counting timers the machine maps, TIM5 (32 bits) and TIM16 (16 bits),
//! with their update interrupt (docs/timers.md). The firmware builds TIMER from TIM16 and
//! counts the wraps of both in their interrupt handlers, so the interrupt matters.
//!
//! The model is instantaneous: the count is brought up to date from the machine's clock
//! whenever a register is accessed, so nothing runs in between. For the long stretches in
//! which nothing touches a timer, it asks the clock for a wake at the next overflow (the event
//! scheduler), and the wake's tick raises the interrupt. It follows RM0433 chapters 39 and 41:
//! up and down counting, the ARR and PSC buffers, UG, UDIS, URS, OPM and the update flag.
//! Capture/compare, PWM, DMA requests, the slave-mode controller and center-aligned counting
//! are not modelled: those registers are plain stored registers.

use crate::clock::Clock;
use crate::irq::Irqs;
use crate::peripheral::{ResetCause, StubRegisters};
use crate::registers::{Registers, WordAccess};
use crate::svd::Svd;

pub const TIM5_BASE: u32 = 0x4000_0C00;
pub const TIM16_BASE: u32 = 0x4001_4400;
pub const TIMER_SIZE: u32 = 0x400;

/// What differs between the timers: where they are, how wide they count, the line the update
/// event is on, and whether they count down (TIM2 to TIM5 do; TIM15 to TIM17 only count up).
#[derive(Clone, Copy, Debug)]
pub struct TimerSpec {
    pub base: u32,
    pub width: u32,
    /// The NVIC line of the update event, from the NVIC table in RM0433 (Table 147; the SVD
    /// agrees).
    pub irq: u16,
    pub direction: bool,
}

/// `TIM5_IRQn`, 50, which carries every TIM5 event.
pub const TIM5_SPEC: TimerSpec = TimerSpec {
    base: TIM5_BASE,
    width: 32,
    irq: 50,
    direction: true,
};

/// `TIM16_IRQn`, 117, which carries every TIM16 event. Unlike some STM32 parts, the H743 gives
/// TIM16 a line of its own: TIM1's update is on TIM1_UP (25), not shared with it.
pub const TIM16_SPEC: TimerSpec = TimerSpec {
    base: TIM16_BASE,
    width: 16,
    irq: 117,
    direction: false,
};

// Register offsets and bit positions, as the SVD gives them.
const CR1: u32 = 0x00;
const DIER: u32 = 0x0C;
const SR: u32 = 0x10;
const EGR: u32 = 0x14;
const CNT: u32 = 0x24;
const PSC: u32 = 0x28;
const ARR: u32 = 0x2C;

const CEN: u32 = 1 << 0;
const UDIS: u32 = 1 << 1;
const URS: u32 = 1 << 2;
const OPM: u32 = 1 << 3;
const DIR: u32 = 1 << 4;
const ARPE: u32 = 1 << 7;
/// The CR1 bits that exist: CEN, UDIS, URS, OPM, ARPE, CKD and UIFREMAP, and for the timers
/// that count down DIR and CMS as well.
const CR1_BITS: u32 = 0x0B8F;
const CR1_DIRECTION_BITS: u32 = 0x0060 | DIR;
/// The DIER bits that exist (41.6.3 and 39.4.4). TIM16: UIE, CC1IE, COMIE, BIE, UDE and CC1DE.
/// TIM2 to TIM5: UIE, CC1IE to CC4IE, TIE, UDE, CC1DE to CC4DE and TDE.
const TIM16_DIER_BITS: u32 = 0x03A3;
const TIM5_DIER_BITS: u32 = 0x5F5F;
const UIF: u32 = 1 << 0;
const UIE: u32 = 1 << 0;
const UG: u32 = 1 << 0;

/// Core cycles for one step of the prescaler. This assumes the core runs at 480 MHz and the
/// timer kernel clock is half of it (240 MHz), as on the board.
const TIMER_CYCLES_PER_PRESCALER_STEP: u64 = 2;

/// The registers of one timer. The ones the model works with are plain fields, because the
/// firmware reads the count in a tight loop; every other register is stored.
///
/// The count is the value it had at `anchor`, a core cycle. `arr` and `psc` are the registers
/// the software writes (the preload); `period` (ARR + 1) and `divisor` are the active values
/// the count runs on. The chip loads the active ones from the registers at an update event: PSC
/// always, ARR when ARPE is set (with ARPE clear a write to ARR is active at once).
#[derive(Debug)]
pub struct TimerRegisters {
    stored: StubRegisters,
    clock: Clock,
    spec: TimerSpec,
    irqs: Option<Irqs>,
    width_mask: u32,
    cr1: u32,
    dier: u32,
    sr: u32,
    psc: u32,
    arr: u32,
    divisor: u64,
    period: u64,
    count: u32,
    anchor: u64,
}

impl TimerRegisters {
    fn counts_down(&self) -> bool {
        self.spec.direction && self.cr1 & DIR != 0
    }

    /// The count after an overflow or underflow: 0 up, the reload value down.
    fn reload_count(&self) -> u32 {
        if self.counts_down() {
            (self.period - 1) as u32
        } else {
            0
        }
    }

    /// Prescaler ticks from the count to the next update event. A count already past the
    /// reload wraps at once (the chip would run on to the end of the width; not modelled).
    fn ticks_to_overflow(&self) -> u64 {
        if self.counts_down() {
            u64::from(self.count) + 1
        } else {
            self.period.saturating_sub(u64::from(self.count))
        }
    }

    /// The update event loads the buffers: ARR into the active reload and PSC into the
    /// prescaler (RM0433 39.3.2, 41.4.2).
    fn load_buffers(&mut self) {
        self.period = u64::from(self.arr & self.width_mask) + 1;
        self.divisor = TIMER_CYCLES_PER_PRESCALER_STEP * (u64::from(self.psc & 0xFFFF) + 1);
    }

    /// Moves the count on to the current cycle. An overflow (or underflow) is an update event:
    /// unless UDIS is set it loads the buffers and sets UIF, and in one-pulse mode it stops the
    /// timer. With UDIS set the count still wraps but nothing else happens. Returns whether
    /// there was one.
    fn catch_up(&mut self) -> bool {
        let mut wrapped = false;
        while self.cr1 & CEN != 0 {
            let now = self.clock.now();
            let need = self.ticks_to_overflow() * self.divisor;
            let available = now.saturating_sub(self.anchor);
            if available < need {
                let ticks = available / self.divisor;
                self.anchor += ticks * self.divisor;
                self.count = if self.counts_down() {
                    self.count - ticks as u32
                } else {
                    self.count + ticks as u32
                };
                break;
            }
            self.anchor += need;
            wrapped = true;
            if self.cr1 & UDIS == 0 {
                self.load_buffers();
                self.sr |= UIF;
            }
            self.count = self.reload_count();
            if self.cr1 & UDIS == 0 && self.cr1 & OPM != 0 {
                self.cr1 &= !CEN;
                break;
            }
            // Whole periods on the same buffers repeat exactly: skip them, then the loop
            // finishes the part period.
            let whole = self.period * self.divisor;
            self.anchor += (now.saturating_sub(self.anchor) / whole) * whole;
        }
        wrapped
    }

    fn restart(&mut self, count: u32) {
        self.count = count;
        self.anchor = self.clock.now();
    }

    /// UG: the counter and the prescaler start again. Unless UDIS is set it is an update event
    /// too, which loads the buffers and, unless URS is set, sets UIF (39.3.2).
    fn update_generation(&mut self) {
        if self.cr1 & UDIS == 0 {
            self.load_buffers();
            if self.cr1 & URS == 0 {
                self.sr |= UIF;
            }
        }
        let count = self.reload_count();
        self.restart(count);
    }

    /// The level of the interrupt: the update flag with its enable.
    fn interrupt_active(&self) -> bool {
        self.sr & self.dier & UIF != 0
    }

    /// Raises the line if the level has become active, or if `still` and it is.
    fn raise(&self, was_active: bool, still: bool) {
        if let Some(irqs) = &self.irqs
            && self.interrupt_active()
            && (!was_active || still)
        {
            irqs.raise(self.spec.irq);
        }
    }

    /// Asks the clock for a wake at the next overflow, if one would raise the interrupt: the
    /// timer is running, UIE is set, UDIS is clear and UIF is not already up.
    fn schedule(&self) {
        if self.cr1 & CEN != 0 && self.cr1 & UDIS == 0 && self.dier & UIE != 0 && self.sr & UIF == 0
        {
            self.clock
                .wake_at(self.anchor + self.ticks_to_overflow() * self.divisor);
        }
    }
}

impl Registers for TimerRegisters {
    fn read(&mut self, offset: u32) -> u32 {
        let was_active = self.interrupt_active();
        if self.catch_up() {
            self.raise(was_active, false);
            self.schedule();
        }
        match offset {
            CNT => self.count & self.width_mask,
            CR1 => self.cr1,
            DIER => self.dier,
            SR => self.sr,
            PSC => self.psc,
            ARR => self.arr,
            EGR => 0,
            _ => self.stored.read(offset),
        }
    }

    fn write(&mut self, offset: u32, value: u32, mask: u32) {
        let was_active = self.interrupt_active();
        self.catch_up();
        match offset {
            CR1 => {
                let was_running = self.cr1 & CEN != 0;
                let bits = if self.spec.direction {
                    CR1_BITS | CR1_DIRECTION_BITS
                } else {
                    CR1_BITS
                };
                self.cr1 = self.cr1 & !mask | value & mask & bits;
                if !was_running && self.cr1 & CEN != 0 {
                    self.anchor = self.clock.now();
                }
                // With ARPE clear the register is not buffered: its value is the active one.
                if self.cr1 & ARPE == 0 {
                    self.period = u64::from(self.arr & self.width_mask) + 1;
                }
            }
            DIER => {
                let bits = if self.spec.direction {
                    TIM5_DIER_BITS
                } else {
                    TIM16_DIER_BITS
                };
                self.dier = self.dier & !mask | value & mask & bits;
            }
            CNT => self.restart((self.count & !mask | value & mask) & self.width_mask),
            SR => self.sr &= value | !mask,
            PSC => self.psc = self.psc & !mask | value & mask & 0xFFFF,
            ARR => {
                self.arr = (self.arr & !mask | value & mask) & self.width_mask;
                if self.cr1 & ARPE == 0 {
                    self.period = u64::from(self.arr & self.width_mask) + 1;
                }
            }
            EGR => {
                if value & mask & UG != 0 {
                    self.update_generation();
                }
            }
            _ => self.stored.write(offset, value, mask),
        }
        self.raise(was_active, offset == SR);
        self.schedule();
    }

    // A system reset stops the timer and puts every register back, count included.
    fn reset(&mut self, cause: ResetCause) {
        self.stored.reset(cause);
        self.cr1 = 0;
        self.dier = 0;
        self.sr = 0;
        self.psc = 0;
        // The SVD gives ARR a reset value of 0, but the chip resets it to all ones, and a
        // timer that is enabled without setting ARR counts to the end of its width.
        self.arr = self.width_mask;
        self.load_buffers();
        self.restart(0);
    }

    // The clock's wake: an overflow nothing has looked at yet.
    fn tick(&mut self) {
        let was_active = self.interrupt_active();
        self.catch_up();
        self.raise(was_active, false);
        self.schedule();
    }
}

pub type Timer = WordAccess<TimerRegisters>;

impl Timer {
    /// A timer of `width` bits (16 or 32) at `base` that counts up and has no interrupt line.
    pub fn new(base: u32, width: u32, clock: Clock) -> Timer {
        Timer::build(
            TimerSpec {
                base,
                width,
                irq: 0, // unused: there is no Irqs
                direction: false,
            },
            clock,
            None,
        )
    }

    /// A timer as `spec` says, which raises its update interrupt on `irqs`.
    pub fn with_interrupt(spec: TimerSpec, clock: Clock, irqs: Irqs) -> Timer {
        Timer::build(spec, clock, Some(irqs))
    }

    fn build(spec: TimerSpec, clock: Clock, irqs: Option<Irqs>) -> Timer {
        let width_mask = (u64::MAX >> (64 - spec.width)) as u32;
        let mut regs = TimerRegisters {
            stored: StubRegisters::with_reset_values(spec.base, Svd::stm32h743()),
            clock,
            spec,
            irqs,
            width_mask,
            cr1: 0,
            dier: 0,
            sr: 0,
            psc: 0,
            arr: 0,
            divisor: 2,
            period: 1,
            count: 0,
            anchor: 0,
        };
        regs.reset(ResetCause::PowerOn);
        WordAccess::wrap(regs)
    }
}

#[cfg(test)]
mod tests;
