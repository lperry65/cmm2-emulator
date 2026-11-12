//! The reset and clock controller. The clocks start instantly: the firmware only waits for
//! the ready flags, so they read as set as soon as what they wait for is switched on.

use crate::peripheral::StubRegisters;
use crate::registers::{Registers, WordAccess, follow};
use crate::svd::Svd;

pub const RCC_BASE: u32 = 0x5802_4400;
pub const RCC_SIZE: u32 = 0x400;

// Register offsets and bit positions, as the SVD gives them.
const CR: u32 = 0x00;
const CFGR: u32 = 0x10;
const BDCR: u32 = 0x70;
const CSR: u32 = 0x74;

// RCC_CR: each ready bit follows the on bit next to it in this table.
const READY_FOLLOWS_ON: [(u32, u32); 7] = [
    (1 << 0, 1 << 2),   // HSION, HSIRDY
    (1 << 7, 1 << 8),   // CSION, CSIRDY
    (1 << 12, 1 << 13), // RC48ON, RC48RDY
    (1 << 16, 1 << 17), // HSEON, HSERDY
    (1 << 24, 1 << 25), // PLL1ON, PLL1RDY
    (1 << 26, 1 << 27), // PLL2ON, PLL2RDY
    (1 << 28, 1 << 29), // PLL3ON, PLL3RDY
];
// RCC_CR: the board reads these as set whatever the on bits say.
const HSIDIVF: u32 = 1 << 5;
const D1CKRDY: u32 = 1 << 14;
const D2CKRDY: u32 = 1 << 15;
const SW: u32 = 0b111; // RCC_CFGR
const SWS: u32 = 0b111 << 3; // RCC_CFGR
const LSEON: u32 = 1 << 0; // RCC_BDCR
const LSERDY: u32 = 1 << 1; // RCC_BDCR
const LSION: u32 = 1 << 0; // RCC_CSR
const LSIRDY: u32 = 1 << 1; // RCC_CSR

/// Plain registers with the SVD reset values, except for the status bits that the hardware
/// computes. Those are worked out on every read and ignore what is written to them.
#[derive(Debug)]
pub struct RccRegisters {
    stored: StubRegisters,
}

impl Registers for RccRegisters {
    fn read(&mut self, offset: u32) -> u32 {
        let value = self.stored.read(offset);
        match offset {
            CR => READY_FOLLOWS_ON
                .iter()
                .fold(value | HSIDIVF | D1CKRDY | D2CKRDY, |v, &(on, ready)| {
                    follow(v, on, ready)
                }),
            CFGR => value & !SWS | (value & SW) << 3,
            BDCR => follow(value, LSEON, LSERDY),
            CSR => follow(value, LSION, LSIRDY),
            _ => value,
        }
    }

    fn write(&mut self, offset: u32, value: u32, mask: u32) {
        self.stored.write(offset, value, mask);
    }
}

pub type Rcc = WordAccess<RccRegisters>;

