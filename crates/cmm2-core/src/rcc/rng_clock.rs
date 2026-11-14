//! What the RCC gives the RNG: whether its clocks run, whether it is held in reset, and the
//! rate of its kernel clock. It wraps a `Signal`, as the pixel clock does.

use super::signal::Signal;
use crate::clock::Clock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RngClockState {
    /// RCC_AHB2ENR.RNGEN: the kernel clock and the bus clock are both on (RM0433 8.7.42).
    pub(crate) enabled: bool,
    /// RCC_AHB2RSTR.RNGRST is set.
    pub(crate) in_reset: bool,
    /// The rate of rng_ker_ck as RNGSEL picks it, or 0 when the oscillator it picks is off.
    pub(crate) kernel_hz: u64,
}

impl RngClockState {
    /// What a reset leaves: the clock off, and RNGSEL at 0, hsi48, which is off too.
    pub(crate) const RESET: RngClockState = RngClockState {
        enabled: false,
        in_reset: false,
        kernel_hz: 0,
    };
}

#[derive(Clone, Debug)]
pub(crate) struct RngClock(Signal<RngClockState>);

impl RngClock {
    pub(crate) fn new(clock: Clock, state: RngClockState) -> RngClock {
        RngClock(Signal::new(clock, state))
    }

    #[cfg(test)]
    pub(crate) fn get(&self) -> RngClockState {
        self.0.get()
    }

    /// The changes since the last call, oldest first, each as the core cycle and the new state.
    pub(crate) fn take_changes(&self) -> Vec<(u64, RngClockState)> {
        self.0.take_changes()
    }

    pub(crate) fn set(&self, state: RngClockState) {
        self.0.set(state);
    }
}
