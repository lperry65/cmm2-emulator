//! The real-time clock. The calendar and the sub-second counter run off the machine's clock,
//! from the time and date the firmware writes in initialisation mode, so the firmware's wait
//! for the seconds to change ends.
//!
//! The model is instantaneous: the counters are worked out from the clock whenever a register
//! is read. Only the 24-hour format is modelled. The alarm, wake-up, time stamp, calibration
//! and other registers are plain stored registers, and no RTC interrupt is raised.
//!
//! The RTC is in the backup domain, so a system reset leaves it alone and the time keeps
//! counting through it: the time base is an absolute cycle count, which a reset does not
//! change.

use std::cell::RefCell;
use std::rc::Rc;

use crate::clock::{CORE_HZ, Clock};
use crate::peripheral::{Peripheral, ResetCause, StubRegisters};
use crate::registers::{Registers, WordAccess};
use crate::svd::Svd;

pub const RTC_BASE: u32 = 0x5800_4000;
pub const RTC_SIZE: u32 = 0x400;

// Register offsets and bit positions, as the SVD gives them.
const TR: u32 = 0x00;
const DR: u32 = 0x04;
const CR: u32 = 0x08;
const ISR: u32 = 0x0C;
const PRER: u32 = 0x10;
const SSR: u32 = 0x28;

const BYPSHAD: u32 = 1 << 5;
const WUTE: u32 = 1 << 10;
const ALRBE: u32 = 1 << 9;
const ALRAE: u32 = 1 << 8;
const ALRAWF: u32 = 1 << 0;
const ALRBWF: u32 = 1 << 1;
const WUTWF: u32 = 1 << 2;
const INITS: u32 = 1 << 4;
const RSF: u32 = 1 << 5;
const INITF: u32 = 1 << 6;
const INIT: u32 = 1 << 7;

// The bits that exist. TR has a PM bit (22) as well as the hour, minute and second digits. Only
// the 24-hour format is modelled, so the bit is kept as written and never read as AM or PM.
// The rest is reserved.
const TR_MASK: u32 = 0x007F_7F7F;
const DR_MASK: u32 = 0x00FF_FF3F;
const PRER_MASK: u32 = 0x007F_7FFF;

/// The RTC runs from the 32.768 kHz crystal.
const RTC_HZ: u128 = 32_768;

fn bcd_pack(n: u32) -> u32 {
    ((n / 10) << 4) | (n % 10)
}

/// A digit above 9 is not BCD; it is read as it stands (0x1F is 25) and the caller clamps.
fn bcd_unpack(bcd: u32) -> u32 {
    (bcd >> 4 & 0xF) * 10 + (bcd & 0xF)
}

fn is_leap(year: u32) -> bool {
    // The years run 2000 to 2099, so the century rule never applies.
    year.is_multiple_of(4)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// A calendar time in plain numbers: the year is 0 to 99, the weekday 1 (Monday) to 7 (Sunday).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DateTime {
    year: u32,
    month: u32,
    day: u32,
    weekday: u32,
    hours: u32,
    minutes: u32,
    seconds: u32,
    /// The PM bit of TR. Only the 24-hour format is modelled, so it means nothing to the
    /// counting; it is carried so that what the guest wrote to it reads back.
    pm: bool,
}

impl DateTime {
    /// A field that is out of range (month 13, hour 25, day 0) is clamped to the nearest valid
    /// value; a day is clamped to the length of its month.
    fn from_registers(tr: u32, dr: u32) -> DateTime {
        let year = bcd_unpack(dr >> 16 & 0xFF).min(99);
        let month = bcd_unpack(dr >> 8 & 0x1F).clamp(1, 12);
        DateTime {
            year,
            month,
            day: bcd_unpack(dr & 0x3F).clamp(1, days_in_month(year, month)),
            weekday: (dr >> 13 & 7).max(1),
            hours: bcd_unpack(tr >> 16 & 0x3F).min(23),
            minutes: bcd_unpack(tr >> 8 & 0x7F).min(59),
            seconds: bcd_unpack(tr & 0x7F).min(59),
            pm: tr >> 22 & 1 != 0,
        }
    }

    /// The TR and DR values.
    fn to_registers(self) -> (u32, u32) {
        let tr = u32::from(self.pm) << 22
            | bcd_pack(self.hours) << 16
            | bcd_pack(self.minutes) << 8
            | bcd_pack(self.seconds);
        let dr = bcd_pack(self.year) << 16
            | self.weekday << 13
            | bcd_pack(self.month) << 8
            | bcd_pack(self.day);
        (tr, dr)
    }

    fn plus_seconds(self, seconds: u64) -> DateTime {
        let total = u64::from(self.hours * 3600 + self.minutes * 60 + self.seconds) + seconds;
        let of_day = (total % 86_400) as u32;
        let days = total / 86_400;
        let mut time = DateTime {
            weekday: ((u64::from(self.weekday) - 1 + days) % 7) as u32 + 1,
            hours: of_day / 3600,
            minutes: of_day / 60 % 60,
            seconds: of_day % 60,
            ..self
        };
        for _ in 0..days {
            time.next_date();
        }
        time
    }

    // Leaves the weekday to `plus_seconds`, which moves it in one go.
    fn next_date(&mut self) {
        if self.day < days_in_month(self.year, self.month) {
            self.day += 1;
        } else if self.month < 12 {
            self.day = 1;
            self.month += 1;
        } else {
            self.day = 1;
            self.month = 1;
            self.year = (self.year + 1) % 100;
        }
    }
}

