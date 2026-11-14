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
mod tests;
