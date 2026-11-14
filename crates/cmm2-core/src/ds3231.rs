//! The board's DS3231 real-time clock (IC7), the chip that keeps the time with the power off.
//! The firmware does not use an I2C controller for it: it bit-bangs PD4 (SDA) and PD5 (SCL),
//! so this is a slave that watches the two wires, and the GPIO port D tells it what the
//! firmware drives and asks it what it pulls low (see docs/ds3231.md). The slot is a cheap
//! handle, as `SdSlot` is: clones share the chip, so the port and the front end each keep one.
//!
//! Until the guest sets the time the chip shows the host's local time, as I decided. A time or
//! date written to it makes it a clock of its own, like the real chip: it counts seconds evenly
//! from then on and knows nothing of daylight saving or time zones. What the front end keeps
//! in the `--flash` file in place of the coin cell is the offset of that clock from the host's
//! UTC time.

use std::cell::{Cell, RefCell};
use std::fmt::{self, Debug};
use std::rc::Rc;

/// The chip's 7-bit address on the bus.
const ADDRESS: u8 = 0x68;
/// Registers 0x00 to 0x12.
const REGISTERS: usize = 0x13;
/// The first of the seven calendar registers is 0, and the last is the year.
const CALENDAR: usize = 7;
const CONTROL: usize = 0x0E;
const STATUS: usize = 0x0F;
const TEMPERATURE: usize = 0x11;

/// A new chip's status: the oscillator has stopped (OSF) and the 32 kHz output is enabled.
const STATUS_AT_POWER_UP: u8 = 0x88;
/// The flags that a write can only clear: OSF, and the two alarm flags.
const STATUS_FLAGS: u8 = 0x83;
const EN32KHZ: u8 = 0x08;

/// The most the chip's clock may differ from the host's before a saved offset is refused:
/// 200 years.
const MAX_OFFSET_MILLIS: i64 = 200 * 366 * 86_400 * 1000;

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
    fn seconds(self) -> i64 {
        days_from_civil(self.year.into(), self.month, self.day) * 86_400
            + i64::from(self.hour * 3600 + self.minute * 60 + self.second)
    }

    fn from_seconds(seconds: i64) -> HostTime {
        let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
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

// Days between 1970-01-01 and a date of the proleptic Gregorian calendar, and back. The year
// is counted from March, so that the leap day is the last day of it.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_from_march = i64::from((month + 9) % 12);
    let day_of_year = (153 * month_from_march + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
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

fn days_in_month(year: i32, month: u32) -> u32 {
    let next = if month == 12 {
        days_from_civil(year.into(), 12, 1) + 31
    } else {
        days_from_civil(year.into(), month + 1, 1)
    };
    (next - days_from_civil(year.into(), month, 1)) as u32
}

fn bcd(n: u32) -> u8 {
    (((n / 10) << 4) | (n % 10)) as u8
}

/// A digit above 9 is not BCD; it is read as it stands (0x1F is 25) and the caller clamps.
fn unbcd(byte: u8) -> u32 {
    u32::from(byte >> 4) * 10 + u32::from(byte & 0xF)
}

/// Which hundred years of the calendar the chip is in, counted from 2000. The year register
/// has two digits; the century bit of the month register says which hundred it is by changing
/// each time the year rolls over from 99 to 00.
fn century(year: i32) -> i32 {
    (year - 2000).div_euclid(100)
}

/// The day of the week of a day counted from 1970-01-01, 0 (Monday) to 6 (Sunday).
fn civil_weekday(days: i64) -> i64 {
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
fn calendar_registers(seconds: i64, weekday_skew: u8, hour12: bool) -> [u8; CALENDAR] {
    let time = HostTime::from_seconds(seconds);
    let weekday = (civil_weekday(seconds.div_euclid(86_400)) + i64::from(weekday_skew)) % 7 + 1;
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
fn register_hour(register: u8) -> u32 {
    if register & 0x40 == 0 {
        return unbcd(register & 0x3F).min(23);
    }
    let hour = unbcd(register & 0x1F).clamp(1, 12) % 12;
    hour + 12 * u32::from(register & 0x20 != 0)
}

fn next_register(pointer: usize) -> usize {
    (pointer + 1) % REGISTERS
}

/// The saved offset is further from the host's clock than the chip can have been left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OffsetError {
    pub millis: i64,
}

impl fmt::Display for OffsetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the DS3231's clock is {} seconds from the host's, more than 200 years",
            self.millis / 1000
        )
    }
}

impl std::error::Error for OffsetError {}

fn checked(offset: Option<i64>) -> Result<Option<i64>, OffsetError> {
    match offset {
        Some(millis) if millis.unsigned_abs() > MAX_OFFSET_MILLIS.unsigned_abs() => {
            Err(OffsetError { millis })
        }
        _ => Ok(offset),
    }
}

/// What the slave is doing with the clock pulses that come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// No transaction, or one for another address: the chip does not take part until a START.
    Idle,
    /// Receiving the address byte, and then the register pointer and the data bytes of a write.
    Address,
    Pointer,
    Data,
    /// Acknowledging a read address; the first byte goes out when that clock pulse is over.
    ReadAcknowledged,
    /// Sending this byte to the master.
    Sending(u8),
}

#[derive(Debug)]
struct Chip {
    clock: Box<dyn HostClock>,
    /// The chip's own clock, in milliseconds ahead of the host's UTC time. `None` until the
    /// guest sets the time: the chip then shows the host's local time. This is what the coin
    /// cell stands for.
    offset: Option<i64>,
    /// Registers 0x07 to 0x12 as the chip holds them. The calendar ones are worked out.
    stored: [u8; REGISTERS],
    /// The weekday register is a count the chip keeps: it goes on a day at midnight from the
    /// value it was given, whatever the date. This is how many days that value is on from the
    /// day the date says it is (Monday is 1), 0 to 6.
    weekday_skew: u8,
    /// The hours register was last written in 12-hour form (bit 6), and is read in it.
    hour12: bool,
    /// The registers as a transaction reads them: the calendar is taken at START, and again
    /// when the pointer wraps round to 0, so that a read that runs over a second boundary is
    /// still consistent.
    registers: [u8; REGISTERS],
    /// The date registers (date, month, year) written in this transaction, as they were
    /// written. A date is written a register at a time, and 30 only fits a month that has 30
    /// days once the month has been written too.
    date_written: [Option<u8>; 3],
    pointer: usize,
    phase: Phase,
    /// Clock pulses of the byte so far (1 to 9, the ninth being the acknowledge).
    pulses: u8,
    shift: u8,
    master_acknowledged: bool,
    /// The levels the master drives on the wires; the chip never changes these.
    sda: bool,
    scl: bool,
    /// The chip holds SDA low.
    pulling_low: bool,
}

impl Default for Chip {
    fn default() -> Chip {
        Chip::new(Box::new(FixedClock::default()))
    }
}

impl Chip {
    fn new(clock: Box<dyn HostClock>) -> Chip {
        let mut stored = [0; REGISTERS];
        // Control at its power-up value, the status of a chip that has never run, and a
        // temperature of 25 degrees.
        stored[CONTROL] = 0x1C;
        stored[STATUS] = STATUS_AT_POWER_UP;
        stored[TEMPERATURE] = 25;
        Chip {
            clock,
            offset: None,
            stored,
            weekday_skew: 0,
            hour12: false,
            registers: stored,
            date_written: [None; 3],
            pointer: 0,
            phase: Phase::Idle,
            pulses: 0,
            shift: 0,
            master_acknowledged: false,
            sda: true,
            scl: true,
            pulling_low: false,
        }
    }