/// What reads of TR, SSR and DR return for one instant.
#[derive(Debug, Clone, Copy)]
struct Reading {
    tr: u32,
    ssr: u32,
    dr: u32,
}

/// TR, DR, PRER, CR and the other registers are stored. While INIT is clear the time is the
/// stored TR and DR as they were at `anchor` (a core cycle) plus what has elapsed since; while
/// INIT is set the counters are stopped and TR and DR are what the firmware writes. The real
/// RTC ignores writes to TR, DR and PRER outside initialisation mode, and so does this one.
///
/// Unless BYPSHAD is set, a read of TR locks the date, and a read of SSR locks the time and
/// the date, as they are then, and the read of DR lets go. RM0433 46.3.9 (p. 1915) has it that
/// a read of either SSR or TR locks the higher-order shadow registers (TR and DR for SSR, DR
/// for TR) until DR is read. So TR after TR is the current time with the date held, and SSR is
/// always the current value (a read of SSR or TR does not lock SSR), so that a guest that waits
/// on either is not stuck.
/// A read of DR with nothing locked gets the current value. With BYPSHAD set the reads are taken
/// from the counters directly and nothing is locked (46.6.3, p. 1929). A system reset lets go of
/// the lock (it resets the shadow registers, 46.3.10). The model does not have the shadow
/// registers' lag of one or two RTCCLK cycles (the manual says both, 46.3.9 and the BYPSHAD
/// description): what a read returns is the time at that read, and the clock never reads
/// incoherently with BYPSHAD set.
///
/// RTC_ISR is worked out on every read, and only INIT is stored. INITF follows INIT at once
/// (the manual has it take about 2 RTCCLK cycles, 46.3.8, p. 1914). RSF is 0 in initialization
/// mode and while BYPSHAD is set, and otherwise 1: the shadow registers are always in sync, so
/// a write of 0 to RSF, which the HAL does before it waits, is set again at once (the manual
/// has it set at the next copy, 46.6.4, p. 1931). ALRAWF and ALRBWF are set while the alarm is
/// not enabled (ALRAE, ALRBE = 0) and cleared in initialization mode, and WUTWF while the
/// wake-up timer is not enabled (WUTE = 0): 46.6.4, p. 1932. INITS is set once the year is not
/// 0. The shift and event flags read 0: nothing ever sets them. A system reset clears INIT, INITF
/// and RSF (46.6.4, p. 1930), and with BYPSHAD = 0 resets the TR and DR shadows to 0 and 0x2101
/// (46.6.1, 46.6.2) until the next copy. The model clears INIT, so INITF reads 0 afterwards, but
/// copies at once: RSF reads 1 straight after the reset and TR and DR read the live calendar (the
/// calendar itself goes on from the value it stopped at). The firmware's waits for RSF cannot tell,
/// and modelling the lag would only add a delay of two RTCCLK cycles.
///
/// TR, DR and PRER are written in initialisation mode only, as 46.6.1, 46.6.2 and 46.6.5 say.
/// PRER's two fields have to be written in separate accesses on the chip; the model takes either.
/// TR bit 22 (PM, "AM or 24-hour format" when clear) is kept as written and never changes: only
/// the 24-hour format is modelled, CR.FMT is stored and ignored.
///
/// At rest RTC_ISR reads 0x27. The real board reads 0 at the prompt, because by then the
/// firmware has finished with the internal clock and uses the external one.
#[derive(Debug)]
pub struct RtcRegisters {
    stored: StubRegisters,
    clock: Clock,
    anchor: u64,
    lock: Option<Lock>,
}

/// What the reads have locked, as it was when they did.
#[derive(Debug, Clone, Copy)]
struct Lock {
    dr: u32,
    /// Only a read of SSR locks TR.
    tr: Option<u32>,
}

impl RtcRegisters {
    fn in_init_mode(&mut self) -> bool {
        self.stored.read(ISR) & INIT != 0
    }

    /// The time now. In initialisation mode that is the stored time with SSR at its top.
    fn reading(&mut self) -> Reading {
        let (tr, dr, prer) = (
            self.stored.read(TR),
            self.stored.read(DR),
            self.stored.read(PRER),
        );
        let prediv_s = u64::from(prer & 0x7FFF);
        if self.in_init_mode() {
            return Reading {
                tr,
                ssr: prediv_s as u32,
                dr,
            };
        }
        let prediv_a = u64::from(prer >> 16 & 0x7F) + 1;
        let elapsed = u128::from(self.clock.now().saturating_sub(self.anchor));
        let ticks = (elapsed * RTC_HZ / u128::from(CORE_HZ)) as u64;
        let (tr, dr) = DateTime::from_registers(tr, dr)
            .plus_seconds(ticks / (prediv_a * (prediv_s + 1)))
            .to_registers();
        Reading {
            tr,
            ssr: (prediv_s - ticks / prediv_a % (prediv_s + 1)) as u32,
            dr,
        }
    }

    /// TR and DR now, as `reading` has them, for the front end. Nothing is locked, so it
    /// leaves what the guest sees alone.
    fn calendar(&mut self) -> [u32; 2] {
        let now = self.reading();
        [now.tr, now.dr]
    }

