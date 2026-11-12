//! USART1, the firmware's console (the USB-serial chip on the board). Nothing is ever busy:
//! the transmitter is always empty and the bytes go straight to the console.
//!
//! RXNEIE and TXEIE are stored, but no interrupt is raised yet.

use crate::console::Console;
use crate::peripheral::StubRegisters;
use crate::registers::{Registers, WordAccess};
use crate::svd::Svd;

pub const USART1_BASE: u32 = 0x4001_1000;
pub const USART_SIZE: u32 = 0x400;

// Register offsets and bit positions, as the SVD gives them.
const CR1: u32 = 0x00;
const RQR: u32 = 0x18;
const ISR: u32 = 0x1C;
const ICR: u32 = 0x20;
const RDR: u32 = 0x24;
const TDR: u32 = 0x28;

const UE: u32 = 1 << 0;
const RE: u32 = 1 << 2;
const TE: u32 = 1 << 3;
const FIFOEN: u32 = 1 << 29;

const RXNE: u32 = 1 << 5;
const TC: u32 = 1 << 6;
const TXE: u32 = 1 << 7;
const TEACK: u32 = 1 << 21;
const REACK: u32 = 1 << 22;
const TXFE: u32 = 1 << 23;
const TXFT: u32 = 1 << 27;

/// Plain registers with the SVD reset values, except for the status, request and data
/// registers.
///
/// USART_ISR is worked out on every read. The transmitter is always empty (TXE, TC), the
/// enable acknowledgements follow UE with TE and RE, and RXNE follows the console's input.
/// With the FIFO on, the transmit FIFO is empty and below its threshold. Every other bit
/// reads 0, as nothing ever sets it: the board's leftover CMF, EOBF and IDLE bits are not
/// modelled. That also means ICR and RQR have nothing to clear or request.
#[derive(Debug)]
pub struct UsartRegisters {
    stored: StubRegisters,
    console: Console,
}

impl UsartRegisters {
    fn isr(&mut self) -> u32 {
        let cr1 = self.stored.read(CR1);
        let mut isr = TXE | TC;
        if cr1 & UE != 0 {
            if cr1 & TE != 0 {
                isr |= TEACK;
            }
            if cr1 & RE != 0 {
                isr |= REACK;
            }
        }
        if cr1 & FIFOEN != 0 {
            isr |= TXFE | TXFT;
        }
        if self.console.has_input() {
            isr |= RXNE;
        }
        isr
    }
}

impl Registers for UsartRegisters {
    fn read(&mut self, offset: u32) -> u32 {
        match offset {
            ISR => self.isr(),
            RDR => u32::from(self.console.read().unwrap_or(0)),
            _ => self.stored.read(offset),
        }
    }

    fn write(&mut self, offset: u32, value: u32, mask: u32) {
        match offset {
            ISR | ICR | RQR | RDR => {}
            TDR => {
                // With the transmitter off the real USART sends nothing, so the byte is lost.
                let cr1 = self.stored.read(CR1);
                if cr1 & UE != 0 && cr1 & TE != 0 && mask & 0xFF != 0 {
                    self.console.write(value as u8);
                }
            }
            _ => self.stored.write(offset, value, mask),
        }
    }
}

pub type Usart = WordAccess<UsartRegisters>;

