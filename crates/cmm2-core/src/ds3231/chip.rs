//! The chip itself: the I2C slave that watches SDA and SCL, and the registers behind it.

use std::fmt::Debug;

use super::MAX_OFFSET_MILLIS;
use super::calendar::{
    CALENDAR, calendar_registers, century, chip_days_in_month, civil_weekday, register_hour, unbcd,
};
use super::time::{FixedClock, HostClock, HostNow, HostTime};

/// The chip's 7-bit address on the bus.
pub(super) const ADDRESS: u8 = 0x68;
/// Registers 0x00 to 0x12.
const REGISTERS: usize = 0x13;

const CONTROL: usize = 0x0E;
pub(super) const STATUS: usize = 0x0F;
const TEMPERATURE: usize = 0x11;

/// A new chip's status: the oscillator has stopped (OSF) and the 32 kHz output is enabled.
const STATUS_AT_POWER_UP: u8 = 0x88;
/// The flags that a write can only clear: OSF, and the two alarm flags.
pub(super) const STATUS_FLAGS: u8 = 0x83;
pub(super) const EN32KHZ: u8 = 0x08;

fn next_register(pointer: usize) -> usize {
    (pointer + 1) % REGISTERS
}

/// What the slave is doing with the clock pulses that come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
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
pub(super) struct Chip {
    pub(super) clock: Box<dyn HostClock>,
    /// The chip's own clock, in milliseconds ahead of the host's UTC time. `None` until the
    /// guest sets the time: the chip then shows the host's local time. This is what the coin
    /// cell stands for.
    pub(super) offset: Option<i64>,
    /// Registers 0x07 to 0x12 as the chip holds them. The calendar ones are worked out.
    pub(super) stored: [u8; REGISTERS],
    /// The weekday register is a count the chip keeps: it goes on a day at midnight from the
    /// value it was given, whatever the date. This is how many days that value is on from the
    /// day the date says it is (Monday is 1), 0 to 6.
    pub(super) weekday_skew: u8,
    /// The hours register was last written in 12-hour form (bit 6), and is read in it.
    pub(super) hour12: bool,
    /// The registers as a transaction reads them: the calendar is taken at START, and again
    /// when the pointer wraps round to 0, so that a read that runs over a second boundary is
    /// still consistent.
    registers: [u8; REGISTERS],
    /// The date registers (date, month, year) written in this transaction, as they were
    /// written. A date is written a register at a time, and 30 only fits a month that has 30
    /// days once the month has been written too.
    date_written: [Option<u8>; 3],
    pointer: usize,
    pub(super) phase: Phase,
    /// Clock pulses of the byte so far (1 to 9, the ninth being the acknowledge).
    pulses: u8,
    shift: u8,
    master_acknowledged: bool,
    /// The levels the master drives on the wires; the chip never changes these.
    sda: bool,
    scl: bool,
    /// The chip holds SDA low.
    pub(super) pulling_low: bool,
}

impl Default for Chip {
    fn default() -> Chip {
        Chip::new(Box::new(FixedClock::default()))
    }
}

impl Chip {
    pub(super) fn new(clock: Box<dyn HostClock>) -> Chip {
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
    pub(super) fn is_as_made(&self) -> bool {
        self.offset.is_none()
            && self.stored[STATUS] == STATUS_AT_POWER_UP
            && self.weekday_skew == 0
            && !self.hour12
    }

    /// Puts what the coin cell keeps back as it was made.
    pub(super) fn make_new(&mut self) {
        self.offset = None;
        self.stored[STATUS] = STATUS_AT_POWER_UP;
        self.weekday_skew = 0;
        self.hour12 = false;
    }

    /// The date and time of `seconds` of the chip's wall clock: the host's own calendar until the
    /// chip is set, and the chip's after.
    pub(super) fn civil(&self, seconds: i64) -> HostTime {
        if self.offset.is_some() {
            HostTime::from_chip_seconds(seconds)
        } else {
            HostTime::from_seconds(seconds)
        }
    }

    /// The chip's wall-clock time in milliseconds, as if it were UTC.
    pub(super) fn wall_millis(&self, now: &HostNow) -> i64 {
        match self.offset {
            None => now.local.seconds() * 1000 + now.utc_millis.rem_euclid(1000),
            Some(offset) => now.utc_millis.saturating_add(offset),
        }
    }

    pub(super) fn set_lines(&mut self, sda: bool, scl: bool) {
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
    pub(super) fn release_bus(&mut self) {
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
        let seconds = wall.div_euclid(1000);
        let time = self.civil(seconds);
        let calendar = calendar_registers(
            time,
            seconds.div_euclid(86_400),
            self.weekday_skew,
            self.hour12,
        );
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
        let mut time = self.civil(wall.div_euclid(1000));
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
        // The century bit goes with the month, and says which hundred years the year is in: 0 is
        // 2000 to 2099 and 1 is 2100 to 2199, the years the chip has. Without a month written the
        // chip stays in the hundred years it is in.
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
        time.day = day.min(chip_days_in_month(year, month));
        let target = time.chip_seconds() * 1000 + millis;
        // The chip must stay as far from the host as a saved offset may be, or it could not be
        // loaded again.
        let offset = target.saturating_sub(now.utc_millis);
        self.offset = Some(offset.clamp(-MAX_OFFSET_MILLIS, MAX_OFFSET_MILLIS));
        // The weekday goes on from the day the chip is on, which is not the one written if that
        // was clamped.
        self.set_weekday(self.wall_millis(&now).div_euclid(86_400_000), weekday);
    }
}