    /// Sets the time and date as the firmware does, in initialisation mode, and starts the
    /// counters from them.
    fn set_calendar(&mut self, [tr, dr]: [u32; 2]) {
        self.write_isr(INIT, u32::MAX);
        self.write(TR, tr, u32::MAX);
        self.write(DR, dr, u32::MAX);
        self.write_isr(0, u32::MAX);
    }

    // Moves INIT to its new value. Setting it folds the time so far into TR and DR, which
    // then stand still; clearing it starts the counters from what is stored. Either way the
    // time changes under the lock, so it is let go.
    fn write_isr(&mut self, value: u32, mask: u32) {
        let was_in_init_mode = self.in_init_mode();
        let now = self.reading();
        self.stored.write(ISR, value & INIT, mask);
        match (was_in_init_mode, self.in_init_mode()) {
            (false, true) => {
                self.stored.write(TR, now.tr, u32::MAX);
                self.stored.write(DR, now.dr, u32::MAX);
                self.lock = None;
            }
            (true, false) => {
                self.anchor = self.clock.now();
                self.lock = None;
            }
            _ => {}
        }
    }
}

impl Registers for RtcRegisters {
    // The backup domain keeps the time and the registers through a system reset, but what the
    // reads had locked is not part of that.
    // A system reset also clears INIT (so INITF and RSF with it, RM0433 46.6.4, p. 1930); the
    // counting goes on from the stopped value.
    fn reset(&mut self, _cause: ResetCause) {
        self.write_isr(0, u32::MAX);
        self.lock = None;
    }

    fn read(&mut self, offset: u32) -> u32 {
        let bypass = self.stored.read(CR) & BYPSHAD != 0;
        match offset {
            TR => {
                let now = self.reading();
                match self.lock {
                    Some(Lock { tr: Some(tr), .. }) if !bypass => tr,
                    Some(_) => now.tr,
                    None => {
                        self.lock = (!bypass).then_some(Lock {
                            dr: now.dr,
                            tr: None,
                        });
                        now.tr
                    }
                }
            }
            SSR => {
                let now = self.reading();
                if !bypass {
                    let lock = self.lock.get_or_insert(Lock {
                        dr: now.dr,
                        tr: None,
                    });
                    lock.tr.get_or_insert(now.tr);
                }
                now.ssr
            }
            DR => match self.lock.take() {
                Some(lock) if !bypass => lock.dr,
                _ => self.reading().dr,
            },
            ISR => {
                let init = self.stored.read(ISR) & INIT;
                let cr = self.stored.read(CR);
                let flag = |set: bool, bit: u32| if set { bit } else { 0 };
                flag(init == 0 && cr & ALRAE == 0, ALRAWF)
                    | flag(init == 0 && cr & ALRBE == 0, ALRBWF)
                    | flag(cr & WUTE == 0, WUTWF)
                    | flag(init == 0 && !bypass, RSF)
                    | flag(init != 0, INITF)
                    | init
                    | flag(self.reading().dr >> 16 & 0xFF != 0, INITS)
            }
            _ => self.stored.read(offset),
        }
    }

    fn write(&mut self, offset: u32, value: u32, mask: u32) {
        let writable_mask = match offset {
            ISR => return self.write_isr(value, mask),
            SSR => return,
            TR => TR_MASK,
            DR => DR_MASK,
            PRER => PRER_MASK,
            _ => u32::MAX,
        };
        if matches!(offset, TR | DR | PRER) && !self.in_init_mode() {
            return;
        }
        self.stored.write(offset, value, mask & writable_mask);
    }
}

/// The RTC as the machine holds it. It is a cheap handle: clones share the clock, so the
/// memory map serves the registers to the guest and the front end keeps one for the time and
/// date, which it reads and sets without going through the bus (and so without the trace).
#[derive(Clone, Debug)]
pub struct Rtc(Rc<RefCell<WordAccess<RtcRegisters>>>);

impl Rtc {
    /// The counters run from cycle 0 on 00:00:00 on Monday 01/01/00, the reset values, until
    /// the firmware sets the clock.
    pub fn new(clock: Clock) -> Rtc {
        Rtc(Rc::new(RefCell::new(WordAccess::wrap(RtcRegisters {
            stored: StubRegisters::with_reset_values(RTC_BASE, Svd::stm32h743()),
            clock,
            anchor: 0,
            lock: None,
        }))))
    }

    /// The time and date, `[TR, DR]`, as they are now.
    pub(crate) fn calendar(&self) -> [u32; 2] {
        self.0.borrow_mut().inner_mut().calendar()
    }

    /// Sets the time and date from `[TR, DR]`, so the clock carries on from there.
    pub(crate) fn set_calendar(&self, calendar: [u32; 2]) {
        self.0.borrow_mut().inner_mut().set_calendar(calendar);
    }
}

impl Peripheral for Rtc {
    fn read(&mut self, offset: u32, size: u8) -> u32 {
        self.0.borrow_mut().read(offset, size)
    }

    fn write(&mut self, offset: u32, size: u8, value: u32) {
        self.0.borrow_mut().write(offset, size, value);
    }

    fn reset(&mut self, cause: ResetCause) {
        self.0.borrow_mut().reset(cause);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryMap, ResetCause};

    const PRER: u32 = RTC_BASE + 0x10;
    const CR: u32 = RTC_BASE + 0x08;
    const WPR: u32 = RTC_BASE + 0x24;
    const ISR_ADDR: u32 = RTC_BASE + ISR;

