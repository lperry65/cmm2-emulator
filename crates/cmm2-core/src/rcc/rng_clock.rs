//! What the RCC gives the RNG: whether its clocks run, whether it is held in reset, and the
//! rate of its kernel clock. A cheap handle, as `PixelClock` is: clones share the state, so the
//! RCC keeps one and the RNG another. Each change is logged with the core cycle it was made at,
//! for the RNG to take, so that it works out what happened in between however the firmware
//! switched things, and a change asks the clock for a tick at once.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::clock::Clock;

/// The RC48 oscillator, which RNGSEL picks after a reset (RM0433 8.7.5, RCC_D2CCIP2R).
const HSI48_HZ: u64 = 48_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RngClockState {
    /// RCC_AHB2ENR.RNGEN: the kernel clock and the bus clock are both on (RM0433 8.7.42).
    pub(crate) enabled: bool,
    /// RCC_AHB2RSTR.RNGRST is set.
    pub(crate) in_reset: bool,
    /// The rate of rng_ker_ck, as RNGSEL picks it.
    pub(crate) kernel_hz: u64,
}

impl RngClockState {
    /// What a reset leaves: the clock off, and RNGSEL at 0, which is hsi48.
    pub(crate) const RESET: RngClockState = RngClockState {
        enabled: false,
        in_reset: false,
        kernel_hz: HSI48_HZ,
    };
}

#[derive(Clone, Debug)]
pub(crate) struct RngClock(Rc<RngClockShared>);

#[derive(Debug)]
struct RngClockShared {
    clock: Clock,
    state: Cell<RngClockState>,
    changes: RefCell<Vec<(u64, RngClockState)>>,
}

impl RngClock {
    pub(crate) fn new(clock: Clock) -> RngClock {
        RngClock(Rc::new(RngClockShared {
            clock,
            state: Cell::new(RngClockState::RESET),
            changes: RefCell::new(Vec::new()),
        }))
    }

    /// The changes since the last call, oldest first, each as the core cycle and the new state.
    pub(crate) fn take_changes(&self) -> Vec<(u64, RngClockState)> {
        self.0.changes.take()
    }

    pub(crate) fn set(&self, state: RngClockState) {
        if state != self.0.state.get() {
            let now = self.0.clock.now();
            self.0.state.set(state);
            self.0.changes.borrow_mut().push((now, state));
            self.0.clock.wake_at(now);
        }
    }
}
