use super::calendar::{
    bcd, chip_civil_from_days, chip_days_from_civil, chip_days_in_month, civil_from_days,
    days_from_civil,
};
use super::chip::{ADDRESS, Phase};
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
    assert_eq!(chip_days_in_month(2000, 2), 29);
    assert_eq!(
        chip_days_in_month(2100, 2),
        29,
        "the chip's rule is every 4th year"
    );
    assert_eq!(chip_days_in_month(2026, 2), 28);
    assert_eq!(chip_days_in_month(2026, 12), 31);
    assert_eq!(chip_days_in_month(2026, 11), 30);
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
    // 0 is 2000 to 2099 and 1 is 2100 to 2199, as the chip's bit is set after its first
    // rollover from 99 to 00.
    let bus = Bus::at(december());
    bus.write(0, &[0x30, 0x20, 0x10, 5, 0x05, 0x91, 0x27]);
    assert_eq!(bus.read(0, 7), [0x30, 0x20, 0x10, 5, 0x05, 0x91, 0x27]);
    assert_eq!(bus.chip.time(), HostTime::new(2127, 11, 5, 10, 20, 30));
    // And clear, whatever hundred years the chip was in.
    bus.write(4, &[0x05, 0x11, 0x27]);
    assert_eq!(bus.chip.time().year, 2027);
    let bus = Bus::at(HostTime::new(2100, 6, 15, 12, 0, 0));
    bus.write(4, &[0x05, 0x11, 0x27]);
    assert_eq!(bus.read(5, 2), [0x11, 0x27]);
    assert_eq!(bus.chip.time().year, 2027);
}

#[test]
fn the_month_written_back_as_it_was_read_changes_nothing_in_the_years_the_chip_has() {
    for year in [2000, 2026, 2099, 2100, 2150, 2199] {
        let time = HostTime::new(year, 6, 9, 12, 0, 0);
        let bus = Bus::at(time);
        let registers = bus.read(4, 3);
        bus.write(5, &registers[1..2]);
        assert_eq!(bus.chip.time(), time, "{year}");
        bus.write(4, &registers);
        assert_eq!(bus.chip.time(), time, "{year}");
    }
}

#[test]
fn a_date_written_in_the_2000s_after_the_chip_rolled_over_to_the_2100s_is_in_the_2000s() {
    // The firmware's DATE$ writes bit 0.
    let bus = Bus::at(december());
    bus.write(0, &[0x58, 0x59, 0x23]);
    bus.write(3, &[2, 0x31, 0x12, 0x99]);
    bus.clock.advance(5);
    assert_eq!(bus.read(4, 3), [0x01, 0x81, 0x00]);
    bus.write(3, &[2, 0x01, 0x01, 0x30]);
    assert_eq!(bus.read(4, 3), [0x01, 0x01, 0x30]);
    assert_eq!(bus.chip.time().year, 2030);
}

#[test]
fn a_chip_that_was_set_counts_29_february_2100_as_the_chip_does() {
    let bus = Bus::at(december());
    // 28 February 2100, in the 2100s.
    bus.write(0, &[0x00, 0x00, 0x12]);
    bus.write(4, &[0x28, 0x82, 0x00]);
    bus.clock.advance(86_400);
    assert_eq!(bus.read(4, 3), [0x29, 0x82, 0x00]);
    bus.clock.advance(86_400);
    assert_eq!(bus.read(4, 3), [0x01, 0x83, 0x00]);
    // A day later it is the day after that, and 2104 is a leap year too.
    bus.clock.advance(86_400);
    assert_eq!(bus.read(4, 3), [0x02, 0x83, 0x00]);
    // Written, 29/02/00 is a date.
    bus.write(4, &[0x29, 0x82, 0x00]);
    assert_eq!(bus.read(4, 3), [0x29, 0x82, 0x00]);
    assert_eq!(bus.chip.time(), HostTime::new(2100, 2, 29, 12, 0, 0));
    // The weekday counted through it.
    bus.write(3, &[1]);
    bus.clock.advance(86_400);
    assert_eq!(bus.read(3, 1), [2]);
    assert_eq!(bus.read(4, 3), [0x01, 0x83, 0x00]);
}

#[test]
fn the_chips_calendar_is_the_hosts_until_2100_and_counts_one_day_more_after() {
    for days in [-800, 0, 11_016, 20_811, 47_540] {
        assert_eq!(chip_civil_from_days(days), civil_from_days(days), "{days}");
    }
    for (year, month, day) in [
        (2100, 2, 28),
        (2100, 2, 29),
        (2100, 3, 1),
        (2200, 2, 29),
        (2200, 3, 1),
        (2201, 12, 31),
        (2400, 2, 29),
        (2400, 3, 1),
        (2401, 1, 1),
        (2500, 3, 1),
    ] {
        let days = chip_days_from_civil(year, month, day);
        assert_eq!(chip_civil_from_days(days), (year, month, day));
    }
    assert_eq!(
        chip_days_from_civil(2100, 3, 1),
        chip_days_from_civil(2100, 2, 28) + 2
    );
    // 2400 is a leap year for both, so it adds nothing.
    assert_eq!(
        chip_days_from_civil(2400, 3, 1) - chip_days_from_civil(2400, 2, 29),
        1
    );
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
    assert_eq!(chip_days_in_month(2104, 2), 29);
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
fn a_write_that_is_clamped_leaves_the_weekday_as_it_was() {
    let bus = Bus::at(december());
    bus.chip
        .set_offset_millis(Some(MAX_OFFSET_MILLIS - 1000))
        .unwrap();
    bus.write(3, &[3]);
    // The day it is clamped to is not the day written, and the weekday is not set from that.
    for year in [0x99, 0x98, 0x97, 0x96, 0x95, 0x94, 0x93] {
        bus.write(6, &[year]);
        assert_eq!(bus.read(3, 1), [3], "{year:#x}");
    }
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
fn the_offset_of_the_first_version_past_2100_is_the_date_it_showed() {
    // It counted in Gregorian days: 2100-03-05 is the day after 4 March, and the chip's
    // calendar has a 29 February before it, so the chip shows the same date by being a day
    // on in its days.
    let bus = Bus::at(december());
    let shown = HostTime::new(2100, 3, 5, 12, 0, 0);
    let seconds = shown.seconds() - bus.clock.now().local.seconds();
    bus.chip.load_v1(seconds).unwrap();
    assert_eq!(bus.chip.time(), shown);
    bus.clock.advance(86_400);
    assert_eq!(bus.chip.time(), HostTime::new(2100, 3, 6, 12, 0, 0));
    // Before 2100 it is as it was.
    let bus = Bus::at(december());
    bus.chip.load_v1(3600).unwrap();
    assert_eq!(bus.chip.time(), HostTime::new(2026, 12, 24, 14, 45, 56));
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