    const TR_ADDR: u32 = RTC_BASE + TR;
    const DR_ADDR: u32 = RTC_BASE + DR;
    const SSR_ADDR: u32 = RTC_BASE + SSR;
    const SECOND: u64 = 480_000_000;

    fn machine() -> MemoryMap {
        let mut m = MemoryMap::new();
        m.map_peripheral(RTC_BASE, RTC_SIZE, Box::new(Rtc::new(m.clock())))
            .unwrap();
        m
    }

    // Sets the time and date in initialisation mode and starts the counters.
    fn set_clock(m: &mut MemoryMap, tr: u32, dr: u32) {
        m.write32(ISR_ADDR, INIT);
        m.write32(TR_ADDR, tr);
        m.write32(DR_ADDR, dr);
        m.write32(ISR_ADDR, 0);
    }

    fn date_time(
        year: u32,
        month: u32,
        day: u32,
        weekday: u32,
        h: u32,
        m: u32,
        s: u32,
    ) -> DateTime {
        DateTime {
            year,
            month,
            day,
            weekday,
            hours: h,
            minutes: m,
            seconds: s,
            pm: false,
        }
    }

    #[test]
    fn bcd_packs_and_unpacks() {
        assert_eq!(bcd_pack(0), 0x00);
        assert_eq!(bcd_pack(59), 0x59);
        assert_eq!(bcd_pack(7), 0x07);
        assert_eq!(bcd_unpack(0x59), 59);
        assert_eq!(bcd_unpack(0x07), 7);
        assert_eq!(bcd_unpack(0x1F), 25);
    }

    #[test]
    fn registers_round_trip() {
        let t = date_time(26, 11, 12, 4, 23, 59, 58);
        assert_eq!(t.to_registers(), (0x0023_5958, 0x0026_9112));
        assert_eq!(DateTime::from_registers(0x0023_5958, 0x0026_9112), t);
    }

    #[test]
    fn seconds_roll_over_minute_hour_and_day() {
        let t = date_time(26, 11, 12, 4, 23, 59, 59);
        assert_eq!(t.plus_seconds(0), t);
        assert_eq!(
            date_time(26, 11, 12, 4, 10, 20, 59).plus_seconds(1),
            date_time(26, 11, 12, 4, 10, 21, 0)
        );
        assert_eq!(
            date_time(26, 11, 12, 4, 10, 59, 59).plus_seconds(1),
            date_time(26, 11, 12, 4, 11, 0, 0)
        );
        assert_eq!(t.plus_seconds(1), date_time(26, 11, 13, 5, 0, 0, 0));
    }

    #[test]
    fn months_have_30_or_31_days() {
        assert_eq!(
            date_time(26, 4, 30, 4, 23, 59, 59).plus_seconds(1),
            date_time(26, 5, 1, 5, 0, 0, 0)
        );
        assert_eq!(
            date_time(26, 5, 31, 7, 23, 59, 59).plus_seconds(1),
            date_time(26, 6, 1, 1, 0, 0, 0)
        );
        assert_eq!(
            date_time(26, 5, 30, 6, 23, 59, 59).plus_seconds(1),
            date_time(26, 5, 31, 7, 0, 0, 0)
        );
    }

    #[test]
    fn february_in_leap_and_other_years() {
        let day = 86_400;
        let leap = date_time(24, 2, 28, 3, 12, 0, 0);
        assert_eq!(leap.plus_seconds(day), date_time(24, 2, 29, 4, 12, 0, 0));
        assert_eq!(leap.plus_seconds(2 * day), date_time(24, 3, 1, 5, 12, 0, 0));
        let other = date_time(23, 2, 28, 2, 12, 0, 0);
        assert_eq!(other.plus_seconds(day), date_time(23, 3, 1, 3, 12, 0, 0));
        // 2000 was a leap year too.
        assert_eq!(
            date_time(0, 2, 28, 1, 0, 0, 0).plus_seconds(day),
            date_time(0, 2, 29, 2, 0, 0, 0)
        );
    }

    #[test]
    fn the_year_wraps_from_99_to_00() {
        assert_eq!(
            date_time(99, 12, 31, 5, 23, 59, 59).plus_seconds(1),
            date_time(0, 1, 1, 6, 0, 0, 0)
        );
    }

    #[test]
    fn the_weekday_wraps_from_sunday_to_monday() {
        assert_eq!(
            date_time(26, 11, 15, 7, 23, 59, 59).plus_seconds(1),
            date_time(26, 11, 16, 1, 0, 0, 0)
        );
    }

    #[test]
    fn a_jump_of_many_days() {
        // 2000 had 366 days and 2001 had 365, so day 1000 is the 270th of 2002.
        let t = date_time(0, 1, 1, 1, 0, 0, 0).plus_seconds(1000 * 86_400 + 61);
        assert_eq!(t, date_time(2, 9, 27, 7, 0, 1, 1));
    }

    #[test]
    fn invalid_digits_are_clamped() {
        // Hours 35 and digits above 9 in minutes and seconds, day 0, month 13, year 165.
        let t = DateTime::from_registers(0x0075_9A9F, 0x00FF_0D00);
        assert_eq!(
            t,
            DateTime {
                pm: true,
                ..date_time(99, 12, 1, 1, 23, 20, 25)
            }
        );
        // Day 31 of February.
        let t = DateTime::from_registers(0, 0x0025_0231);
        assert_eq!(t, date_time(25, 2, 28, 1, 0, 0, 0));
        // Weekday 0 and month 0.
        let t = DateTime::from_registers(0, 0x0025_0000);
        assert_eq!(t, date_time(25, 1, 1, 1, 0, 0, 0));
        // And they do not panic when added to.
        let _ = DateTime::from_registers(u32::MAX, u32::MAX).plus_seconds(u64::from(u32::MAX));
    }

