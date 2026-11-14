//! The two calendars and how the chip's registers hold a date and a time: the Gregorian one
//! the host has, and the chip's own, which counts every 4th year a leap year.

use super::time::HostTime;

/// The first of the seven calendar registers is 0, and the last is the year.
pub(super) const CALENDAR: usize = 7;

// Days between 1970-01-01 and a date of the proleptic Gregorian calendar, and back. The year
// is counted from March, so that the leap day is the last day of it.
pub(super) fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_from_march = i64::from((month + 9) % 12);
    let day_of_year = (153 * month_from_march + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

pub(super) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_from_march + 2) / 5 + 1) as u32;
    let month = ((month_from_march + 2) % 12 + 1) as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

// The chip's calendar. It counts a year as a leap year when it is divisible by 4, as the
// datasheet says, 2100 included, where the Gregorian calendar the host has is not: the chip has a
// 29 February 2100. A chip that has not been set shows the host's own date; one that has counts
// in this calendar, which is the Gregorian one up to 28 February 2100, and has the days of 29
// February 2100, 2200 and so on put in after it. Day numbers are the same as the other calendar's
// (days from 1970-01-01) up to there.
fn is_chip_extra_leap_year(year: i64) -> bool {
    year >= 2100 && year % 100 == 0 && year % 400 != 0
}

/// How many of those extra days have come by the end of `year`, or the year before it.
fn extra_leap_days_by(year: i64) -> i64 {
    if year < 2100 {
        return 0;
    }
    (year / 100 - 20) - (year / 400 - 5)
}

pub(super) fn chip_days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    if month == 2 && day == 29 && is_chip_extra_leap_year(year) {
        return days_from_civil(year, 2, 28) + 1 + extra_leap_days_by(year - 1);
    }
    // The extra day of this year comes in February, so it is before March.
    let before = if month >= 3 { year } else { year - 1 };
    days_from_civil(year, month, day) + extra_leap_days_by(before)
}

pub(super) fn chip_civil_from_days(days: i64) -> (i64, u32, u32) {
    let mut extra = 0;
    for year in (2100..).step_by(100).filter(|year| year % 400 != 0) {
        let leap_day = days_from_civil(year, 2, 28) + 1 + extra;
        if leap_day > days {
            break;
        }
        if leap_day == days {
            return (year, 2, 29);
        }
        extra += 1;
    }
    civil_from_days(days - extra)
}

/// The days of a month in the chip's calendar.
pub(super) fn chip_days_in_month(year: i32, month: u32) -> u32 {
    match month {
        2 if year % 4 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

pub(super) fn bcd(n: u32) -> u8 {
    (((n / 10) << 4) | (n % 10)) as u8
}

/// A digit above 9 is not BCD; it is read as it stands (0x1F is 25) and the caller clamps.
pub(super) fn unbcd(byte: u8) -> u32 {
    u32::from(byte >> 4) * 10 + u32::from(byte & 0xF)
}

/// Which hundred years of the calendar the chip is in, counted from 2000. The year register
/// has two digits; the century bit of the month register says which hundred it is by changing
/// each time the year rolls over from 99 to 00.
pub(super) fn century(year: i32) -> i32 {
    (year - 2000).div_euclid(100)
}

/// The day of the week of a day counted from 1970-01-01, 0 (Monday) to 6 (Sunday).
pub(super) fn civil_weekday(days: i64) -> i64 {
    // 1970-01-01 was a Thursday.
    (days + 3).rem_euclid(7)
}

/// The hours register for `hour` (0 to 23): in 12-hour form it is bit 6, bit 5 for PM and the
/// hour from 1 to 12 (12 AM is midnight).
fn hour_register(hour: u32, hour12: bool) -> u8 {
    if !hour12 {
        return bcd(hour);
    }
    let on_the_clock = if hour.is_multiple_of(12) {
        12
    } else {
        hour % 12
    };
    let pm = if hour >= 12 { 0x20 } else { 0 };
    0x40 | bcd(on_the_clock) | pm
}

/// The seven calendar registers for `seconds` of wall-clock time. The weekday is 1 to 7, as the
/// chip counts it from the one it was given: `weekday_skew` days on from Monday as 1.
pub(super) fn calendar_registers(
    time: HostTime,
    days: i64,
    weekday_skew: u8,
    hour12: bool,
) -> [u8; CALENDAR] {
    let weekday = (civil_weekday(days) + i64::from(weekday_skew)) % 7 + 1;
    [
        bcd(time.second),
        bcd(time.minute),
        hour_register(time.hour, hour12),
        bcd(weekday as u32),
        bcd(time.day),
        bcd(time.month) | ((century(time.year) & 1) as u8 * 0x80),
        bcd((time.year - 2000).rem_euclid(100) as u32),
    ]
}

/// The hour a hours register holds. In 12-hour mode (bit 6) bit 5 is PM and the hour is 1 to 12;
/// the chip is read in 24-hour mode all the same. A digit out of range is clamped.
pub(super) fn register_hour(register: u8) -> u32 {
    if register & 0x40 == 0 {
        return unbcd(register & 0x3F).min(23);
    }
    let hour = unbcd(register & 0x1F).clamp(1, 12) % 12;
    hour + 12 * u32::from(register & 0x20 != 0)
}
