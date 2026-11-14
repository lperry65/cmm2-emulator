//! The host's time, and the clock the front end hands in for it. The core never reads the
//! wall clock itself: tests hand in a `FixedClock`.

use std::cell::Cell;
use std::fmt::Debug;
use std::rc::Rc;

use super::calendar::{
    chip_civil_from_days, chip_days_from_civil, civil_from_days, days_from_civil,
};

/// A wall-clock time to the second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl HostTime {
    pub fn new(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> HostTime {
        HostTime {
            year,
            month,
            day,
            hour,
            minute,
            second,
        }
    }

    /// Seconds since 1970-01-01 00:00:00 of this wall-clock time, as if it were UTC.
    pub(super) fn seconds(self) -> i64 {
        self.seconds_by(days_from_civil)
    }

    pub(super) fn from_seconds(seconds: i64) -> HostTime {
        HostTime::from_seconds_by(seconds, civil_from_days)
    }

    /// As `seconds`, in the calendar of the chip (see `chip_days_from_civil`).
    pub(super) fn chip_seconds(self) -> i64 {
        self.seconds_by(chip_days_from_civil)
    }

    pub(super) fn from_chip_seconds(seconds: i64) -> HostTime {
        HostTime::from_seconds_by(seconds, chip_civil_from_days)
    }

    fn seconds_by(self, days: fn(i64, u32, u32) -> i64) -> i64 {
        days(self.year.into(), self.month, self.day) * 86_400
            + i64::from(self.hour * 3600 + self.minute * 60 + self.second)
    }

    fn from_seconds_by(seconds: i64, civil: fn(i64) -> (i64, u32, u32)) -> HostTime {
        let (year, month, day) = civil(seconds.div_euclid(86_400));
        let of_day = seconds.rem_euclid(86_400) as u32;
        HostTime::new(
            year as i32,
            month,
            day,
            of_day / 3600,
            of_day / 60 % 60,
            of_day % 60,
        )
    }
}

/// The host's time at one moment: UTC, which runs evenly, and what the wall clock of the
/// time zone the host is in shows, which does not (daylight saving, a change of zone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostNow {
    /// Milliseconds since 1970-01-01 00:00:00 UTC.
    pub utc_millis: i64,
    pub local: HostTime,
}

/// Where the chip gets the host's time. The core never reads the wall clock itself: the front
/// end hands one in, and tests hand in a `FixedClock`.
pub trait HostClock: Debug {
    fn now(&self) -> HostNow;
}

/// A clock that stands still until it is moved on. Clones share the time. It is in a time zone
/// that is UTC until `set_zone` says otherwise.
#[derive(Debug, Clone)]
pub struct FixedClock(Rc<FixedTime>);

#[derive(Debug)]
struct FixedTime {
    utc_millis: Cell<i64>,
    zone_seconds: Cell<i64>,
}

impl FixedClock {
    /// A clock showing `local`, in UTC.
    pub fn new(local: HostTime) -> FixedClock {
        FixedClock(Rc::new(FixedTime {
            utc_millis: Cell::new(local.seconds() * 1000),
            zone_seconds: Cell::new(0),
        }))
    }

    pub fn advance(&self, seconds: i64) {
        self.advance_millis(seconds * 1000);
    }

    pub fn advance_millis(&self, millis: i64) {
        self.0.utc_millis.set(self.0.utc_millis.get() + millis);
    }

    /// Puts the wall clock `seconds` ahead of UTC, as a daylight saving change or a move to
    /// another time zone does. UTC does not change.
    pub fn set_zone(&self, seconds: i64) {
        self.0.zone_seconds.set(seconds);
    }
}

impl Default for FixedClock {
    /// The same moment as the STM32 RTC's reset values.
    fn default() -> FixedClock {
        FixedClock::new(HostTime::new(2000, 1, 1, 0, 0, 0))
    }
}

impl HostClock for FixedClock {
    fn now(&self) -> HostNow {
        let utc_millis = self.0.utc_millis.get();
        HostNow {
            utc_millis,
            local: HostTime::from_seconds(utc_millis.div_euclid(1000) + self.0.zone_seconds.get()),
        }
    }
}