    #[test]
    fn isr_at_rest_reads_the_always_set_flags() {
        let mut m = machine();
        assert_eq!(m.read32(ISR_ADDR), 0x27);
    }

    #[test]
    fn initf_follows_init_both_ways() {
        let mut m = machine();
        m.write32(ISR_ADDR, 0x80);
        assert_eq!(m.read32(ISR_ADDR), 0xC4);
        m.write32(ISR_ADDR, 0);
        assert_eq!(m.read32(ISR_ADDR), 0x27);
    }

    #[test]
    fn init_by_byte_and_halfword_writes() {
        let mut m = machine();
        m.write8(ISR_ADDR, 0x80);
        assert_eq!(m.read32(ISR_ADDR), 0xC4);
        m.write16(ISR_ADDR, 0);
        assert_eq!(m.read32(ISR_ADDR), 0x27);
        m.write16(ISR_ADDR, 0x80);
        assert_eq!(m.read32(ISR_ADDR), 0xC4);
    }

    #[test]
    fn initf_cannot_be_written() {
        let mut m = machine();
        m.write32(ISR_ADDR, 0x40);
        assert_eq!(m.read32(ISR_ADDR), 0x27);
    }

    #[test]
    fn rsf_is_set_again_at_once_when_the_software_clears_it() {
        // RM0433 46.6.4 (p. 1931): the software can clear RSF and the hardware sets it at the next
        // copy into the shadow registers, 2 RTCCLK cycles on (46.3.9, 46.6.3). The model copies
        // at once, so the HAL's clear and wait finds it set.
        let mut m = machine();
        m.write32(ISR_ADDR, 0xFFFF_FF5F);
        assert_eq!(m.read32(ISR_ADDR) & RSF, RSF);
        // Bit 7 of the HAL's pattern is clear, so INIT and INITF are too.
        assert_eq!(m.read32(ISR_ADDR) & (INIT | INITF), 0);
    }

    #[test]
    fn rsf_is_clear_in_init_mode_and_while_the_shadow_registers_are_bypassed() {
        // RM0433 46.6.4 (p. 1931): the hardware clears RSF in initialization mode and when
        // BYPSHAD = 1.
        let mut m = machine();
        m.write32(ISR_ADDR, INIT);
        assert_eq!(m.read32(ISR_ADDR) & RSF, 0);
        m.write32(ISR_ADDR, 0);
        assert_eq!(m.read32(ISR_ADDR) & RSF, RSF);
        m.write32(CR, BYPSHAD);
        assert_eq!(m.read32(ISR_ADDR) & RSF, 0);
        m.write32(CR, 0);
        assert_eq!(m.read32(ISR_ADDR) & RSF, RSF);
    }

    #[test]
    fn the_write_flags_say_when_the_alarms_and_the_wake_up_timer_can_be_changed() {
        // RM0433 46.6.4 (p. 1932): ALRAWF and ALRBWF are set once ALRAE and ALRBE are 0 and are
        // cleared in initialization mode; WUTWF is set once WUTE is 0. (The manual gives a delay of
        // up to 2 RTCCLK cycles, which the model does not have.)
        let mut m = machine();
        let flags = |m: &mut MemoryMap| m.read32(ISR_ADDR) & (ALRAWF | ALRBWF | WUTWF);
        assert_eq!(flags(&mut m), ALRAWF | ALRBWF | WUTWF);
        m.write32(CR, 1 << 8);
        assert_eq!(flags(&mut m), ALRBWF | WUTWF);
        m.write32(CR, 1 << 9);
        assert_eq!(flags(&mut m), ALRAWF | WUTWF);
        m.write32(CR, 1 << 10);
        assert_eq!(flags(&mut m), ALRAWF | ALRBWF);
        m.write32(CR, 0);
        m.write32(ISR_ADDR, INIT);
        assert_eq!(flags(&mut m), WUTWF);
    }

    #[test]
    fn a_system_reset_clears_init() {
        // RM0433 46.6.4 (p. 1930): a system reset clears INIT, INITF and RSF; the calendar keeps
        // counting from the value it had.
        let mut m = machine();
        set_clock(&mut m, 0x0012_0000, 0x0026_9112);
        m.clock().set(10 * SECOND);
        m.write32(ISR_ADDR, INIT);
        m.clock().set(20 * SECOND);
        m.reset_peripherals(ResetCause::Software);
        assert_eq!(m.read32(ISR_ADDR) & (INIT | INITF), 0);
        assert_eq!(m.read32(TR_ADDR), 0x0012_0010);
        m.clock().set(21 * SECOND);
        assert_eq!(m.read32(TR_ADDR), 0x0012_0011);
    }

    #[test]
    fn event_flags_and_the_bits_above_read_zero() {
        let mut m = machine();
        m.write32(ISR_ADDR, 0xFFFF_FFFF);
        assert_eq!(m.read32(ISR_ADDR) >> 8, 0);
    }