impl Usart {
    pub fn new(base: u32, console: Console) -> Usart {
        WordAccess::wrap(UsartRegisters {
            stored: StubRegisters::with_reset_values(base, Svd::stm32h743()),
            console,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryMap;

    const CR1_ADDR: u32 = USART1_BASE + CR1;
    const CR2_ADDR: u32 = USART1_BASE + 0x04;
    const BRR_ADDR: u32 = USART1_BASE + 0x0C;
    const RQR_ADDR: u32 = USART1_BASE + RQR;
    const ISR_ADDR: u32 = USART1_BASE + ISR;
    const ICR_ADDR: u32 = USART1_BASE + ICR;
    const RDR_ADDR: u32 = USART1_BASE + RDR;
    const TDR_ADDR: u32 = USART1_BASE + TDR;

    fn machine() -> (MemoryMap, Console) {
        let console = Console::new();
        let mut m = MemoryMap::new();
        m.map_peripheral(
            USART1_BASE,
            USART_SIZE,
            Box::new(Usart::new(USART1_BASE, console.clone())),
        )
        .unwrap();
        (m, console)
    }

    #[test]
    fn isr_at_rest_has_the_transmitter_empty_only() {
        let (mut m, _) = machine();
        assert_eq!(m.read32(ISR_ADDR), 0xC0);
    }

    #[test]
    fn teack_and_reack_follow_ue_with_te_and_re() {
        let (mut m, _) = machine();
        m.write32(CR1_ADDR, UE | TE);
        assert_eq!(m.read32(ISR_ADDR), 0x0020_00C0);
        m.write32(CR1_ADDR, UE | RE);
        assert_eq!(m.read32(ISR_ADDR), 0x0040_00C0);
        m.write32(CR1_ADDR, UE | TE | RE);
        assert_eq!(m.read32(ISR_ADDR), 0x0060_00C0);
    }

    #[test]
    fn nothing_is_acknowledged_while_ue_is_clear() {
        let (mut m, _) = machine();
        m.write32(CR1_ADDR, TE | RE);
        assert_eq!(m.read32(ISR_ADDR), 0xC0);
    }

    #[test]
    fn the_fifo_is_empty_and_below_its_threshold() {
        let (mut m, _) = machine();
        m.write32(CR1_ADDR, FIFOEN);
        assert_eq!(m.read32(ISR_ADDR), 0x0880_00C0);
    }

    #[test]
    fn the_boards_cr1_reads_the_modelled_bits_of_its_isr() {
        let (mut m, _) = machine();
        m.write32(CR1_ADDR, 0x2000_002D);
        let isr = m.read32(ISR_ADDR);
        assert_eq!(isr, 0x08E0_00C0);
        // The board also has CMF, EOBF and IDLE set, which are not modelled.
        let board = 0x08E2_10D0;
        assert_eq!(isr & board, isr);
    }

    #[test]
    fn rxne_follows_the_console_input() {
        let (mut m, console) = machine();
        assert_eq!(m.read32(ISR_ADDR) & RXNE, 0);
        console.push_input(b"A");
        assert_eq!(m.read32(ISR_ADDR) & RXNE, RXNE);
        assert_eq!(m.read32(RDR_ADDR), 0x41);
        assert_eq!(m.read32(ISR_ADDR) & RXNE, 0);
    }

    #[test]
    fn received_bytes_read_back_in_order() {
        let (mut m, console) = machine();
        console.push_input(b"hi");
        assert_eq!(m.read32(RDR_ADDR), u32::from(b'h'));
        assert_eq!(m.read32(ISR_ADDR) & RXNE, RXNE);
        assert_eq!(m.read32(RDR_ADDR), u32::from(b'i'));
        assert_eq!(m.read32(ISR_ADDR) & RXNE, 0);
    }

    #[test]
    fn rdr_with_no_input_reads_zero() {
        let (mut m, _) = machine();
        assert_eq!(m.read32(RDR_ADDR), 0);
    }

    #[test]
    fn tdr_writes_are_captured() {
        let (mut m, console) = machine();
        m.write32(CR1_ADDR, UE | TE);
        m.write32(TDR_ADDR, 0x41);
        m.write32(TDR_ADDR, 0x42);
        assert_eq!(console.take_output(), b"AB");
    }

    #[test]
    fn a_byte_write_to_tdr_is_captured() {
        let (mut m, console) = machine();
        m.write32(CR1_ADDR, UE | TE);
        m.write8(TDR_ADDR, 0x43);
        assert_eq!(console.take_output(), b"C");
    }

    #[test]
    fn only_the_low_eight_bits_are_kept() {
        let (mut m, console) = machine();
        m.write32(CR1_ADDR, UE | TE);
        m.write32(TDR_ADDR, 0x1FF);
        assert_eq!(console.take_output(), [0xFF]);
    }

    #[test]
    fn nothing_is_sent_with_te_clear() {
        let (mut m, console) = machine();
        m.write32(CR1_ADDR, UE | RE);
        m.write32(TDR_ADDR, 0x41);
        assert!(console.take_output().is_empty());
    }

    #[test]
    fn nothing_is_sent_with_ue_clear() {
        let (mut m, console) = machine();
        m.write32(CR1_ADDR, TE);
        m.write32(TDR_ADDR, 0x41);
        assert!(console.take_output().is_empty());
    }

    #[test]
    fn icr_rqr_and_isr_writes_change_nothing_readable() {
        let (mut m, _) = machine();
        m.write32(CR1_ADDR, UE | TE);
        m.write32(ICR_ADDR, 0xFFFF_FFFF);
        m.write32(RQR_ADDR, 0xFFFF_FFFF);
        m.write32(ISR_ADDR, 0xFFFF_FFFF);
        assert_eq!(m.read32(ISR_ADDR), 0x0020_00C0);
        assert_eq!(m.read32(ICR_ADDR), 0);
        assert_eq!(m.read32(RQR_ADDR), 0);
    }

    #[test]
    fn other_registers_keep_their_reset_values_and_store_writes() {
        let (mut m, _) = machine();
        let svd = Svd::stm32h743();
        for addr in [CR2_ADDR, BRR_ADDR] {
            let reset = svd.lookup(addr).unwrap().reset_value;
            assert_eq!(m.read32(addr), reset);
        }
        m.write32(BRR_ADDR, 0x1234);
        m.write32(CR2_ADDR, 0x0000_2000);
        assert_eq!(m.read32(BRR_ADDR), 0x1234);
        assert_eq!(m.read32(CR2_ADDR), 0x0000_2000);
    }

    #[test]
    fn cr1_stores_all_its_bits() {
        let (mut m, _) = machine();
        m.write32(CR1_ADDR, 0x2000_00FD);
        assert_eq!(m.read32(CR1_ADDR), 0x2000_00FD);
    }
}
