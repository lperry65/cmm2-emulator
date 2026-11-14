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

use std::cell::RefCell;
use std::fmt;
use std::fmt::Debug;
use std::rc::Rc;

mod calendar;
mod chip;
mod time;
use chip::{Chip, EN32KHZ, STATUS, STATUS_FLAGS};
pub use time::{FixedClock, HostClock, HostNow, HostTime};

/// The most the chip's clock may differ from the host's before a saved offset is refused:
/// 200 years.
const MAX_OFFSET_MILLIS: i64 = 200 * 366 * 86_400 * 1000;

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
        chip.civil(chip.wall_millis(&chip.clock.now()).div_euclid(1000))
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
    ///
    /// An offset saved by the build before the chip had its own calendar (the one with the weekday
    /// and the 12-hour form, e32008d) was worked out in Gregorian days and is read in the chip's
    /// now, which differs after 28 February 2100: a chip those set past it loads a day early, and
    /// the saved bytes cannot tell that build from this one (docs/ds3231.md).
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

    /// Takes the offset the first version saved: whole seconds from the host's local time, 0 for
    /// a chip never set. The time it meant is kept, now as an offset from UTC. That version
    /// counted in Gregorian days, so the date it showed now is converted to the chip's calendar
    /// exactly, which is the same until 28 February 2100.
    pub fn load_v1(&self, seconds: i64) -> Result<(), OffsetError> {
        let millis = checked(Some(seconds.saturating_mul(1000)))?;
        let mut chip = self.0.borrow_mut();
        // Both checks are made before anything of the chip is changed.
        let offset = if seconds == 0 {
            None
        } else {
            let now = chip.clock.now();
            let zone = now.local.seconds() - now.utc_millis.div_euclid(1000);
            let gregorian = checked(millis.map(|millis| millis.saturating_add(zone * 1000)))?;
            // The date it showed now, taken in the chip's calendar.
            gregorian.map_or(Ok(None), |gregorian| {
                let wall = now.utc_millis.saturating_add(gregorian);
                let time = HostTime::from_seconds(wall.div_euclid(1000));
                let target = time.chip_seconds() * 1000 + wall.rem_euclid(1000);
                checked(Some(target.saturating_sub(now.utc_millis)))
            })?
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
mod tests;