    #[test]
    fn the_svd_reset_value_of_isr_is_not_visible() {
        let mut m = machine();
        assert_ne!(m.read32(ISR_ADDR), 0x0003_FF5F);
        assert_eq!(m.read32(ISR_ADDR), 0x27);
    }

    #[test]
    fn other_registers_keep_their_reset_values_and_store_writes() {
        let mut m = machine();
        assert_eq!(m.read32(PRER), 0x007F_00FF);
        m.write32(ISR_ADDR, INIT);
        m.write32(PRER, 0x0063_0171);
        assert_eq!(m.read32(PRER), 0x0063_0171);
        m.write32(CR, 0x0000_0040);
        assert_eq!(m.read32(CR), 0x0000_0040);
        m.write32(WPR, 0xCA);
        m.write32(WPR, 0x53);
        assert_eq!(m.read32(WPR), 0x53);
    }

    #[test]
    fn window_ends_at_rtc_size() {
        let mut m = machine();
        m.write32(RTC_BASE + RTC_SIZE - 4, 5);
        assert_eq!(m.read32(RTC_BASE + RTC_SIZE - 4), 5);
        assert!(m.take_unmapped().is_empty());
    }

    #[test]
    fn a_reset_does_not_touch_the_rtc() {
        let mut m = machine();
        m.write32(ISR_ADDR, INIT);
        m.write32(PRER, 0x0063_0171);
        m.reset_peripherals(ResetCause::Software);
        assert_eq!(m.read32(PRER), 0x0063_0171);
        assert_eq!(m.read32(ISR_ADDR), 0x27);
        m.reset_peripherals(ResetCause::PowerOn);
        assert_eq!(m.read32(PRER), 0x0063_0171);
    }

    #[test]
    fn it_counts_from_power_on() {
        let mut m = machine();
        assert_eq!(m.read32(TR_ADDR), 0);
        assert_eq!(m.read32(DR_ADDR), 0x0000_2101);
        assert_eq!(m.read32(SSR_ADDR), 0x00FF);
        // The read of SSR locked the time, and DR lets go of it.
        m.read32(DR_ADDR);
        m.clock().set(SECOND);
        assert_eq!(m.read32(TR_ADDR), 0x0000_0001);
        m.read32(DR_ADDR);
        m.clock().set(61 * SECOND);
        assert_eq!(m.read32(TR_ADDR), 0x0000_0101);
    }

    #[test]
    fn ssr_counts_down_and_wraps_each_second() {
        let mut m = machine();
        m.clock().set(SECOND / 2);
        // Half a second: 16384 ticks, 128 steps of 256 ticks.
        assert_eq!(m.read32(SSR_ADDR), 0xFF - 128);
        m.clock().set(SECOND - 1);
        assert_eq!(m.read32(SSR_ADDR), 0);
        m.clock().set(SECOND);
        assert_eq!(m.read32(SSR_ADDR), 0xFF);
    }

    #[test]
    fn a_different_prescaler_gives_the_same_second() {
        let mut m = machine();
        m.write32(ISR_ADDR, INIT);
        m.write32(PRER, 0x0000_7FFF);
        m.write32(ISR_ADDR, 0);
        assert_eq!(m.read32(SSR_ADDR), 0x7FFF);
        m.clock().set(SECOND / 2);
        assert_eq!(m.read32(SSR_ADDR), 0x7FFF - 0x4000);
        assert_eq!(m.read32(TR_ADDR), 0);
        m.read32(DR_ADDR);
        m.clock().set(SECOND);
        assert_eq!(m.read32(TR_ADDR), 1);
        assert_eq!(m.read32(SSR_ADDR), 0x7FFF);
    }

    #[test]
    fn the_time_starts_from_what_was_written_in_init_mode() {
        let mut m = machine();
        m.clock().set(5 * SECOND);
        m.write32(ISR_ADDR, INIT);
        m.write32(TR_ADDR, 0x0012_0000);
        m.write32(DR_ADDR, 0x0026_9112);
        // Stopped: the stored values, SSR at its top.
        m.clock().set(9 * SECOND);
        assert_eq!(m.read32(TR_ADDR), 0x0012_0000);
        assert_eq!(m.read32(DR_ADDR), 0x0026_9112);
        assert_eq!(m.read32(SSR_ADDR), 0xFF);
        m.write32(ISR_ADDR, 0);
        assert_eq!(m.read32(TR_ADDR), 0x0012_0000);
        m.read32(DR_ADDR);
        m.clock().set(12 * SECOND);
        assert_eq!(m.read32(TR_ADDR), 0x0012_0003);
        assert_eq!(m.read32(DR_ADDR), 0x0026_9112);
    }

    #[test]
    fn the_day_rolls_over_through_the_registers() {
        let mut m = machine();
        set_clock(&mut m, 0x0023_5958, 0x0099_B231);
        m.clock().set(3 * SECOND);
        assert_eq!(m.read32(TR_ADDR), 0x0000_0001);
        assert_eq!(m.read32(DR_ADDR), 0x0000_C101);
    }

    #[test]
    fn writes_outside_init_mode_are_ignored() {
        let mut m = machine();
        m.write32(TR_ADDR, 0x0012_3456);
        m.write32(DR_ADDR, 0x0026_9112);
        m.write32(PRER, 0x0000_7FFF);
        assert_eq!(m.read32(TR_ADDR), 0);
        assert_eq!(m.read32(DR_ADDR), 0x0000_2101);
        assert_eq!(m.read32(PRER), 0x007F_00FF);
    }

