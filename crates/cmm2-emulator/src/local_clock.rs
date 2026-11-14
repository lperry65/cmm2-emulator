//! The host's clock for the DS3231: UTC, which runs evenly, and the wall-clock time of the time
//! zone the system is set to, as `date` shows it. The core has no clock of its own, so this is
//! where the board's chip gets the real time from. The time zone is read from the system when
//! the first time is asked for (glibc's `tzset`) and not again, so a zone changed while the
//! emulator runs (a `TZ` change, a move to another zone) is not seen until the next run;
//! daylight saving changes are, as the C library applies the zone's rules.

use cmm2_core::{HostClock, HostNow, HostTime};

#[derive(Debug, Clone, Copy, Default)]
pub struct LocalClock;

impl HostClock for LocalClock {
    fn now(&self) -> HostNow {
        // SAFETY: both calls are given structures of their own to fill, which live on the stack,
        // and keep nothing of them.
        let (now, tm) = unsafe {
            let mut now: libc::timespec = std::mem::zeroed();
            assert_eq!(
                libc::clock_gettime(libc::CLOCK_REALTIME, &mut now),
                0,
                "the system has no clock"
            );
            let mut tm: libc::tm = std::mem::zeroed();
            let filled = libc::localtime_r(&now.tv_sec, &mut tm);
            assert!(!filled.is_null(), "the system cannot convert the time");
            (now, tm)
        };
        // `time_t` and the nanoseconds are not always i64, so they are converted.
        #[allow(clippy::useless_conversion)]
        let utc_millis = i64::from(now.tv_sec) * 1000 + i64::from(now.tv_nsec) / 1_000_000;
        HostNow {
            utc_millis,
            local: host_time(&tm),
        }
    }
}

fn host_time(tm: &libc::tm) -> HostTime {
    HostTime::new(
        tm.tm_year + 1900,
        tm.tm_mon as u32 + 1,
        tm.tm_mday as u32,
        tm.tm_hour as u32,
        tm.tm_min as u32,
        // A leap second is shown as 60; the chip has no such second.
        (tm.tm_sec as u32).min(59),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tm_is_read_with_its_years_from_1900_and_months_from_0() {
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        tm.tm_year = 126;
        tm.tm_mon = 11;
        tm.tm_mday = 24;
        tm.tm_hour = 13;
        tm.tm_min = 45;
        tm.tm_sec = 56;
        assert_eq!(host_time(&tm), HostTime::new(2026, 12, 24, 13, 45, 56));
        tm.tm_sec = 60;
        assert_eq!(host_time(&tm).second, 59);
    }

    #[test]
    fn the_local_clock_gives_a_plausible_date() {
        let time = LocalClock.now().local;
        assert!(time.year >= 2026, "{time:?}");
        assert!((1..=12).contains(&time.month), "{time:?}");
        assert!((1..=31).contains(&time.day), "{time:?}");
        assert!(
            time.hour < 24 && time.minute < 60 && time.second < 60,
            "{time:?}"
        );
    }

    #[test]
    fn utc_is_in_milliseconds_and_runs_with_the_system_clock() {
        let before = LocalClock.now().utc_millis;
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let after = LocalClock.now().utc_millis;
        assert!(before <= after);
        assert!(
            (before - 5..=after + 5).contains(&seconds),
            "{before} {seconds} {after}"
        );
    }

    // The same moment as the system's own `date` says it, in the same zone.
    #[test]
    fn the_local_clock_agrees_with_the_date_command() {
        let before = LocalClock.now().local;
        let output = std::process::Command::new("date")
            .arg("+%Y %m %d %H %M")
            .output()
            .unwrap();
        let after = LocalClock.now().local;
        let text = String::from_utf8(output.stdout).unwrap();
        let [year, month, day, hour, minute] = text
            .split_whitespace()
            .map(|field| field.parse::<u32>().unwrap())
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        let seen = (year as i32, month, day, hour, minute);
        let key = |t: HostTime| (t.year, t.month, t.day, t.hour, t.minute);
        assert!(
            seen == key(before) || seen == key(after),
            "{seen:?} against {before:?} and {after:?}"
        );
    }
}
