//! The true random number generator (RNG), which the firmware's boot code enables and polls
//! for a first value (docs/rng.md). The model follows RM0433 chapter 34: the three registers,
//! DRDY and the error flags, the four-word output buffer, the time to a value in RNG kernel
//! clocks, the clock gate and kernel clock the RCC gives it, and the interrupt.
//!
//! The values come from a seeded PRNG in place of the noise source. The seed is taken from the
//! host unless the caller gives one. Time is the machine's clock: work on the next four words
//! is counted in core cycles, and only while the RNG is enabled and its clocks run. The model
//! is brought up to date whenever a register is touched or the clock wakes it.
//!
//! The interrupt line is a level on the real chip, but `Irqs` is edge like, so the line is
//! raised when the level becomes active, and when a read of the data register or a flag clear
//! leaves it active.

use crate::clock::{CORE_HZ, Clock};
use crate::irq::Irqs;
use crate::peripheral::ResetCause;
use crate::rcc::{RngClock, RngClockState};
use crate::registers::{Registers, WordAccess};

mod prng;
use prng::Prng;

pub const RNG_BASE: u32 = 0x4802_1800;
pub const RNG_SIZE: u32 = 0x400;

/// `HASH_RNG_IRQn`, shared with the HASH (RM0433 Table 144).
const RNG_IRQ: u16 = 80;

const CR: u32 = 0x00;
const SR: u32 = 0x04;
const DR: u32 = 0x08;

// RNG_CR: the bits that exist (RM0433 34.7.1).
const CED: u32 = 1 << 5;
const IE: u32 = 1 << 3;
const RNGEN: u32 = 1 << 2;
const CR_BITS: u32 = CED | IE | RNGEN;
// RNG_SR (34.7.2).
const SEIS: u32 = 1 << 6;
const CEIS: u32 = 1 << 5;
const SECS: u32 = 1 << 2;
const CECS: u32 = 1 << 1;
const DRDY: u32 = 1;
// The two flags that a write of 0 clears; the others are read-only.
const CLEARABLE: u32 = SEIS | CEIS;

/// The AHB clock the board runs the RNG's bus interface and conditioning at: half the core
/// clock, as for the timers.
const AHB_HZ: u64 = CORE_HZ / 2;
/// Words the output buffer gives before it is empty (34.3.3).
const WORDS: u8 = 4;
/// Words to read and discard to recover from a seed error (34.3.7).
const RECOVERY_WORDS: u8 = 12;

/// The time to the next four words, in core cycles, for a kernel clock of `hz` (RM0433 34.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Work {
    /// The first set after the RNG is enabled: 192 RNG clocks and 213 AHB clocks, or, with an
    /// AHB clock under (213 x fRNG) / 64, 128 RNG clocks and 426 AHB clocks.
    Init,
    /// The next set: 16 x fAHB / fRNG AHB clocks, but at least 213.
    Refill,
}

impl Work {
    fn cycles(self, hz: u64) -> u64 {
        let rng = |n: u64| (n * CORE_HZ).div_ceil(hz);
        let ahb = |n: u64| n * (CORE_HZ / AHB_HZ);
        match self {
            Work::Init if AHB_HZ * 64 >= 213 * hz => rng(192) + ahb(213),
            Work::Init => rng(128) + ahb(426),
            Work::Refill => rng(16).max(ahb(213)),
        }
    }
}

#[derive(Debug)]
pub struct RngRegisters {
    clock: Clock,
    rcc: RngClock,
    irqs: Irqs,
    prng: Prng,
    state: RngClockState,
    cr: u32,
    /// SEIS, CEIS, SECS and CECS. DRDY is worked out from the buffer.
    flags: u32,
    /// Words left in the output buffer.
    words: u8,
    /// The work on the next four words, and the core cycles done of it.
    work: Option<(Work, u64)>,
    /// The first four words have been made since a reset.
    started: bool,
    /// Words still to read and discard before SECS clears, after a seed error is cleared
    /// (34.3.7).
    recovery: u8,
    /// The cycle the model has been brought up to.
    anchor: u64,
}

impl RngRegisters {
    fn running(&self) -> bool {
        self.cr & RNGEN != 0 && self.bus_clock()
    }

    /// The registers answer only with the clocks on and the block out of reset.
    fn bus_clock(&self) -> bool {
        self.state.enabled && !self.state.in_reset
    }

    /// The interrupt level: an enabled interrupt with a flag to report, with the RNG enabled.
    fn interrupt_active(&self) -> bool {
        self.running() && self.cr & IE != 0 && (self.words > 0 || self.flags & CLEARABLE != 0)
    }

    /// The clock error check: with CED clear, a kernel clock under a 32nd of the AHB clock
    /// sets CECS, and CEIS with it when CECS rises; CECS follows the condition and CEIS stays
    /// until it is cleared (34.3.7). It cannot happen on the board's clocks; the check is for
    /// the kernel clock choices that can (LSE and LSI).
    fn check_clock(&mut self) {
        let too_slow = self.cr & CED == 0 && self.state.kernel_hz * 32 < AHB_HZ;
        if self.running() && too_slow {
            if self.flags & CECS == 0 {
                self.flags |= CEIS;
            }
            self.flags |= CECS;
        } else {
            self.flags &= !CECS;
        }
    }