    #[test]
    fn ssr_cannot_be_written() {
        let mut m = machine();
        m.write32(SSR_ADDR, 0);
        assert_eq!(m.read32(SSR_ADDR), 0xFF);
    }

    #[test]
    fn reserved_bits_of_tr_and_dr_are_not_kept() {
        let mut m = machine();
        m.write32(ISR_ADDR, INIT);
        m.write32(TR_ADDR, 0xFFFF_FFFF);
        m.write32(DR_ADDR, 0xFFFF_FFFF);
        assert_eq!(m.read32(TR_ADDR), TR_MASK);
        assert_eq!(m.read32(DR_ADDR), DR_MASK);
    }

    #[test]
    fn inits_follows_the_year() {
        let mut m = machine();
        assert_eq!(m.read32(ISR_ADDR) & INITS, 0);
        set_clock(&mut m, 0, 0x0026_9112);
        assert_eq!(m.read32(ISR_ADDR), 0x27 | INITS);
        // In init mode too, from the stored date.
        m.write32(ISR_ADDR, INIT);
        assert_eq!(m.read32(ISR_ADDR), 0xC4 | INITS);
        m.write32(DR_ADDR, 0x0000_2101);
        assert_eq!(m.read32(ISR_ADDR) & INITS, 0);
        // The year turning over from 99 to 00 clears it again.
        m.write32(DR_ADDR, 0x0099_B231);
        m.write32(TR_ADDR, 0x0023_5959);
        m.write32(ISR_ADDR, 0);
        assert_eq!(m.read32(ISR_ADDR) & INITS, INITS);
        m.clock().set(SECOND);
        assert_eq!(m.read32(ISR_ADDR) & INITS, 0);
    }

    #[test]
    fn reading_tr_holds_the_date_until_dr_is_read() {
        let mut m = machine();
        set_clock(&mut m, 0x0023_5959, 0x0026_9112);
        m.clock().set(SECOND / 2);
        assert_eq!(m.read32(TR_ADDR), 0x0023_5959);
        m.clock().set(SECOND);
        // The date as it was at the read of TR, not the new day.
        assert_eq!(m.read32(DR_ADDR), 0x0026_9112);
        // The read of DR let it go.
        assert_eq!(m.read32(DR_ADDR), 0x0026_B113);
    }

    #[test]
    fn tr_and_ssr_stay_current_after_a_read_of_tr() {
        // A guest that reads TR and then waits on either of them is not stuck.
        let mut m = machine();
        set_clock(&mut m, 0x0023_5958, 0x0026_9112);
        assert_eq!(m.read32(TR_ADDR), 0x0023_5958);
        m.clock().set(SECOND / 2);
        assert_eq!(m.read32(SSR_ADDR), 0xFF - 128);
        m.clock().set(2 * SECOND);
        assert_eq!(m.read32(SSR_ADDR), 0xFF);
        // The date is still the one TR locked, from before midnight.
        assert_eq!(m.read32(DR_ADDR), 0x0026_9112);
        assert_eq!(m.read32(TR_ADDR), 0x0000_0000);
        assert_eq!(m.read32(DR_ADDR), 0x0026_B113);
    }

    #[test]
    fn a_read_of_ssr_holds_tr_too_until_dr_is_read() {
        let mut m = machine();
        set_clock(&mut m, 0x0023_5958, 0x0026_9112);
        m.read32(SSR_ADDR);
        m.clock().set(2 * SECOND);
        assert_eq!(m.read32(TR_ADDR), 0x0023_5958);
        assert_eq!(m.read32(SSR_ADDR), 0xFF);
        assert_eq!(m.read32(DR_ADDR), 0x0026_9112);
        assert_eq!(m.read32(TR_ADDR), 0x0000_0000);
    }

    #[test]
    fn a_system_reset_lets_go_of_the_lock() {
        let mut m = machine();
        set_clock(&mut m, 0x0023_5958, 0x0026_9112);
        m.read32(SSR_ADDR);
        m.clock().set(2 * SECOND);
        m.reset_peripherals(ResetCause::Software);
        assert_eq!(m.read32(TR_ADDR), 0x0000_0000);
        assert_eq!(m.read32(DR_ADDR), 0x0026_B113);
    }

    #[test]
    fn the_pm_bit_is_kept_while_the_clock_counts() {
        let mut m = machine();
        set_clock(&mut m, 1 << 22 | 0x0011_5958, 0x0026_9112);
        m.clock().set(3 * SECOND);
        assert_eq!(m.read32(TR_ADDR), 1 << 22 | 0x0012_0001);
        m.read32(DR_ADDR);
        // And through a stop of the clock for INIT.
        m.write32(ISR_ADDR, INIT);
        assert_eq!(m.read32(TR_ADDR), 1 << 22 | 0x0012_0001);
    }

    #[test]
    fn a_read_of_ssr_locks_the_time_and_date_until_dr_is_read() {
        let mut m = machine();
        set_clock(&mut m, 0x0023_5959, 0x0026_9112);
        m.clock().set(SECOND / 2);
        assert_eq!(m.read32(SSR_ADDR), 0xFF - 128);
        m.clock().set(SECOND);
        assert_eq!(m.read32(TR_ADDR), 0x0023_5959);
        assert_eq!(m.read32(DR_ADDR), 0x0026_9112);
        assert_eq!(m.read32(DR_ADDR), 0x0026_B113);
    }

