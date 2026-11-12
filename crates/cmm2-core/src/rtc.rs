//! The real-time clock. Nothing counts: the firmware only walks the initialisation
//! handshake and waits for the shadow registers, so the flags it waits on settle at once.

use crate::peripheral::StubRegisters;
use crate::registers::{Registers, WordAccess};
use crate::svd::Svd;

pub const RTC_BASE: u32 = 0x5800_4000;
pub const RTC_SIZE: u32 = 0x400;

// Register offset and bit positions, as the SVD gives them.
const ISR: u32 = 0x0C;

const ALRAWF: u32 = 1 << 0;
const ALRBWF: u32 = 1 << 1;
const WUTWF: u32 = 1 << 2;
const RSF: u32 = 1 << 5;
const INITF: u32 = 1 << 6;
const INIT: u32 = 1 << 7;

/// Plain registers with the SVD reset values, except for RTC_ISR, which is worked out on
/// every read. Only INIT is stored.
///
/// The alarm and wake-up registers are always writable (nothing is enabled) and the shadow
/// registers are always in sync, so those flags read as set and a write of 0 to RSF, which
/// the HAL does before it waits, has no visible effect. The shift, calendar-initialised and
/// event flags read 0: nothing ever sets them.
///
/// At rest RTC_ISR reads 0x27. The real board reads 0 at the prompt, because by then the
/// firmware has finished with the internal clock and uses the external one.
#[derive(Debug)]
pub struct RtcRegisters {
    stored: StubRegisters,
}

impl Registers for RtcRegisters {
    fn read(&mut self, offset: u32) -> u32 {
        let value = self.stored.read(offset);
        match offset {
            ISR => {
                let init = value & INIT;
                let initf = if init != 0 { INITF } else { 0 };
                ALRAWF | ALRBWF | WUTWF | RSF | initf | init
            }
            _ => value,
        }
    }

    fn write(&mut self, offset: u32, value: u32, mask: u32) {
        let value = if offset == ISR { value & INIT } else { value };
        self.stored.write(offset, value, mask);
    }
}

pub type Rtc = WordAccess<RtcRegisters>;

impl Rtc {
    pub fn new() -> Rtc {
        WordAccess::wrap(RtcRegisters {
            stored: StubRegisters::with_reset_values(RTC_BASE, Svd::stm32h743()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryMap;

    const PRER: u32 = RTC_BASE + 0x10;
    const CR: u32 = RTC_BASE + 0x08;
    const WPR: u32 = RTC_BASE + 0x24;
    const ISR_ADDR: u32 = RTC_BASE + ISR;

    fn machine() -> MemoryMap {
        let mut m = MemoryMap::new();
        m.map_peripheral(RTC_BASE, RTC_SIZE, Box::new(Rtc::new()))
            .unwrap();
        m
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
        assert_eq!(m.read32(ISR_ADDR), 0xE7);
        m.write32(ISR_ADDR, 0);
        assert_eq!(m.read32(ISR_ADDR), 0x27);
    }

    #[test]
    fn init_by_byte_and_halfword_writes() {
        let mut m = machine();
        m.write8(ISR_ADDR, 0x80);
        assert_eq!(m.read32(ISR_ADDR), 0xE7);
        m.write16(ISR_ADDR, 0);
        assert_eq!(m.read32(ISR_ADDR), 0x27);
        m.write16(ISR_ADDR, 0x80);
        assert_eq!(m.read32(ISR_ADDR), 0xE7);
    }

    #[test]
    fn initf_cannot_be_written() {
        let mut m = machine();
        m.write32(ISR_ADDR, 0x40);
        assert_eq!(m.read32(ISR_ADDR), 0x27);
    }

    #[test]
    fn rsf_cannot_be_cleared() {
        let mut m = machine();
        m.write32(ISR_ADDR, 0xFFFF_FF5F);
        assert_eq!(m.read32(ISR_ADDR) & RSF, RSF);
        // Bit 7 of the HAL's pattern is clear, so INIT and INITF are too.
        assert_eq!(m.read32(ISR_ADDR) & (INIT | INITF), 0);
        m.write32(ISR_ADDR, 0xFFFF_FFDF);
        assert_eq!(m.read32(ISR_ADDR) & (INIT | INITF), INIT | INITF);
        assert_eq!(m.read32(ISR_ADDR) & RSF, RSF);
        m.write32(ISR_ADDR, 0);
        assert_eq!(m.read32(ISR_ADDR) & RSF, RSF);
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
}