    /// Whether the chip is as it was made, with nothing for the coin cell to have kept.
    fn is_as_made(&self) -> bool {
        self.offset.is_none()
            && self.stored[STATUS] == STATUS_AT_POWER_UP
            && self.weekday_skew == 0
            && !self.hour12
    }

    /// Puts what the coin cell keeps back as it was made.
    fn make_new(&mut self) {
        self.offset = None;
        self.stored[STATUS] = STATUS_AT_POWER_UP;
        self.weekday_skew = 0;
        self.hour12 = false;
    }

    /// The chip's wall-clock time in milliseconds, as if it were UTC.
    fn wall_millis(&self, now: &HostNow) -> i64 {
        match self.offset {
            None => now.local.seconds() * 1000 + now.utc_millis.rem_euclid(1000),
            Some(offset) => now.utc_millis.saturating_add(offset),
        }
    }

    fn set_lines(&mut self, sda: bool, scl: bool) {
        let (sda_was, scl_was) = (self.sda, self.scl);
        self.sda = sda;
        self.scl = scl;
        if scl != scl_was {
            if scl {
                self.rising();
            } else {
                self.falling();
            }
        }
        // SDA moving while SCL is high is not data: a fall starts a transaction, a rise ends it.
        if sda != sda_was && scl {
            if sda {
                self.stop();
            } else {
                self.start();
            }
        }
    }

    /// The master has gone: the pins are inputs again and the board's pull-ups hold both wires
    /// high. A transaction in progress is dropped. What the chip had taken from it stays, as the
    /// chip takes each byte as it acknowledges it, and the chip is not reset.
    fn release_bus(&mut self) {
        self.phase = Phase::Idle;
        self.pulses = 0;
        self.pulling_low = false;
        self.sda = true;
        self.scl = true;
    }

    fn start(&mut self) {
        self.registers = self.stored;
        self.refresh_calendar();
        self.date_written = [None; 3];
        self.phase = Phase::Address;
        self.pulses = 0;
        self.pulling_low = false;
    }

    fn stop(&mut self) {
        self.date_written = [None; 3];
        self.phase = Phase::Idle;
        self.pulses = 0;
        self.pulling_low = false;
    }

    fn refresh_calendar(&mut self) {
        let wall = self.wall_millis(&self.clock.now());
        let calendar = calendar_registers(wall.div_euclid(1000), self.weekday_skew, self.hour12);
        self.registers[..CALENDAR].copy_from_slice(&calendar);
    }

    // Data is sampled while the clock is high.
    fn rising(&mut self) {
        self.pulses += 1;
        match self.phase {
            Phase::Address | Phase::Pointer | Phase::Data if self.pulses <= 8 => {
                self.shift = self.shift << 1 | u8::from(self.sda);
            }
            Phase::Sending(_) if self.pulses == 9 => self.master_acknowledged = !self.sda,
            _ => {}
        }
    }

    // The chip changes what it drives while the clock is low.
    fn falling(&mut self) {
        match (self.phase, self.pulses) {
            (Phase::Address | Phase::Pointer | Phase::Data, 8) => {
                self.pulling_low = self.receive(self.shift);
            }
            (Phase::Sending(_), 8) => self.pulling_low = false,
            (Phase::ReadAcknowledged, 9) => self.send_next(),
            (Phase::Sending(_), 9) if self.master_acknowledged => self.send_next(),
            (Phase::Sending(_), 9) => {
                self.phase = Phase::Idle;
                self.end_of_byte();
            }
            (Phase::Sending(byte), pulse @ 1..=7) => {
                self.pulling_low = byte >> (7 - pulse) & 1 == 0;
            }
            (_, 9) => self.end_of_byte(),
            _ => {}
        }
    }

    fn end_of_byte(&mut self) {
        self.pulses = 0;
        self.pulling_low = false;
    }

    // Moves the pointer on a register. Wrapping round to the seconds takes the time again.
    fn advance_pointer(&mut self) {
        self.pointer = next_register(self.pointer);
        if self.pointer == 0 {
            self.refresh_calendar();
        }
    }

    // Puts the next register on the wire, most significant bit first.
    fn send_next(&mut self) {
        let byte = self.registers[self.pointer];
        self.advance_pointer();
        self.phase = Phase::Sending(byte);
        self.pulses = 0;
        self.pulling_low = byte & 0x80 == 0;
    }

    /// Takes a byte the master sent. True if the chip acknowledges it.
    fn receive(&mut self, byte: u8) -> bool {
        match self.phase {
            Phase::Address if byte >> 1 != ADDRESS => self.phase = Phase::Idle,
            Phase::Address if byte & 1 != 0 => self.phase = Phase::ReadAcknowledged,
            Phase::Address => self.phase = Phase::Pointer,
            Phase::Pointer => {
                self.pointer = usize::from(byte) % REGISTERS;
                self.phase = Phase::Data;
            }
            Phase::Data => {
                self.write_register(byte);
                self.advance_pointer();
            }
            _ => unreachable!("receive is only called for a byte the master sent"),
        }
        self.phase != Phase::Idle
    }

    /// A byte for the register the pointer is at, which takes effect as the chip acknowledges it.
    fn write_register(&mut self, byte: u8) {
        let register = self.pointer;
        match register {
            ..CALENDAR => self.write_calendar(register, byte),
            STATUS => {
                // The flags can only be cleared, BSY is the chip's own, and 32 kHz is a switch.
                let status = (self.stored[STATUS] & byte & STATUS_FLAGS) | (byte & EN32KHZ);
                self.stored[STATUS] = status;
                self.registers[STATUS] = status;
            }
            // The temperature is the chip's own measurement; writing it does nothing.
            TEMPERATURE.. => {}
            _ => {
                self.stored[register] = byte;
                self.registers[register] = byte;
            }
        }
    }

    /// The weekday the chip shows on the day `days` (counted from 1970-01-01), 0 to 6.
    fn weekday_on(&self, days: i64) -> i64 {
        (civil_weekday(days) + i64::from(self.weekday_skew)) % 7
    }

    /// Makes the weekday on the day `days` be `weekday` (0 to 6): the chip counts on from there.
    fn set_weekday(&mut self, days: i64, weekday: i64) {
        self.weekday_skew = (weekday - civil_weekday(days)).rem_euclid(7) as u8;
    }

