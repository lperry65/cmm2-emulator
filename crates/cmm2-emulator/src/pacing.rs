//! Real-time pacing: holds the machine's time (the core's cycle count) to the wall clock when
//! the host is fast enough, and does nothing when it is not.

use std::time::{Duration, Instant};

/// One sleep is never longer than this, so the window and the keys are looked at often enough.
const MAX_SLEEP: Duration = Duration::from_millis(10);

/// Behind by more than this means the host stalled (a window drag, a suspended process). The
/// pacer starts again from now rather than run flat out to catch up.
const MAX_LAG: Duration = Duration::from_millis(100);

/// The wall clock and the sleep, so the pacing can be tested without waiting.
pub trait Time {
    /// The time since the pacer was made.
    fn elapsed(&self) -> Duration;
    fn sleep(&self, duration: Duration);
}

pub struct RealTime(Instant);

impl RealTime {
    pub fn start() -> RealTime {
        RealTime(Instant::now())
    }
}

impl Time for RealTime {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

pub struct Pacer<T> {
    time: T,
    hz: u64,
    /// Machine and wall time are compared from here, which moves up after a stall.
    base_cycles: u64,
    base_elapsed: Duration,
}

impl<T: Time> Pacer<T> {
    pub fn new(time: T, hz: u64) -> Pacer<T> {
        Pacer {
            base_elapsed: time.elapsed(),
            time,
            hz,
            base_cycles: 0,
        }
    }

    /// Sleeps if the machine, at `cycles`, is ahead of the wall clock.
    pub fn pace(&mut self, cycles: u64) {
        let sleep = self.sleep_for(self.time.elapsed(), cycles);
        if !sleep.is_zero() {
            self.time.sleep(sleep);
        }
    }

    fn sleep_for(&mut self, elapsed: Duration, cycles: u64) -> Duration {
        let machine = self.machine_time(cycles);
        let wall = elapsed.saturating_sub(self.base_elapsed);
        if wall.saturating_sub(machine) > MAX_LAG {
            self.base_cycles = cycles;
            self.base_elapsed = elapsed;
        }
        machine.saturating_sub(wall).min(MAX_SLEEP)
    }

    /// The time the machine has run since the base.
    fn machine_time(&self, cycles: u64) -> Duration {
        let cycles = u128::from(cycles.saturating_sub(self.base_cycles));
        Duration::from_nanos((cycles * 1_000_000_000 / u128::from(self.hz)) as u64)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    const HZ: u64 = 1_000_000;

    /// A clock that only moves when it is told, or when it is slept on.
    #[derive(Default)]
    struct Fake {
        now: Cell<Duration>,
        slept: Cell<Duration>,
    }

    impl Time for &Fake {
        fn elapsed(&self) -> Duration {
            self.now.get()
        }

        fn sleep(&self, duration: Duration) {
            self.slept.set(self.slept.get() + duration);
            self.now.set(self.now.get() + duration);
        }
    }

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    // 1 MHz: a cycle is a microsecond, so 1000 cycles are 1 ms of machine time.
    fn pacer(fake: &Fake) -> Pacer<&Fake> {
        Pacer::new(fake, HZ)
    }

    #[test]
    fn ahead_of_the_wall_clock_sleeps_the_difference() {
        let fake = Fake::default();
        let mut pacer = pacer(&fake);
        fake.now.set(ms(2));
        pacer.pace(5_000);
        assert_eq!(fake.slept.get(), ms(3));
    }

    #[test]
    fn on_time_or_behind_does_not_sleep() {
        let fake = Fake::default();
        let mut pacer = pacer(&fake);
        fake.now.set(ms(5));
        pacer.pace(5_000);
        pacer.pace(1_000);
        fake.now.set(ms(50));
        pacer.pace(2_000);
        assert_eq!(fake.slept.get(), Duration::ZERO);
    }

    #[test]
    fn a_single_sleep_is_capped() {
        let fake = Fake::default();
        let mut pacer = pacer(&fake);
        pacer.pace(1_000_000);
        assert_eq!(fake.slept.get(), MAX_SLEEP);
    }

    #[test]
    fn a_stall_is_forgotten_rather_than_caught_up() {
        let fake = Fake::default();
        let mut pacer = pacer(&fake);
        // The host stalls for a second while the machine stands at 2 ms.
        fake.now.set(ms(1_000));
        pacer.pace(2_000);
        assert_eq!(fake.slept.get(), Duration::ZERO);
        // From here the machine is paced against the new start, not run flat out: 3 ms later
        // on the machine's clock but only 1 ms later on the wall's, it sleeps 2 ms.
        fake.now.set(ms(1_001));
        pacer.pace(5_000);
        assert_eq!(fake.slept.get(), ms(2));
    }

    #[test]
    fn a_lag_within_the_limit_is_caught_up() {
        let fake = Fake::default();
        let mut pacer = pacer(&fake);
        fake.now.set(ms(100));
        pacer.pace(1_000);
        assert_eq!(fake.slept.get(), Duration::ZERO);
        // Still 99 ms behind, so the machine's next 99 ms are not slept on.
        fake.now.set(ms(101));
        pacer.pace(100_000);
        assert_eq!(fake.slept.get(), Duration::ZERO);
        // One more ms puts it 1 ms ahead.
        pacer.pace(102_000);
        assert_eq!(fake.slept.get(), ms(1));
    }

    #[test]
    fn real_time_follows_the_wall_clock() {
        // 1 MHz, 40_000 cycles: the machine's 40 ms cannot pass in less than that.
        let mut pacer = Pacer::new(RealTime::start(), HZ);
        let start = Instant::now();
        for cycles in (1..=40).map(|step| step * 1_000) {
            pacer.pace(cycles);
        }
        assert!(start.elapsed() >= ms(39), "{:?}", start.elapsed());
    }
}
