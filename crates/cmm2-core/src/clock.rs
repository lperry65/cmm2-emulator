use std::cell::Cell;
use std::rc::Rc;

/// The machine's time, for the peripherals that count: the core's cycle count, which the CPU
/// publishes before each instruction. A cheap handle: clones share the value.
///
/// The count is an estimate, about 2.2 cycles per instruction on this firmware, not cycle
/// exact. It is 0 until a CPU is attached.
#[derive(Clone, Debug, Default)]
pub struct Clock(Rc<Cell<u64>>);

impl Clock {
    pub fn new() -> Clock {
        Clock::default()
    }

    pub fn now(&self) -> u64 {
        self.0.get()
    }

    pub(crate) fn set(&self, cycles: u64) {
        self.0.set(cycles);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
