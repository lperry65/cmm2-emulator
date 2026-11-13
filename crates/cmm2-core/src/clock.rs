use std::cell::Cell;
use std::rc::Rc;

/// The core clock, in Hz. The machine counts its time in cycles of it.
pub const CORE_HZ: u64 = 480_000_000;

/// The machine's time, for the peripherals that count: the core's cycle count, which the CPU
/// publishes before each instruction. A cheap handle: clones share the value.
///
/// The count is an estimate, about 2.2 cycles per instruction on this firmware, not cycle
/// exact. It is 0 until a CPU is attached.
///
/// A peripheral that has to act while nothing is touching it (to raise an interrupt) asks to
/// be woken at a cycle. The CPU checks before each instruction whether that cycle has come.
#[derive(Clone, Debug, Default)]
pub struct Clock {
    cycles: Rc<Cell<u64>>,
    wake: Rc<Cell<Option<u64>>>,
}

impl Clock {
    pub fn new() -> Clock {
        Clock::default()
    }

    pub fn now(&self) -> u64 {
        self.cycles.get()
    }

    pub(crate) fn set(&self, cycles: u64) {
        self.cycles.set(cycles);
    }

    /// Asks for a tick at `cycle`, or at the earliest one asked for so far.
    pub fn wake_at(&self, cycle: u64) {
        let earliest = self.wake.get().map_or(cycle, |wake| wake.min(cycle));
        self.wake.set(Some(earliest));
    }

    /// Whether a wake has come due, which it then no longer is. Every device that wants to be
    /// woken again asks again when it is ticked.
    pub(crate) fn take_due(&self) -> bool {
        let due = self.wake.get().is_some_and(|wake| wake <= self.now());
        if due {
            self.wake.set(None);
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_due_until_a_wake_is_asked_for() {
        let clock = Clock::new();
        clock.set(1_000);
        assert!(!clock.take_due());
    }

    #[test]
    fn a_wake_is_due_once_its_cycle_comes() {
        let clock = Clock::new();
        clock.wake_at(100);
        clock.set(99);
        assert!(!clock.take_due());
        clock.set(100);
        assert!(clock.take_due());
    }

    #[test]
    fn a_due_wake_is_taken_once() {
        let clock = Clock::new();
        clock.wake_at(5);
        clock.set(6);
        assert!(clock.take_due());
        assert!(!clock.take_due());
    }

    #[test]
    fn the_earliest_wake_wins() {
        let clock = Clock::new();
        clock.wake_at(200);
        clock.wake_at(100);
        clock.wake_at(300);
        clock.set(100);
        assert!(clock.take_due());
        // The later ones were folded into it: devices ask again from their tick.
        clock.set(300);
        assert!(!clock.take_due());
    }

    #[test]
    fn clones_share_the_wake() {
        let clock = Clock::new();
        let other = clock.clone();
        other.wake_at(10);
        clock.set(10);
        assert!(clock.take_due());
    }

    #[test]
    fn a_new_clock_reads_zero() {
        assert_eq!(Clock::new().now(), 0);
    }

    #[test]
    fn set_then_now() {
        let clock = Clock::new();
        clock.set(12345);
        assert_eq!(clock.now(), 12345);
    }

    #[test]
    fn clones_share_the_value() {
        let clock = Clock::new();
        let other = clock.clone();
        other.set(7);
        assert_eq!(clock.now(), 7);
    }
}
