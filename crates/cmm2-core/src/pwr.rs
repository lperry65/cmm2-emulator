//! The power controller. The supply settles instantly: the firmware only waits for the
//! ready flags, so they read as set as soon as what they wait for is enabled.

use crate::peripheral::StubRegisters;
use crate::registers::{Registers, WordAccess, follow};
use crate::svd::Svd;

pub const PWR_BASE: u32 = 0x5802_4800;
pub const PWR_SIZE: u32 = 0x400;

// Register offsets and bit positions, as the SVD gives them.
const CSR1: u32 = 0x04;
const CR2: u32 = 0x08;
const CR3: u32 = 0x0C;
const D3CR: u32 = 0x18;

const VOSRDY: u32 = 1 << 13; // PWR_D3CR
const VOS: u32 = 0b11 << 14; // PWR_D3CR
const ACTVOSRDY: u32 = 1 << 13; // PWR_CSR1
const ACTVOS: u32 = 0b11 << 14; // PWR_CSR1
const BREN: u32 = 1 << 0; // PWR_CR2
const BRRDY: u32 = 1 << 16; // PWR_CR2
const USB33DEN: u32 = 1 << 24; // PWR_CR3
const USB33RDY: u32 = 1 << 26; // PWR_CR3

/// Plain registers with the SVD reset values, except for the status bits that the hardware
/// computes. Those are worked out on every read and ignore what is written to them.
#[derive(Debug)]
pub struct PwrRegisters {
    stored: StubRegisters,
}

impl Registers for PwrRegisters {
    fn read(&mut self, offset: u32) -> u32 {
        let value = self.stored.read(offset);
        match offset {
            D3CR => value | VOSRDY,
            CSR1 => {
                let vos = self.stored.read(D3CR) & VOS;
                value & !ACTVOS | ACTVOSRDY | vos
            }
            CR2 => follow(value, BREN, BRRDY),
            CR3 => follow(value, USB33DEN, USB33RDY),
            _ => value,
        }
    }

    fn write(&mut self, offset: u32, value: u32, mask: u32) {
        // PWR_CSR1 is a status register.
        if offset != CSR1 {
            self.stored.write(offset, value, mask);
        }
    }
}

pub type Pwr = WordAccess<PwrRegisters>;