impl Rcc {
    pub fn new() -> Rcc {
        WordAccess::wrap(RccRegisters {
            stored: StubRegisters::with_reset_values(RCC_BASE, Svd::stm32h743()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryMap;

    const CR: u32 = RCC_BASE;
    const CFGR: u32 = RCC_BASE + 0x10;
    const BDCR: u32 = RCC_BASE + 0x70;
    const CSR: u32 = RCC_BASE + 0x74;
    const PLL1DIVR: u32 = RCC_BASE + 0x30;
    const AHB4ENR: u32 = RCC_BASE + 0xE0;

    // (name, on bit, ready bit) of the RCC_CR pairs that follow.
    const PAIRS: [(&str, u32, u32); 7] = [
        ("HSI", 1 << 0, 1 << 2),
        ("CSI", 1 << 7, 1 << 8),
        ("RC48", 1 << 12, 1 << 13),
        ("HSE", 1 << 16, 1 << 17),
        ("PLL1", 1 << 24, 1 << 25),
        ("PLL2", 1 << 26, 1 << 27),
        ("PLL3", 1 << 28, 1 << 29),
    ];
    // Always set: D1CKRDY, D2CKRDY and HSIDIVF.
    const ALWAYS_READY: u32 = 1 << 14 | 1 << 15 | 1 << 5;
    const ON_BITS: u32 = 0x1501_1001;
    const BOARD_CR: u32 = 0x3F03_F025;

    fn machine() -> MemoryMap {
        let mut m = MemoryMap::new();
        m.map_peripheral(RCC_BASE, RCC_SIZE, Box::new(Rcc::new()))
            .unwrap();
        m
    }

    #[test]
    fn each_ready_bit_follows_its_on_bit_both_ways() {
        for (name, on, ready) in PAIRS {
            let mut m = machine();
            m.write32(CR, 0);
            m.write32(CR, on);
            assert_eq!(m.read32(CR) & ready, ready, "{name} set");
            m.write32(CR, 0);
            assert_eq!(m.read32(CR) & ready, 0, "{name} cleared");
        }
    }

    #[test]
    fn domain_clocks_and_hsidivf_always_read_set() {
        let mut m = machine();
        m.write32(CR, 0);
        assert_eq!(m.read32(CR), ALWAYS_READY);
    }

    #[test]
    fn writes_to_ready_bits_do_nothing() {
        let mut m = machine();
        let on: u32 = PAIRS.iter().map(|p| p.1).sum();
        let ready: u32 = PAIRS.iter().map(|p| p.2).sum();
        m.write32(CR, !on);
        assert_eq!(m.read32(CR) & ready, 0);
    }

    // The reset value 0x83 has HSION, HSIKERON and CSION set.
    #[test]
    fn unwritten_cr_reads_the_reset_value_with_its_ready_bits() {
        let mut m = machine();
        assert_eq!(m.read32(CR), 0x0000_C1A7);
    }

    #[test]
    fn the_board_value_of_cr_comes_from_its_on_bits() {
        let mut m = machine();
        m.write32(CR, ON_BITS);
        assert_eq!(m.read32(CR), BOARD_CR);
    }

    #[test]
    fn cfgr_sws_follows_sw() {
        let mut m = machine();
        for sw in 0..=3 {
            m.write32(CFGR, sw);
            assert_eq!(m.read32(CFGR), sw | sw << 3, "SW {sw}");
        }
    }

    #[test]
    fn cfgr_stores_the_other_fields() {
        let mut m = machine();
        m.write32(CFGR, 0xFFFF_FFC3);
        assert_eq!(m.read32(CFGR), 0xFFFF_FFDB);
    }

    #[test]
    fn cfgr_byte_write_to_the_low_byte() {
        let mut m = machine();
        m.write32(CFGR, 0x0000_0100);
        m.write8(CFGR, 3);
        assert_eq!(m.read32(CFGR), 0x0000_011B);
    }

    #[test]
    fn bdcr_lserdy_follows_lseon_and_the_rest_is_stored() {
        let mut m = machine();
        m.write32(BDCR, 0x0000_8111);
        assert_eq!(m.read32(BDCR), 0x0000_8113);
        m.write32(BDCR, 0x0000_8110);
        assert_eq!(m.read32(BDCR), 0x0000_8110);
        m.write32(BDCR, 0x2);
        assert_eq!(m.read32(BDCR), 0);
    }

    #[test]
    fn csr_lsirdy_follows_lsion() {
        let mut m = machine();
        m.write32(CSR, 1);
        assert_eq!(m.read32(CSR), 3);
        m.write32(CSR, 0);
        assert_eq!(m.read32(CSR), 0);
        m.write32(CSR, 2);
        assert_eq!(m.read32(CSR), 0);
    }

    #[test]
    fn read_modify_write_of_cr_brings_pll1_up_and_down() {
        let (hse_on, hse_rdy) = (1 << 16, 1 << 17);
        let (pll1_on, pll1_rdy) = (1 << 24, 1 << 25);
        let mut m = machine();
        let cr = m.read32(CR);
        m.write32(CR, cr | hse_on);
        assert_eq!(m.read32(CR) & hse_rdy, hse_rdy);
        let cr = m.read32(CR);
        m.write32(CR, cr | pll1_on);
        let cr = m.read32(CR);
        assert_eq!(cr & pll1_rdy, pll1_rdy);
        assert_eq!(cr & hse_rdy, hse_rdy);
        m.write32(CR, cr & !pll1_on);
        let cr = m.read32(CR);
        assert_eq!(cr & pll1_rdy, 0);
        assert_eq!(cr & hse_rdy, hse_rdy);
    }

    #[test]
    fn ordinary_registers_store_writes_and_start_at_their_reset_values() {
        let mut m = machine();
        assert_eq!(m.read32(PLL1DIVR), 0x0101_0280);
        m.write32(PLL1DIVR, 0x0102_03FF);
        assert_eq!(m.read32(PLL1DIVR), 0x0102_03FF);
        m.write16(AHB4ENR, 0x00FF);
        assert_eq!(m.read32(AHB4ENR), 0x00FF);
    }

    #[test]
    fn window_ends_at_rcc_size() {
        let mut m = machine();
        m.write32(RCC_BASE + RCC_SIZE - 4, 5);
        assert_eq!(m.read32(RCC_BASE + RCC_SIZE - 4), 5);
        assert!(m.take_unmapped().is_empty());
    }
}