    /// A seed error: the model never has one, so only the tests ask for it. It sets SEIS and
    /// SECS and stops the generator until SEIS is cleared.
    #[cfg(test)]
    fn seed_error(&mut self) {
        let was_active = self.interrupt_active();
        self.flags |= SEIS | SECS;
        self.raise(was_active, false);
    }

    /// Brings the work up to `to`, a cycle at or after the anchor.
    fn advance(&mut self, to: u64) {
        if self.running() {
            if self.words == 0 && self.work.is_none() {
                self.work = Some((
                    if self.started {
                        Work::Refill
                    } else {
                        Work::Init
                    },
                    0,
                ));
            }
            // A seed error stops the generator until SEIS is cleared (34.3.7).
            if self.flags & SEIS != 0 {
                self.anchor = to;
                return;
            }
            if let Some((work, done)) = &mut self.work {
                *done += to - self.anchor;
                if *done >= work.cycles(self.state.kernel_hz) {
                    self.work = None;
                    self.words = WORDS;
                    self.started = true;
                }
            }
            self.check_clock();
        }
        self.anchor = to;
    }

    /// Takes in what the RCC has done to the clocks since the last time, and the time passed.
    fn sync(&mut self) {
        for (at, state) in self.rcc.take_changes() {
            self.advance(at);
            self.state = state;
            if state.in_reset {
                self.reset_registers();
            }
            self.check_clock();
        }
        self.advance(self.clock.now());
    }

    /// Asks for a tick when the work will be done, if an interrupt is waiting for it.
    fn schedule(&mut self) {
        if let Some((work, done)) = self.work
            && self.running()
            && self.cr & IE != 0
        {
            let left = work.cycles(self.state.kernel_hz).saturating_sub(done);
            self.clock.wake_at(self.anchor + left);
        }
    }

    /// Raises the line if the level has become active, or if `still` and it is.
    fn raise(&self, was_active: bool, still: bool) {
        let active = self.interrupt_active();
        if active && (!was_active || still) {
            self.irqs.raise(RNG_IRQ);
        }
    }

    fn reset_registers(&mut self) {
        self.cr = 0;
        self.flags = 0;
        self.words = 0;
        self.work = None;
        self.started = false;
        self.recovery = 0;
    }

    /// What a read of DR gives: the next word, and with the fourth the buffer is empty, so
    /// DRDY drops until the next four are made. With no word it gives 0 (34.7.3).
    fn take_word(&mut self) -> u32 {
        if self.words == 0 {
            return 0;
        }
        self.words -= 1;
        if self.words == 0 {
            self.work = Some((Work::Refill, 0));
        }
        // After a seed error is cleared, 12 words are read and thrown away, then SECS clears.
        if self.recovery > 0 {
            self.recovery -= 1;
            if self.recovery == 0 {
                self.flags &= !SECS;
            }
        }
        self.prng.next_u32()
    }
}

impl Registers for RngRegisters {
    fn read(&mut self, offset: u32) -> u32 {
        // With the bus clock off a read returns invalid data and does nothing (RM0433 8.5.11,
        // p. 373); the model returns 0.
        let was_active = self.interrupt_active();
        self.sync();
        if !self.bus_clock() {
            return 0;
        }
        let value = match offset {
            CR => self.cr,
            SR => self.flags | if self.words > 0 { DRDY } else { 0 },
            DR => self.take_word(),
            _ => 0,
        };
        if offset == DR {
            self.raise(was_active, true);
        }
        self.schedule();
        value
    }

    fn write(&mut self, offset: u32, value: u32, mask: u32) {
        // With the bus clock off a write is ignored, with no bus error (8.5.11, p. 373).
        let was_active = self.interrupt_active();
        self.sync();
        if !self.bus_clock() {
            return;
        }
        match offset {
            CR => {
                let mut new = self.cr & !mask | value & mask & CR_BITS;
                // CED cannot change with the RNG enabled (34.7.1).
                if self.cr & RNGEN != 0 {
                    new = new & !CED | self.cr & CED;
                }
                self.cr = new;
                self.check_clock();
            }
            // SEIS and CEIS clear by writing 0; writing 1 does nothing (34.7.2).
            SR => {
                let had_seed_error = self.flags & SEIS != 0;
                self.flags &= !(CLEARABLE & !value & mask);
                if had_seed_error && self.flags & SEIS == 0 {
                    self.recovery = RECOVERY_WORDS;
                }
            }
            _ => {}
        }
        self.raise(was_active, offset == SR);
        self.schedule();
    }

    fn reset(&mut self, _cause: ResetCause) {
        self.reset_registers();
        self.state = RngClockState::RESET;
        self.rcc.take_changes();
        self.anchor = self.clock.now();
    }

    fn tick(&mut self) {
        let was_active = self.interrupt_active();
        self.sync();
        self.raise(was_active, false);
        self.schedule();
    }
}

pub type Rng = WordAccess<RngRegisters>;

impl Rng {
    /// `seed` fixes the values, for a run that is to repeat; without it they come from the
    /// host's entropy and differ from run to run.
    pub(crate) fn new(clock: Clock, rcc: RngClock, irqs: Irqs, seed: Option<u64>) -> Rng {
        WordAccess::wrap(RngRegisters {
            anchor: clock.now(),
            clock,
            state: RngClockState::RESET,
            rcc,
            irqs,
            prng: Prng::new(seed.unwrap_or_else(Prng::host_seed)),
            cr: 0,
            flags: 0,
            words: 0,
            work: None,
            started: false,
            recovery: 0,
        })
    }
}

#[cfg(test)]
mod tests;