    /// Writes one field of the time. The other fields go on as they were counting: only the
    /// seconds register restarts the counting of the second, as it does on the chip, and a
    /// write to it begins a new second now. The chip is then a clock of its own, except for a
    /// write of the weekday, which is only the chip's count of the days of the week.
    fn write_calendar(&mut self, register: usize, byte: u8) {
        let now = self.clock.now();
        let wall = self.wall_millis(&now);
        let today = wall.div_euclid(1000).div_euclid(86_400);
        // The weekday as it is before this write: only a write of its own changes it, and one
        // that moves the date leaves it as it was.
        let weekday = self.weekday_on(today);
        let mut time = HostTime::from_seconds(wall.div_euclid(1000));
        let mut millis = wall.rem_euclid(1000);
        match register {
            0 => {
                time.second = unbcd(byte & 0x7F).min(59);
                millis = 0;
            }
            1 => time.minute = unbcd(byte & 0x7F).min(59),
            2 => {
                time.hour = register_hour(byte);
                self.hour12 = byte & 0x40 != 0;
            }
            3 => {
                // Three bits, and 1 to 7: the datasheet leaves anything else undefined, and here
                // 0 is 1.
                self.set_weekday(today, i64::from((byte & 0x07).max(1)) - 1);
                return;
            }
            _ => self.date_written[register - 4] = Some(byte),
        }
        let [day, month, year] = self.date_written;
        // The century bit goes with the month, and says which hundred years the year is in: the
        // 2000s with 0 and the 2100s with 1, as the chip's own is 1 after its first rollover.
        // Without a month written the chip stays in the hundred years it is in.
        let century = month.map_or(century(time.year), |m| i32::from(m >> 7));
        let month = month.map_or(time.month, |m| unbcd(m & 0x1F).clamp(1, 12));
        let digits = year.map_or((time.year - 2000).rem_euclid(100), |y| {
            unbcd(y).min(99) as i32
        });
        let year = 2000 + 100 * century + digits;
        let day = day.map_or(time.day, |d| unbcd(d & 0x3F).max(1));
        time.month = month;
        time.year = year;
        // A month that does not have the day is not a date. The datasheet leaves what the chip
        // does then undefined, and this one takes the last day of the month.
        time.day = day.min(days_in_month(year, month));
        let target = time.seconds() * 1000 + millis;
        self.set_weekday(target.div_euclid(86_400_000), weekday);
        // The chip must stay as far from the host as a saved offset may be, or it could not be
        // loaded again.
        let offset = target.saturating_sub(now.utc_millis);
        self.offset = Some(offset.clamp(-MAX_OFFSET_MILLIS, MAX_OFFSET_MILLIS));
    }
}

/// How many bytes `Ds3231Slot::saved` gives.
pub const STATE_SIZE: usize = 10;
/// Where the weekday count and the 12-hour form are in the flags byte of the saved state, whose
/// bit 0 says the time has been set.
const FLAGS_WEEKDAY: u32 = 1;
const FLAGS_HOUR12: u32 = 4;

#[derive(Clone, Debug, Default)]
pub struct Ds3231Slot(Rc<RefCell<Chip>>);

impl Ds3231Slot {
    pub fn new(clock: Box<dyn HostClock>) -> Ds3231Slot {
        Ds3231Slot(Rc::new(RefCell::new(Chip::new(clock))))
    }

    /// Changes where the host's time comes from. What the chip keeps stays.
    pub fn set_host_clock(&self, clock: Box<dyn HostClock>) {
        self.0.borrow_mut().clock = clock;
    }

    /// Milliseconds the chip's clock is ahead of the host's UTC time (behind, if negative), or
    /// `None` while the time has never been set and the chip follows the host's local time.
    pub fn offset_millis(&self) -> Option<i64> {
        self.0.borrow().offset
    }

    /// Sets the chip's clock, or takes it back to the host's local time with `None`. An offset of
    /// more than 200 years is refused.
    pub fn set_offset_millis(&self, offset: Option<i64>) -> Result<(), OffsetError> {
        self.0.borrow_mut().offset = checked(offset)?;
        Ok(())
    }

    /// The time the chip shows.
    pub fn time(&self) -> HostTime {
        let chip = self.0.borrow();
        HostTime::from_seconds(chip.wall_millis(&chip.clock.now()).div_euclid(1000))
    }

    /// What the chip keeps with the power off, for the front end to save: the offset, the status
    /// register, and the weekday and 12-hour form the guest has set, which are in the flags byte
    /// (bit 0 says the time has been set, bits 1 to 3 are the weekday count, bit 4 the 12-hour
    /// form). `None` for a chip as it was made (time never set, oscillator stopped flag still
    /// set, weekday and form as at power-up), as there is nothing to keep then.
    pub fn saved(&self) -> Option<[u8; STATE_SIZE]> {
        let chip = self.0.borrow();
        if chip.is_as_made() {
            return None;
        }
        let mut bytes = [0; STATE_SIZE];
        bytes[..8].copy_from_slice(&chip.offset.unwrap_or(0).to_le_bytes());
        bytes[8] = u8::from(chip.offset.is_some())
            | chip.weekday_skew << FLAGS_WEEKDAY
            | u8::from(chip.hour12) << FLAGS_HOUR12;
        bytes[9] = chip.stored[STATUS];
        Some(bytes)
    }

    /// Takes back what `saved` gave. The chip is left as it was if the offset is refused.
    pub fn load(&self, bytes: [u8; STATE_SIZE]) -> Result<(), OffsetError> {
        let millis = i64::from_le_bytes(bytes[..8].try_into().expect("eight bytes"));
        let offset = checked((bytes[8] & 1 != 0).then_some(millis))?;
        let mut chip = self.0.borrow_mut();
        chip.offset = offset;
        // BSY is the chip's own and is never set.
        chip.stored[STATUS] = bytes[9] & (STATUS_FLAGS | EN32KHZ);
        chip.weekday_skew = (bytes[8] >> FLAGS_WEEKDAY & 7) % 7;
        chip.hour12 = bytes[8] >> FLAGS_HOUR12 & 1 != 0;
        Ok(())
    }

    /// Puts the chip back as it was made, as for a front end with nothing saved.
    pub fn load_new(&self) {
        self.0.borrow_mut().make_new();
    }

    /// Takes the offset an earlier version saved: whole seconds from the host's local time, 0 for
    /// a chip never set. The time it meant is kept, now as an offset from UTC.
    pub fn load_v1(&self, seconds: i64) -> Result<(), OffsetError> {
        let millis = checked(Some(seconds.saturating_mul(1000)))?;
        let mut chip = self.0.borrow_mut();
        // Both checks are made before anything of the chip is changed.
        let offset = if seconds == 0 {
            None
        } else {
            let now = chip.clock.now();
            let zone = now.local.seconds() - now.utc_millis.div_euclid(1000);
            checked(millis.map(|millis| millis.saturating_add(zone * 1000)))?
        };
        chip.make_new();
        chip.offset = offset;
        Ok(())
    }

    /// The levels of SDA and SCL that the firmware drives.
    pub(crate) fn set_lines(&self, sda: bool, scl: bool) {
        self.0.borrow_mut().set_lines(sda, scl);
    }

    /// A system reset has put the pins back to inputs; see `Chip::release_bus`.
    pub(crate) fn release_bus(&self) {
        self.0.borrow_mut().release_bus();
    }