    #[test]
    fn ssr_on_its_own_stays_current_so_that_waiting_on_it_ends() {
        let mut m = machine();
        m.clock().set(SECOND / 2);
        assert_eq!(m.read32(SSR_ADDR), 0xFF - 128);
        m.clock().set(SECOND / 2 + SECOND / 4);
        assert_eq!(m.read32(SSR_ADDR), 0xFF - 192);
    }

    #[test]
    fn a_lock_does_not_survive_setting_the_clock() {
        let mut m = machine();
        set_clock(&mut m, 0x0023_5958, 0x0026_9112);
        // TR locked, DR never read; then the clock is set again.
        m.read32(TR_ADDR);
        set_clock(&mut m, 0x0001_0203, 0x0030_3231);
        assert_eq!(m.read32(DR_ADDR), 0x0030_3231);
        assert_eq!(m.read32(TR_ADDR), 0x0001_0203);
    }

    #[test]
    fn a_lock_does_not_survive_entering_init_mode() {
        let mut m = machine();
        set_clock(&mut m, 0x0023_5958, 0x0026_9112);
        m.read32(TR_ADDR);
        m.write32(ISR_ADDR, INIT);
        m.write32(DR_ADDR, 0x0030_3231);
        assert_eq!(m.read32(DR_ADDR), 0x0030_3231);
    }

    #[test]
    fn the_pm_bit_of_tr_is_kept_though_only_24_hour_format_is_modelled() {
        let mut m = machine();
        m.write32(ISR_ADDR, INIT);
        m.write32(TR_ADDR, 1 << 22);
        assert_eq!(m.read32(TR_ADDR), 1 << 22);
        assert_eq!(TR_MASK & 1 << 22, 1 << 22);
    }

    #[test]
    fn the_handle_reads_and_sets_the_calendar_off_the_bus() {
        let rtc = Rtc::new(Clock::new());
        assert_eq!(rtc.calendar(), [0, 0x0000_2101]);
        rtc.set_calendar([0x0012_0000, 0x0026_9112]);
        assert_eq!(rtc.calendar(), [0x0012_0000, 0x0026_9112]);
        // A read through it takes no lock, so what the guest reads is left alone.
        let mut guest = rtc.clone();
        guest.write(ISR, 4, 0);
        assert_eq!(guest.read(TR, 4), 0x0012_0000);
        let _ = rtc.calendar();
        assert_eq!(guest.read(TR, 4), 0x0012_0000);
    }

    #[test]
    fn without_a_read_of_tr_ssr_and_dr_are_current() {
        let mut m = machine();
        m.clock().set(SECOND / 2);
        assert_eq!(m.read32(SSR_ADDR), 0xFF - 128);
        m.clock().set(SECOND / 2 + SECOND / 4);
        assert_eq!(m.read32(SSR_ADDR), 0xFF - 192);
    }

    #[test]
    fn bypshad_makes_every_read_current() {
        let mut m = machine();
        m.write32(CR, BYPSHAD);
        m.read32(TR_ADDR);
        m.clock().set(SECOND / 2);
        assert_eq!(m.read32(SSR_ADDR), 0xFF - 128);
        m.clock().set(SECOND);
        assert_eq!(m.read32(SSR_ADDR), 0xFF);
        assert_eq!(m.read32(TR_ADDR), 1);
    }

    #[test]
    fn setting_init_again_folds_the_time_in_and_stops_it() {
        let mut m = machine();
        set_clock(&mut m, 0x0012_0000, 0x0026_9112);
        m.clock().set(90 * SECOND);
        m.write32(ISR_ADDR, INIT);
        assert_eq!(m.read32(TR_ADDR), 0x0012_0130);
        m.clock().set(200 * SECOND);
        assert_eq!(m.read32(TR_ADDR), 0x0012_0130);
        // Setting it again changes nothing.
        m.write32(ISR_ADDR, INIT);
        assert_eq!(m.read32(TR_ADDR), 0x0012_0130);
        m.write32(ISR_ADDR, 0);
        m.write32(ISR_ADDR, 0);
        m.clock().set(201 * SECOND);
        assert_eq!(m.read32(TR_ADDR), 0x0012_0131);
    }

    #[test]
    fn a_system_reset_leaves_the_time_counting() {
        let mut m = machine();
        set_clock(&mut m, 0x0012_0000, 0x0026_9112);
        m.clock().set(10 * SECOND);
        m.reset_peripherals(ResetCause::Software);
        assert_eq!(m.read32(TR_ADDR), 0x0012_0010);
        assert_eq!(m.read32(DR_ADDR), 0x0026_9112);
        assert_eq!(m.read32(PRER), 0x007F_00FF);
        m.clock().set(11 * SECOND);
        assert_eq!(m.read32(TR_ADDR), 0x0012_0011);
    }

    #[test]
    fn tr_and_dr_read_by_byte_and_halfword() {
        let mut m = machine();
        set_clock(&mut m, 0x0012_3456, 0x0026_9112);
        m.write32(CR, BYPSHAD);
        assert_eq!(m.read8(TR_ADDR), 0x56);
        assert_eq!(m.read8(TR_ADDR + 2), 0x12);
        assert_eq!(m.read16(TR_ADDR + 1), 0x1234);
        assert_eq!(m.read16(DR_ADDR + 2), 0x0026);
        assert_eq!(m.read8(DR_ADDR + 1), 0x91);
    }
}
