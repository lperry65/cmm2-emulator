//! The machine as the firmware sees it: which model sits where. The models that exist are
//! mapped here; everything else in the peripheral space is a register stub.

use crate::memory::MemoryMap;
use crate::pwr::{PWR_BASE, PWR_SIZE, Pwr};

pub fn cmm2_memory() -> MemoryMap {
    let mut memory = MemoryMap::new();
    // The layout is fixed, so none of this can overlap.
    memory
        .map_peripheral(PWR_BASE, PWR_SIZE, Box::new(Pwr::new()))
        .expect("PWR window is clear of RAM");
    memory
        .map_unmodelled_peripherals()
        .expect("stub windows are clear of RAM");
    memory
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pwr_is_served_by_the_model() {
        let mut m = cmm2_memory();
        assert_eq!(m.read32(0x5802_4818) & 0x2000, 0x2000); // PWR_D3CR VOSRDY
    }

    #[test]
    fn rcc_is_served_by_the_stub() {
        let mut m = cmm2_memory();
        assert_eq!(m.read32(0x5802_4400), 0x83); // RCC_CR
    }

    #[test]
    fn unmodelled_addresses_read_their_reset_values() {
        let mut m = cmm2_memory();
        assert_eq!(m.read32(0x4001_101C), 0xC0); // USART1_ISR
        assert!(m.take_unmapped().is_empty());
    }

    #[test]
    fn ram_is_there() {
        let mut m = cmm2_memory();
        m.write32(0x2400_0000, 5);
        assert_eq!(m.read32(0x2400_0000), 5);
    }
}