    /// The chip is pulling SDA low.
    pub(crate) fn pulls_sda_low(&self) -> bool {
        self.0.borrow().pulling_low
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-12-24 (a Thursday) 13:45:56, with December for the month register's tens bit.
    fn december() -> HostTime {
        HostTime::new(2026, 12, 24, 13, 45, 56)
    }

    // A chip on a fixed clock, and a master that moves the wires as the firmware's routines do.
    struct Bus {
        chip: Ds3231Slot,
        clock: FixedClock,
    }

    impl Bus {
        fn at(time: HostTime) -> Bus {
            let clock = FixedClock::new(time);
            Bus {
                chip: Ds3231Slot::new(Box::new(clock.clone())),
                clock,
            }
        }

        fn lines(&self, sda: bool, scl: bool) {
            self.chip.set_lines(sda, scl);
        }

        fn start(&self) {
            self.lines(true, true);
            self.lines(false, true);
            self.lines(false, false);
        }

        fn stop(&self) {
            self.lines(false, false);
            self.lines(false, true);
            self.lines(true, true);
        }

        // One clock pulse with the master driving `sda` (released is high).
        fn pulse(&self, sda: bool) -> bool {
            self.lines(sda, false);
            self.lines(sda, true);
            let level = !self.chip.pulls_sda_low() && sda;
            self.lines(sda, false);
            level
        }

        // True if the chip acknowledged.
        fn send(&self, byte: u8) -> bool {
            for bit in (0..8).rev() {
                self.pulse(byte >> bit & 1 != 0);
            }
            !self.pulse(true)
        }

        fn receive(&self, acknowledge: bool) -> u8 {
            let byte = (0..8).fold(0, |byte, _| byte << 1 | u8::from(self.pulse(true)));
            self.pulse(!acknowledge);
            self.lines(true, false);
            byte
        }

        // The firmware's read: pointer write, STOP, START, read address, `count` bytes with the
        // last not acknowledged.
        fn read(&self, register: u8, count: usize) -> Vec<u8> {
            self.start();
            assert!(self.send(ADDRESS << 1));
            assert!(self.send(register));
            self.stop();
            self.start();
            assert!(self.send(ADDRESS << 1 | 1));
            let bytes = (0..count).map(|i| self.receive(i + 1 < count)).collect();
            self.stop();
            bytes
        }

        fn write(&self, register: u8, bytes: &[u8]) {
            self.start();
            assert!(self.send(ADDRESS << 1));
            assert!(self.send(register));
            for &byte in bytes {
                assert!(self.send(byte));
            }
            self.stop();
        }
    }

    #[test]
    fn the_calendar_arithmetic_matches_known_days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(2026, 12, 24), 20_811);
        for days in [-800, -1, 0, 59, 60, 11_016, 11_017, 20_811, 50_000] {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, day), days, "{days}");
        }
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(2100, 2), 28);
        assert_eq!(days_in_month(2026, 12), 31);
        assert_eq!(days_in_month(2026, 11), 30);
    }

    #[test]
    fn host_time_round_trips_through_seconds() {
        for time in [december(), HostTime::new(2000, 2, 29, 23, 59, 59)] {
            assert_eq!(HostTime::from_seconds(time.seconds()), time);
        }
    }

    #[test]
    fn a_calendar_in_october_to_december_has_the_tens_bit_of_the_month() {
        for (month, bits) in [(9, 0x09), (10, 0x10), (11, 0x11), (12, 0x12)] {
            let bus = Bus::at(HostTime::new(2026, month, 5, 0, 0, 0));
            assert_eq!(bus.read(5, 1), [bits], "month {month}");
        }
    }

    #[test]
    fn the_calendar_registers_show_the_host_time_in_bcd() {
        let bus = Bus::at(december());
        // Seconds, minutes, hours, weekday (Thursday), date, month, year.
        assert_eq!(bus.read(0, 7), [0x56, 0x45, 0x13, 0x04, 0x24, 0x12, 0x26]);
    }

    #[test]
    fn sunday_is_seven_and_monday_is_one() {
        let sunday = Bus::at(HostTime::new(2026, 11, 15, 0, 0, 0));
        assert_eq!(sunday.read(3, 1), [7]);
        let monday = Bus::at(HostTime::new(2026, 11, 16, 0, 0, 0));
        assert_eq!(monday.read(3, 1), [1]);
    }

    #[test]
    fn a_leap_day_and_the_end_of_a_year_read_correctly() {
        let bus = Bus::at(HostTime::new(2028, 2, 29, 23, 59, 59));
        assert_eq!(bus.read(4, 3), [0x29, 0x02, 0x28]);
        bus.clock.advance(1);
        assert_eq!(bus.read(4, 3), [0x01, 0x03, 0x28]);
        let bus = Bus::at(HostTime::new(2026, 12, 31, 23, 59, 59));
        bus.clock.advance(1);
        assert_eq!(bus.read(0, 7), [0, 0, 0, 5, 1, 1, 0x27]);
    }

    #[test]
    fn the_time_counts_with_the_host_clock() {
        let bus = Bus::at(december());
        bus.clock.advance(5);
        assert_eq!(bus.read(0, 3), [0x01, 0x46, 0x13]);
    }

    #[test]
    fn the_address_is_acknowledged_only_for_the_chip() {
        let bus = Bus::at(december());
        bus.start();
        assert!(bus.send(0x68 << 1));
        bus.stop();
        bus.start();
        assert!(bus.send(0x68 << 1 | 1));
        bus.stop();
        for other in [0x69, 0x48, 0x50, 0x00] {
            bus.start();
            assert!(!bus.send(other << 1), "{other:#x}");
            assert!(!bus.chip.pulls_sda_low());
            // The chip stays out of the rest of that transaction.
            assert!(!bus.send(0));
            bus.stop();
        }
    }

    #[test]
    fn nothing_pulls_sda_low_when_idle() {
        let bus = Bus::at(december());
        assert!(!bus.chip.pulls_sda_low());
        bus.start();
        assert!(!bus.chip.pulls_sda_low());
        bus.stop();
        assert!(!bus.chip.pulls_sda_low());
    }

    #[test]
    fn data_is_only_taken_when_the_clock_is_high() {
        // SDA changes with SCL low are data setup, not a START or a STOP.
        let bus = Bus::at(december());
        bus.start();
        bus.lines(true, false);
        bus.lines(false, false);
        bus.lines(true, false);
        assert!(!bus.chip.pulls_sda_low());
        assert_eq!(bus.chip.0.borrow().phase, Phase::Address);
    }

    #[test]
    fn a_read_ends_when_the_master_does_not_acknowledge() {
        let bus = Bus::at(december());
        bus.start();
        bus.send(ADDRESS << 1);
        bus.send(0);
        bus.stop();
        bus.start();
        bus.send(ADDRESS << 1 | 1);
        assert_eq!(bus.receive(true), 0x56);
        assert_eq!(bus.receive(false), 0x45);
        // After the NACK the chip lets go of the bus and stays quiet.
        for _ in 0..16 {
            bus.pulse(true);
            assert!(!bus.chip.pulls_sda_low());
        }
        bus.stop();
    }

    #[test]
    fn the_chip_lets_go_of_sda_for_the_masters_acknowledge() {
        // 0x56 ends in a 0 bit, which the chip holds low until the clock pulse after it.
        let bus = Bus::at(december());
        bus.start();
        bus.send(ADDRESS << 1);
        bus.send(0);
        bus.stop();
        bus.start();
        bus.send(ADDRESS << 1 | 1);
        for bit in (0..8).rev() {
            bus.pulse(true);
            assert_eq!(
                bus.chip.pulls_sda_low(),
                bit > 0 && 0x56 >> (bit - 1) & 1 == 0
            );
        }
        bus.stop();
    }

    #[test]
    fn the_pointer_wraps_from_0x12_to_0() {
        let bus = Bus::at(december());
        // Registers 0x11 and 0x12 (the temperature), then 0, 1 and 2.
        assert_eq!(bus.read(0x11, 5), [25, 0, 0x56, 0x45, 0x13]);
    }

    #[test]
    fn the_pointer_survives_a_stop() {
        let bus = Bus::at(december());
        bus.start();
        bus.send(ADDRESS << 1);
        bus.send(4);
        bus.stop();
        bus.start();
        bus.send(ADDRESS << 1 | 1);
        assert_eq!(bus.receive(true), 0x24);
        assert_eq!(bus.receive(false), 0x12);
        bus.stop();
        // The next read with no pointer write carries on from where the last one ended.
        bus.start();
        bus.send(ADDRESS << 1 | 1);
        assert_eq!(bus.receive(false), 0x26);
        bus.stop();
    }

    #[test]
    fn a_pointer_past_the_last_register_wraps_too() {
        let bus = Bus::at(december());
        assert_eq!(bus.read(0x13, 1), [0x56]);
        assert_eq!(bus.read(0x14, 1), [0x45]);
    }

    #[test]
    fn the_other_registers_start_as_the_chip_powers_up() {
        let bus = Bus::at(december());
        // Alarms, control (0x1C), status (the oscillator has stopped, and the 32 kHz output is
        // on), aging, temperature.
        assert_eq!(bus.read(7, 12), [0, 0, 0, 0, 0, 0, 0, 0x1C, 0x88, 0, 25, 0]);
    }

    #[test]
    fn those_registers_keep_what_is_written() {
        let bus = Bus::at(december());
        bus.write(7, &[1, 2, 3, 4, 5, 6, 7]);
        bus.write(0x0E, &[0x04]);
        bus.write(0x10, &[0xFE]);
        assert_eq!(bus.read(7, 10), [1, 2, 3, 4, 5, 6, 7, 0x04, 0x88, 0xFE]);
        // None of them is a calendar register, so the time was not set.
        assert_eq!(bus.chip.offset_millis(), None);
        // The temperature is the chip's own.
        bus.write(0x11, &[1, 2]);
        assert_eq!(bus.read(0x11, 2), [25, 0]);
    }

    #[test]
    fn writing_the_time_sets_it_and_it_counts_on_from_there() {
        let bus = Bus::at(december());
        // Hours, minutes, seconds as the firmware's TIME$ = "10:20:30" writes them.
        bus.write(0, &[0x30, 0x20, 0x10]);
        assert_eq!(bus.read(0, 7), [0x30, 0x20, 0x10, 0x04, 0x24, 0x12, 0x26]);
        bus.clock.advance(45);
        assert_eq!(bus.read(0, 3), [0x15, 0x21, 0x10]);
        assert_eq!(
            bus.chip.offset_millis(),
            Some(-(3 * 3600 + 25 * 60 + 26) * 1000)
        );
    }

    #[test]
    fn writing_the_date_keeps_the_time_of_day() {
        let bus = Bus::at(december());
        // Weekday, date, month, year, as DATE$ = "05/11/27" writes them: a Friday.
        bus.write(3, &[0x05, 0x05, 0x11, 0x27]);
        assert_eq!(bus.read(0, 7), [0x56, 0x45, 0x13, 0x05, 0x05, 0x11, 0x27]);
    }

    #[test]
    fn a_time_written_across_midnight_carries_into_the_next_day() {
        let bus = Bus::at(december());
        bus.write(0, &[0x50, 0x59, 0x23]);
        bus.clock.advance(10);
        assert_eq!(bus.read(0, 7), [0, 0, 0, 5, 0x25, 0x12, 0x26]);
    }

    #[test]
    fn the_registers_are_taken_when_the_transaction_starts() {
        let bus = Bus::at(HostTime::new(2026, 12, 24, 13, 45, 59));
        bus.start();
        bus.send(ADDRESS << 1);
        bus.send(0);
        bus.stop();
        bus.start();
        bus.send(ADDRESS << 1 | 1);
        assert_eq!(bus.receive(true), 0x59);
        bus.clock.advance(1);
        // The minute has turned over on the host, not in the snapshot.
        assert_eq!(bus.receive(false), 0x45);
        bus.stop();
    }

    #[test]
    fn a_register_that_is_not_written_goes_on_counting_during_the_write() {
        let bus = Bus::at(december());
        bus.start();
        bus.send(ADDRESS << 1);
        bus.send(0);
        bus.send(0x30);
        bus.clock.advance(3);
        bus.send(0x20);
        bus.send(0x10);
        bus.stop();
        // The seconds were written, then three went by, then the minutes and the hours.
        assert_eq!(bus.read(0, 3), [0x33, 0x20, 0x10]);
    }

    #[test]
    fn a_date_write_does_not_lose_the_seconds_that_pass_during_it() {
        let bus = Bus::at(december());
        bus.start();
        bus.send(ADDRESS << 1);
        bus.send(3);
        bus.send(0x05);
        bus.clock.advance(1);
        bus.send(0x05);
        bus.clock.advance(1);
        bus.send(0x11);
        bus.clock.advance(1);
        bus.send(0x27);
        bus.stop();
        assert_eq!(bus.read(0, 7), [0x59, 0x45, 0x13, 0x05, 0x05, 0x11, 0x27]);
    }

    #[test]
    fn a_day_written_before_its_month_is_kept_for_the_month() {
        // The 30th does not fit February, and the date is written a register at a time.
        let bus = Bus::at(HostTime::new(2027, 2, 10, 12, 0, 0));
        bus.write(3, &[3, 0x30, 0x03, 0x27]);
        assert_eq!(bus.read(4, 3), [0x30, 0x03, 0x27]);
    }

    #[test]
    fn only_the_seconds_register_restarts_the_second() {
        let bus = Bus::at(december());
        bus.clock.advance_millis(600);
        bus.write(1, &[0x50]);
        bus.clock.advance_millis(500);
        // The minutes did not touch the phase: 56.6 and 0.5 on is 57.1.
        assert_eq!(bus.read(0, 2), [0x57, 0x50]);

        let bus = Bus::at(december());
        bus.clock.advance_millis(600);
        bus.write(0, &[0x10]);
        bus.clock.advance_millis(500);
        // The seconds did: the second written began when it was written, and is 0.5 old.
        assert_eq!(bus.read(0, 1), [0x10]);
        bus.clock.advance_millis(500);
        assert_eq!(bus.read(0, 1), [0x11]);
    }

    #[test]
    fn a_write_cut_short_keeps_what_the_chip_took() {
        let bus = Bus::at(december());
        bus.start();
        bus.send(ADDRESS << 1);
        bus.send(0x0A);
        bus.send(0x55);
        bus.send(0x30);
        // The master goes away while the chip is acknowledging the next byte.
        for bit in (0..8).rev() {
            bus.pulse(0x20 >> bit & 1 != 0);
        }
        assert!(bus.chip.pulls_sda_low());
        bus.chip.release_bus();
        assert!(!bus.chip.pulls_sda_low());
        // Two bytes were acknowledged, and so was the one the master was sending when it went.
        assert_eq!(bus.read(0x0A, 3), [0x55, 0x30, 0x20]);
    }

    #[test]
    fn a_time_write_cut_short_keeps_the_seconds_it_took() {
        let bus = Bus::at(december());
        bus.start();
        bus.send(ADDRESS << 1);
        bus.send(0);
        bus.send(0x30);
        bus.chip.release_bus();
        assert_eq!(bus.read(0, 3), [0x30, 0x45, 0x13]);
        assert_eq!(bus.chip.offset_millis(), Some(-26_000));
    }

    #[test]
    fn a_write_with_only_a_pointer_changes_nothing() {
        let bus = Bus::at(december());
        bus.write(0, &[]);
        assert_eq!(bus.chip.offset_millis(), None);
        assert_eq!(bus.read(0, 1), [0x56]);
    }

    #[test]
    fn a_new_start_ends_the_write_before_it() {
        let bus = Bus::at(december());
        bus.start();
        bus.send(ADDRESS << 1);
        bus.send(7);
        bus.send(0x55);
        bus.start();
        bus.send(ADDRESS << 1);
        bus.send(7);
        bus.stop();
        assert_eq!(bus.read(7, 1), [0x55]);
    }

    #[test]
    fn values_out_of_range_are_clamped() {
        let bus = Bus::at(december());
        // 31 April, 25:61:61.
        bus.write(0, &[0x61, 0x61, 0x25, 1, 0x31, 0x04, 0x26]);
        // The weekday is the one written, which has nothing to do with the date.
        assert_eq!(bus.read(0, 7), [0x59, 0x59, 0x23, 1, 0x30, 0x04, 0x26]);
    }

    #[test]
    fn an_invalid_year_is_clamped_like_the_other_fields() {
        let bus = Bus::at(december());
        bus.write(6, &[0xFF]);
        assert_eq!(bus.read(6, 1), [0x99]);
    }

    #[test]
    fn the_century_bit_is_written_with_the_month() {
        // Set, it is the 2100s (the bit is 1 for them as the year rolls over from 99 to 00).
        let bus = Bus::at(december());
        bus.write(0, &[0x30, 0x20, 0x10, 5, 0x05, 0x91, 0x27]);
        assert_eq!(bus.read(0, 7), [0x30, 0x20, 0x10, 5, 0x05, 0x91, 0x27]);
        assert_eq!(bus.chip.time(), HostTime::new(2127, 11, 5, 10, 20, 30));
        // And clear, the 2000s again, whatever hundred years the chip was in.
        bus.write(4, &[0x05, 0x11, 0x27]);
        assert_eq!(bus.read(5, 2), [0x11, 0x27]);
        assert_eq!(bus.chip.time().year, 2027);
    }

    #[test]
    fn the_century_bit_goes_with_the_year_whichever_is_written_first() {
        let bus = Bus::at(december());
        bus.start();
        bus.send(ADDRESS << 1);
        bus.send(6);
        bus.send(0x27);
        bus.stop();
        assert_eq!(bus.chip.time().year, 2027);
        bus.write(5, &[0x95]);
        assert_eq!(bus.chip.time().year, 2127);
        // A year on its own stays in the hundred years the chip is in.
        bus.write(6, &[0x30]);
        assert_eq!(bus.chip.time().year, 2130);
    }

    #[test]
    fn the_hours_are_taken_in_12_hour_form_when_bit_6_is_set() {
        // 12-hour mode: bit 5 is PM, and the hour is 1 to 12.
        let bus = Bus::at(december());
        for (written, hour) in [
            (0x41, 1),
            (0x49, 9),
            (0x52, 0),
            (0x61, 13),
            (0x71, 23),
            (0x72, 12),
            // Without bit 6, bit 5 is the twenties.
            (0x21, 21),
        ] {
            bus.write(2, &[written]);
            assert_eq!(bus.chip.time().hour, hour, "{written:#x}");
        }
    }

    #[test]
    fn a_chip_written_in_12_hour_form_is_read_in_it_until_written_in_24_hour_form() {
        let bus = Bus::at(december());
        // As the chip powers up: 24-hour mode.
        assert_eq!(bus.read(2, 1), [0x13]);
        // Written in 12-hour form, 1 PM reads back as it was written, and keeps counting so.
        bus.write(2, &[0x61]);
        assert_eq!(bus.read(2, 1), [0x61]);
        bus.clock.advance(3600 * 10);
        assert_eq!(bus.read(2, 1), [0x71]);
        bus.clock.advance(3600);
        assert_eq!(bus.read(2, 1), [0x52], "midnight is 12 AM");
        bus.clock.advance(3600 * 12);
        assert_eq!(bus.read(2, 1), [0x72], "noon is 12 PM");
        bus.clock.advance(3600);
        assert_eq!(bus.read(2, 1), [0x61]);
        bus.clock.advance(3600 * 11);
        // The minutes and the other registers do not change the format.
        bus.write(1, &[0x30]);
        assert_eq!(bus.read(2, 1), [0x52]);
        // Written in 24-hour form it goes back.
        bus.write(2, &[0x13]);
        assert_eq!(bus.read(2, 1), [0x13]);
    }

    #[test]
    fn the_12_hour_form_is_kept_with_the_saved_state() {
        let bus = Bus::at(december());
        bus.write(2, &[0x61]);
        let saved = bus.chip.saved().unwrap();
        let again = Bus::at(december());
        again.chip.load(saved).unwrap();
        assert_eq!(again.read(2, 1), [0x61]);
        // A chip never set but written in 12-hour form is not one as it was made.
        let bus = Bus::at(december());
        bus.chip.0.borrow_mut().hour12 = true;
        assert!(bus.chip.saved().is_some());
        bus.chip.load_new();
        assert_eq!(bus.read(2, 1), [0x13]);
        assert_eq!(bus.chip.saved(), None);
    }

    #[test]
    fn the_weekday_is_the_one_written_and_the_chip_counts_it_on_at_midnight() {
        // Thursday, which the chip has not been told: it shows the day the date says.
        let bus = Bus::at(december());
        assert_eq!(bus.read(3, 1), [4]);
        // Told it is 3 (the numbers mean what the user decided, as long as they go in order).
        bus.write(3, &[3]);
        assert_eq!(bus.read(3, 1), [3]);
        // Writing it is not setting the time.
        assert_eq!(bus.chip.offset_millis(), None);
        // The next midnight makes it 4, and 7 goes round to 1.
        bus.clock.advance(10 * 3600 + 14 * 60 + 4);
        assert_eq!(bus.read(0, 4), [0, 0, 0, 4]);
        bus.write(3, &[7]);
        bus.clock.advance(86_400);
        assert_eq!(bus.read(3, 1), [1]);
    }

    #[test]
    fn writing_the_date_or_the_time_does_not_change_the_weekday() {
        let bus = Bus::at(december());
        bus.write(3, &[2]);
        // Time of day, and then a date: neither is a weekday.
        bus.write(0, &[0x30, 0x20, 0x10]);
        assert_eq!(bus.read(3, 1), [2]);
        bus.write(4, &[0x05, 0x11, 0x27]);
        assert_eq!(bus.read(3, 1), [2]);
        // With the weekday, it is the one that is written, and the date after it does not move it.
        bus.write(3, &[6, 0x01, 0x03, 0x28]);
        assert_eq!(bus.read(3, 4), [6, 0x01, 0x03, 0x28]);
        // A date in the order the registers are in, with the weekday last of all.
        bus.write(4, &[0x02, 0x03, 0x28]);
        bus.write(3, &[5]);
        assert_eq!(bus.read(3, 4), [5, 0x02, 0x03, 0x28]);
    }

    #[test]
    fn the_weekday_of_a_chip_never_set_follows_the_hosts_days_and_the_chip_can_be_told_it() {
        let bus = Bus::at(december());
        bus.write(3, &[1]);
        bus.clock.advance(86_400);
        assert_eq!(bus.read(3, 1), [2]);
        assert_eq!(bus.chip.offset_millis(), None);
    }

    #[test]
    fn only_three_bits_of_the_weekday_are_taken_and_0_is_1() {
        let bus = Bus::at(december());
        bus.write(3, &[0xFA]);
        assert_eq!(bus.read(3, 1), [2]);
        bus.write(3, &[0]);
        assert_eq!(bus.read(3, 1), [1]);
    }

    #[test]
    fn the_weekday_is_kept_with_the_saved_state() {
        let bus = Bus::at(december());
        bus.write(3, &[2]);
        let saved = bus
            .chip
            .saved()
            .expect("a chip with a weekday is not as it was made");
        let again = Bus::at(december());
        again.chip.load(saved).unwrap();
        assert_eq!(again.read(3, 1), [2]);
        again.clock.advance(86_400);
        assert_eq!(again.read(3, 1), [3]);
        again.chip.load_new();
        assert_eq!(again.read(3, 1), [5]);
    }

    #[test]
    fn a_month_that_does_not_have_the_day_gives_the_last_day_of_it() {
        // 31 January and then the month 2: the datasheet says that is undefined. The chip here
        // takes the last day of the month, so that it always shows a date there is.
        let bus = Bus::at(HostTime::new(2027, 1, 31, 12, 0, 0));
        bus.write(5, &[0x02]);
        assert_eq!(bus.read(4, 3), [0x28, 0x02, 0x27]);
    }

    #[test]
    fn the_offset_can_be_set_from_outside() {
        let bus = Bus::at(december());
        bus.chip.set_offset_millis(Some(3600 * 1000)).unwrap();
        assert_eq!(bus.read(0, 3), [0x56, 0x45, 0x14]);
        bus.chip.set_offset_millis(Some(-86_400 * 1000)).unwrap();
        assert_eq!(bus.read(4, 1), [0x23]);
    }

    #[test]
    fn the_time_is_the_hosts_with_the_offset() {
        let bus = Bus::at(december());
        assert_eq!(bus.chip.time(), december());
        bus.chip.set_offset_millis(Some((3600 + 4) * 1000)).unwrap();
        assert_eq!(bus.chip.time(), HostTime::new(2026, 12, 24, 14, 46, 0));
        bus.clock.advance(86_400);
        assert_eq!(bus.chip.time(), HostTime::new(2026, 12, 25, 14, 46, 0));
    }

    #[test]
    fn a_new_host_clock_keeps_the_offset() {
        let bus = Bus::at(december());
        bus.write(0, &[0x00, 0x00, 0x10]);
        let offset = bus.chip.offset_millis();
        bus.chip
            .set_host_clock(Box::new(FixedClock::new(december())));
        assert_eq!(bus.chip.offset_millis(), offset);
        assert_eq!(bus.read(0, 3), [0x00, 0x00, 0x10]);
    }

    #[test]
    fn a_chip_with_no_host_clock_set_starts_at_the_year_2000() {
        let chip = Ds3231Slot::default();
        let bus = Bus {
            chip,
            clock: FixedClock::default(),
        };
        assert_eq!(bus.read(0, 7), [0, 0, 0, 6, 1, 1, 0]);
    }

    #[test]
    fn a_read_that_wraps_to_the_seconds_takes_the_time_again() {
        let bus = Bus::at(december());
        bus.start();
        bus.send(ADDRESS << 1);
        bus.send(0x11);
        bus.stop();
        bus.start();
        bus.send(ADDRESS << 1 | 1);
        bus.clock.advance(1);
        assert_eq!(bus.receive(true), 25);
        assert_eq!(bus.receive(true), 0);
        // The registers read at START said 56 seconds; the wrap took the time again.
        assert_eq!(bus.receive(false), 0x57);
        bus.stop();
    }

    #[test]
    fn the_status_flags_can_only_be_cleared() {
        let bus = Bus::at(december());
        assert_eq!(bus.read(0x0F, 1), [0x88]);
        // Writing ones changes nothing: OSF stays, BSY is the chip's.
        bus.write(0x0F, &[0xFF]);
        assert_eq!(bus.read(0x0F, 1), [0x88]);
        bus.write(0x0F, &[0x08]);
        assert_eq!(bus.read(0x0F, 1), [0x08]);
        // Cleared, it stays cleared whatever is written next.
        bus.write(0x0F, &[0x80]);
        assert_eq!(bus.read(0x0F, 1), [0x00]);
        // The 32 kHz output is a switch.
        bus.write(0x0F, &[0x08]);
        assert_eq!(bus.read(0x0F, 1), [0x08]);
        bus.write(0x0F, &[0x04]);
        assert_eq!(bus.read(0x0F, 1), [0x00]);
    }

    // Thursday 2026-03-29 00:59:59 local, an hour before the clocks go forward in the UK.
    fn before_spring_forward() -> HostTime {
        HostTime::new(2026, 3, 29, 0, 59, 59)
    }

    #[test]
    fn a_chip_never_set_follows_the_hosts_local_time_through_a_clock_change() {
        let bus = Bus::at(before_spring_forward());
        bus.clock.advance(1);
        bus.clock.set_zone(3600);
        // UTC went on by a second, and the wall clock by an hour and a second.
        assert_eq!(bus.read(0, 3), [0x00, 0x00, 0x02]);
        assert_eq!(bus.chip.offset_millis(), None);
    }

    #[test]
    fn a_chip_that_was_set_counts_evenly_through_a_clock_change() {
        let bus = Bus::at(before_spring_forward());
        bus.write(0, &[0x00, 0x00, 0x10]);
        bus.clock.advance(2);
        bus.clock.set_zone(3600);
        // 10:00:02: the hour the host's clock skipped is not skipped.
        assert_eq!(bus.read(0, 3), [0x02, 0x00, 0x10]);
        // And the clocks going back, as in the autumn, does not repeat one.
        bus.clock.advance(3600);
        bus.clock.set_zone(0);
        assert_eq!(bus.read(0, 3), [0x02, 0x00, 0x11]);
    }

    #[test]
    fn a_chip_that_was_set_ignores_a_change_of_time_zone() {
        let bus = Bus::at(december());
        bus.write(0, &[0x00, 0x00, 0x10]);
        bus.clock.set_zone(-5 * 3600);
        assert_eq!(bus.read(0, 3), [0x00, 0x00, 0x10]);
        bus.clock.advance(60);
        assert_eq!(bus.read(0, 3), [0x00, 0x01, 0x10]);
    }

    #[test]
    fn a_chip_never_set_takes_the_date_it_shows_from_the_hosts_local_time_when_the_time_is_set() {
        // 23:00 UTC on the 24th is 01:00 on the 25th two hours ahead. Setting the hour keeps
        // the date the chip was showing.
        let bus = Bus::at(HostTime::new(2026, 12, 24, 23, 0, 0));
        bus.clock.set_zone(2 * 3600);
        bus.write(2, &[0x10]);
        assert_eq!(bus.chip.time(), HostTime::new(2026, 12, 25, 10, 0, 0));
    }

    #[test]
    fn the_year_2100_reads_as_year_00_with_the_century_bit_set() {
        let bus = Bus::at(HostTime::new(2100, 1, 1, 0, 0, 0));
        assert_eq!(bus.read(4, 3), [0x01, 0x81, 0x00]);
        let bus = Bus::at(HostTime::new(2099, 12, 31, 23, 59, 59));
        assert_eq!(bus.read(5, 2), [0x12, 0x99]);
        bus.clock.advance(1);
        assert_eq!(bus.read(5, 2), [0x81, 0x00]);
    }

    #[test]
    fn the_year_1999_reads_as_99_and_the_century_bit_of_the_year_before_2000() {
        let bus = Bus::at(HostTime::new(1999, 12, 31, 23, 59, 59));
        assert_eq!(bus.read(5, 2), [0x92, 0x99]);
        bus.clock.advance(1);
        assert_eq!(bus.read(5, 2), [0x01, 0x00]);
    }

    #[test]
    fn a_write_keeps_the_chip_in_the_hundred_years_it_is_in() {
        // The time of day and the year register do not say which hundred years it is.
        let bus = Bus::at(HostTime::new(2100, 6, 15, 12, 0, 0));
        bus.write(0, &[0x30, 0x20, 0x10]);
        assert_eq!(bus.read(4, 3), [0x15, 0x86, 0x00]);
        assert_eq!(bus.chip.time(), HostTime::new(2100, 6, 15, 10, 20, 30));
        bus.write(6, &[0x30]);
        assert_eq!(bus.chip.time(), HostTime::new(2130, 6, 15, 10, 20, 30));
        bus.write(4, &[0x01, 0x82, 0x30]);
        assert_eq!(bus.chip.time(), HostTime::new(2130, 2, 1, 10, 20, 30));
        let bus = Bus::at(HostTime::new(1999, 6, 15, 12, 0, 0));
        bus.write(6, &[0x98]);
        assert_eq!(bus.chip.time().year, 1998);
    }

    #[test]
    fn the_date_arithmetic_is_right_through_2199() {
        for year in [2100, 2104, 2199] {
            for (month, day) in [(2, 28), (12, 31)] {
                let time = HostTime::new(year, month, day, 23, 59, 59);
                let bus = Bus::at(time);
                bus.clock.advance(1);
                let (y, m, d) = civil_from_days(days_from_civil(year.into(), month, day) + 1);
                let shown = bus.chip.time();
                assert_eq!((i64::from(shown.year), shown.month, shown.day), (y, m, d));
                assert_eq!(bus.read(4, 1), [bcd(d)]);
            }
        }
        assert_eq!(days_in_month(2100, 2), 28);
        assert_eq!(days_in_month(2104, 2), 29);
    }

    #[test]
    fn nothing_is_saved_for_a_chip_as_it_was_made() {
        assert_eq!(Ds3231Slot::default().saved(), None);
    }

    #[test]
    fn a_chip_that_was_set_saves_its_offset_and_a_new_one_takes_it_back() {
        let bus = Bus::at(december());
        bus.write(0, &[0x30, 0x20, 0x10]);
        let saved = bus.chip.saved().unwrap();
        assert_eq!(saved.len(), STATE_SIZE);
        assert_eq!(saved[..8], bus.chip.offset_millis().unwrap().to_le_bytes());
        assert_eq!(saved[8], 1);
        assert_eq!(saved[9], 0x88);

        let again = Bus::at(december());
        again.chip.load(saved).unwrap();
        assert_eq!(again.read(0, 3), [0x30, 0x20, 0x10]);
        assert_eq!(again.chip.saved(), Some(saved));
    }

    #[test]
    fn a_chip_with_the_status_cleared_but_the_time_never_set_is_saved_too() {
        let bus = Bus::at(december());
        bus.write(0x0F, &[0]);
        let saved = bus.chip.saved().unwrap();
        assert_eq!(saved[8], 0);
        let again = Bus::at(december());
        again.chip.load(saved).unwrap();
        assert_eq!(again.chip.offset_millis(), None);
        assert_eq!(again.read(0x0F, 1), [0]);
        // Following the host's local time, as it was.
        again.clock.set_zone(3600);
        assert_eq!(again.read(2, 1), [0x14]);
    }

    #[test]
    fn a_new_chip_state_puts_back_the_chip_as_it_was_made() {
        let bus = Bus::at(december());
        bus.write(0, &[0x30, 0x20, 0x10]);
        bus.write(0x0F, &[0]);
        bus.chip.load_new();
        assert_eq!(bus.chip.saved(), None);
        assert_eq!(bus.read(0, 3), [0x56, 0x45, 0x13]);
    }

    #[test]
    fn busy_and_unknown_bits_of_a_saved_status_are_not_kept() {
        let bus = Bus::at(december());
        let mut saved = [0; STATE_SIZE];
        saved[9] = 0xFF;
        bus.chip.load(saved).unwrap();
        assert_eq!(bus.read(0x0F, 1), [0x8B]);
    }

    #[test]
    fn an_offset_of_more_than_200_years_is_refused_and_leaves_the_chip_alone() {
        let bus = Bus::at(december());
        bus.write(0, &[0x30, 0x20, 0x10]);
        let before = bus.chip.saved();
        let too_far = 201 * 366 * 86_400 * 1000;
        for millis in [too_far, -too_far, i64::MAX, i64::MIN] {
            assert_eq!(
                bus.chip.set_offset_millis(Some(millis)),
                Err(OffsetError { millis })
            );
            let mut saved = [0; STATE_SIZE];
            saved[..8].copy_from_slice(&millis.to_le_bytes());
            saved[8] = 1;
            assert_eq!(bus.chip.load(saved), Err(OffsetError { millis }));
            assert_eq!(bus.chip.saved(), before);
        }
        // The same number in a chip never set is not an offset.
        let mut saved = [0; STATE_SIZE];
        saved[..8].copy_from_slice(&i64::MAX.to_le_bytes());
        assert_eq!(bus.chip.load(saved), Ok(()));
        assert_eq!(bus.chip.offset_millis(), None);
        // 199 years is fine, and the chip can be read without a panic.
        bus.chip
            .set_offset_millis(Some(199 * 365 * 86_400 * 1000))
            .unwrap();
        assert!(bus.chip.time().year > 2200);
        assert!(
            OffsetError {
                millis: 1000 * 1000
            }
            .to_string()
            .contains("200 years")
        );
    }

    #[test]
    fn an_offset_of_an_earlier_version_that_the_zone_takes_over_the_cap_changes_nothing() {
        // The seconds are within the cap, and with the host's two hours added they are not.
        let bus = Bus::at(december());
        bus.write(0, &[0x30, 0x20, 0x10]);
        bus.write(0x0F, &[0]);
        bus.clock.set_zone(2 * 3600);
        let before = (bus.chip.saved(), bus.chip.offset_millis());
        let seconds = MAX_OFFSET_MILLIS / 1000 - 3600;
        assert!(bus.chip.load_v1(seconds).is_err());
        assert_eq!((bus.chip.saved(), bus.chip.offset_millis()), before);
        // And the status flags it would have reset are as they were.
        assert_eq!(bus.read(0x0F, 1), [0]);
    }

    #[test]
    fn a_write_cannot_take_the_clock_further_from_the_host_than_the_cap() {
        // The chip is in the 2220s, as far as it may go, and the year goes to 2299.
        let bus = Bus::at(december());
        bus.chip
            .set_offset_millis(Some(MAX_OFFSET_MILLIS - 1000))
            .unwrap();
        assert_eq!(bus.chip.time().year / 100, 22);
        bus.write(6, &[0x99]);
        assert_eq!(bus.chip.offset_millis(), Some(MAX_OFFSET_MILLIS));
        // What it saves is what it can load.
        let saved = bus.chip.saved().unwrap();
        Bus::at(december()).chip.load(saved).unwrap();
        // The same the other way, in the 1820s.
        bus.chip
            .set_offset_millis(Some(-MAX_OFFSET_MILLIS + 1000))
            .unwrap();
        assert_eq!(bus.chip.time().year / 100, 18);
        bus.write(6, &[0x00]);
        assert_eq!(bus.chip.offset_millis(), Some(-MAX_OFFSET_MILLIS));
        let saved = bus.chip.saved().unwrap();
        Bus::at(december()).chip.load(saved).unwrap();
    }

    #[test]
    fn the_offset_of_an_earlier_version_is_kept_as_the_time_it_meant() {
        // It was seconds from the host's local time. The host is 2 hours ahead of UTC.
        let bus = Bus::at(december());
        bus.clock.set_zone(2 * 3600);
        bus.chip.load_v1(-3600).unwrap();
        assert_eq!(bus.chip.time(), HostTime::new(2026, 12, 24, 14, 45, 56));
        assert_eq!(bus.chip.offset_millis(), Some(3_600_000));
        // And then it is a clock of its own, whatever the zone does.
        bus.clock.set_zone(0);
        assert_eq!(bus.chip.time(), HostTime::new(2026, 12, 24, 14, 45, 56));
        // 0 was a chip never set.
        bus.chip.load_v1(0).unwrap();
        assert_eq!(bus.chip.offset_millis(), None);
        assert_eq!(bus.chip.saved(), None);
        assert!(bus.chip.load_v1(i64::MAX).is_err());
        assert_eq!(bus.chip.offset_millis(), None);
    }
}