impl Pwr {
    pub fn new() -> Pwr {
        WordAccess::wrap(PwrRegisters {
            stored: StubRegisters::with_reset_values(PWR_BASE, Svd::stm32h743()),
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryMap;

    const CR1: u32 = PWR_BASE;
    const CSR1: u32 = PWR_BASE + 0x04;
    const CR2: u32 = PWR_BASE + 0x08;
    const CR3: u32 = PWR_BASE + 0x0C;
    const D3CR: u32 = PWR_BASE + 0x18;

    fn machine() -> MemoryMap {
        let mut m = MemoryMap::new();
        m.map_peripheral(PWR_BASE, PWR_SIZE, Box::new(Pwr::new()))
            .unwrap();
        m
    }

    #[test]
    fn d3cr_reports_the_voltage_ready_after_vos_is_set() {
        let mut m = machine();
        m.write32(D3CR, 0xC000);
        assert_eq!(m.read32(D3CR), 0x0000_E000);
    }

    // The reset value has VOS = 1 and VOSRDY clear; the model settles at once, so an
    // unwritten D3CR already reads ready.
    #[test]
    fn unwritten_d3cr_reads_ready_with_the_reset_vos() {
        let mut m = machine();
        assert_eq!(m.read32(D3CR), 0x0000_6000);
    }

    #[test]
    fn vosrdy_cannot_be_cleared() {
        let mut m = machine();
        m.write32(D3CR, 0);
        assert_eq!(m.read32(D3CR), 0x0000_2000);
        m.write32(D3CR, 0xFFFF_FFFF);
        assert_eq!(m.read32(D3CR) & 0x2000, 0x2000);
    }

    #[test]
    fn csr1_follows_the_vos_of_d3cr() {
        let mut m = machine();
        m.write32(D3CR, 0xC000);
        assert_eq!(m.read32(CSR1), 0x0000_E000);
        m.write32(D3CR, 0x4000);
        assert_eq!(m.read32(CSR1), 0x0000_6000);
        m.write32(D3CR, 0x8000);
        assert_eq!(m.read32(CSR1), 0x0000_A000);
    }

    #[test]
    fn unwritten_csr1_reads_ready() {
        let mut m = machine();
        assert_eq!(m.read32(CSR1), 0x0000_6000);
    }

    #[test]
    fn csr1_ignores_writes() {
        let mut m = machine();
        let before = m.read32(CSR1);
        m.write32(CSR1, 0xFFFF_FFFF);
        assert_eq!(m.read32(CSR1), before);
        m.write32(CSR1, 0);
        assert_eq!(m.read32(CSR1), before);
    }

    #[test]
    fn brrdy_follows_bren() {
        let mut m = machine();
        m.write32(CR2, 1);
        assert_eq!(m.read32(CR2), 0x0001_0001);
        m.write32(CR2, 0);
        assert_eq!(m.read32(CR2) & 0x0001_0000, 0);
    }

    #[test]
    fn brrdy_cannot_be_set_without_bren() {
        let mut m = machine();
        m.write32(CR2, 0x0001_0000);
        assert_eq!(m.read32(CR2), 0);
    }

    #[test]
    fn usb33rdy_follows_usb33den() {
        let mut m = machine();
        m.write32(CR3, 0x0100_0006);
        assert_eq!(m.read32(CR3), 0x0500_0006);
        m.write32(CR3, 0x0000_0006);
        assert_eq!(m.read32(CR3), 0x0000_0006);
    }

    #[test]
    fn usb33rdy_cannot_be_set_without_usb33den() {
        let mut m = machine();
        m.write32(CR3, 0x0400_0006);
        assert_eq!(m.read32(CR3), 0x0000_0006);
    }

    #[test]
    fn other_registers_keep_their_reset_values_and_store_writes() {
        let mut m = machine();
        assert_eq!(m.read32(CR1), 0xF000_C000);
        assert_eq!(m.read32(CR3), 0x0000_0006);
        m.write32(CR1, 0xF000_C100);
        assert_eq!(m.read32(CR1), 0xF000_C100);
    }

    #[test]
    fn byte_write_to_the_high_byte_of_d3cr() {
        let mut m = machine();
        m.write8(D3CR + 1, 0xC0);
        assert_eq!(m.read32(D3CR), 0x0000_E000);
        assert_eq!(m.read16(D3CR), 0xE000);
        assert_eq!(m.read8(D3CR + 1), 0xE0);
    }

    #[test]
    fn halfword_write_to_d3cr_and_halfword_read_of_csr1() {
        let mut m = machine();
        m.write16(D3CR, 0x8000);
        assert_eq!(m.read16(CSR1), 0xA000);
    }

    #[test]
    fn registers_read_as_on_the_board_at_the_prompt() {
        let mut m = machine();
        m.write32(CR2, 1);
        m.write32(CR3, 0x0100_0042);
        m.write32(D3CR, 0xC000);
        assert_eq!(m.read32(D3CR), 0x0000_E000);
        assert_eq!(m.read32(CSR1), 0x0000_E000);
        assert_eq!(m.read32(CR2), 0x0001_0001);
        assert_eq!(m.read32(CR3), 0x0500_0042);
    }

    #[test]
    fn window_ends_at_pwr_size() {
        let mut m = machine();
        m.write32(PWR_BASE + PWR_SIZE - 4, 5);
        assert_eq!(m.read32(PWR_BASE + PWR_SIZE - 4), 5);
        assert!(m.take_unmapped().is_empty());
    }
}
