//! USART1, the firmware's console (the USB-serial chip on the board). Nothing is ever busy:
//! the transmitter is always empty and the bytes go straight to the console.
//!
//! The interrupt line is a level on the real chip, but `Irqs` is edge like, so the line is
//! raised at every event that makes the level active or keeps it active after the handler has
//! been through: a CR1 write that turns a condition on, a byte sent (the transmitter is empty
//! again at once), a byte read with more waiting, and input arriving.

use crate::console::Console;
use crate::irq::Irqs;
use crate::peripheral::StubRegisters;
use crate::registers::{Registers, WordAccess};
use crate::svd::Svd;

pub const USART1_BASE: u32 = 0x4001_1000;
pub const USART_SIZE: u32 = 0x400;

/// `USART1_IRQn`.
const USART1_IRQ: u16 = 37;

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
const RXNEIE: u32 = 1 << 5;
const TCIE: u32 = 1 << 6;
const TXEIE: u32 = 1 << 7;
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
    irqs: Irqs,
}

impl UsartRegisters {
    /// Whether the interrupt line would be active, which the transmitter always being empty
    /// makes a matter of the enable bits and the console's input.
    fn interrupt_active(&mut self) -> bool {
        let cr1 = self.stored.read(CR1);
        cr1 & (TXEIE | TCIE) != 0 || cr1 & RXNEIE != 0 && self.console.has_input()
    }

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
            RDR => {
                let byte = self.console.read();
                if byte.is_some() && self.stored.read(CR1) & RXNEIE != 0 && self.console.has_input()
                {
                    self.irqs.raise(USART1_IRQ);
                }
                u32::from(byte.unwrap_or(0))
            }
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
                    if cr1 & (TXEIE | TCIE) != 0 {
                        self.irqs.raise(USART1_IRQ);
                    }
                }
            }
            CR1 => {
                let was_active = self.interrupt_active();
                self.stored.write(offset, value, mask);
                self.console
                    .set_rx_interrupt(self.stored.read(CR1) & RXNEIE != 0);
                if !was_active && self.interrupt_active() {
                    self.irqs.raise(USART1_IRQ);
                }
            }
            _ => self.stored.write(offset, value, mask),
        }
    }
}

pub type Usart = WordAccess<UsartRegisters>;

impl Usart {
    pub fn new(base: u32, console: Console, irqs: Irqs) -> Usart {
        console.attach_interrupt(irqs.clone(), USART1_IRQ);
        WordAccess::wrap(UsartRegisters {
            stored: StubRegisters::with_reset_values(base, Svd::stm32h743()),
            console,
            irqs,
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

    fn machine_with_irqs() -> (MemoryMap, Console, Irqs) {
        let console = Console::new();
        let irqs = Irqs::new();
        let mut m = MemoryMap::new();
        m.map_peripheral(
            USART1_BASE,
            USART_SIZE,
            Box::new(Usart::new(USART1_BASE, console.clone(), irqs.clone())),
        )
        .unwrap();
        (m, console, irqs)
    }

    fn machine() -> (MemoryMap, Console) {
        let (m, console, _) = machine_with_irqs();
        (m, console)
    }

    /// Whether the USART1 line, and only that, was raised since the last call.
    fn raised(irqs: &Irqs) -> bool {
        irqs.take() == Some([0, 1 << (USART1_IRQ - 32), 0, 0, 0, 0, 0, 0])
    }

    fn nothing_raised(irqs: &Irqs) -> bool {
        irqs.take().is_none()
    }

    #[test]
    fn enabling_tcie_raises_the_line() {
        let (mut m, _, irqs) = machine_with_irqs();
        m.write32(CR1_ADDR, UE | TE);
        assert!(nothing_raised(&irqs));
        m.write32(CR1_ADDR, UE | TE | TCIE);
        assert!(raised(&irqs));
    }

    #[test]
    fn enabling_txeie_raises_the_line() {
        let (mut m, _, irqs) = machine_with_irqs();
        m.write32(CR1_ADDR, UE | TE | TXEIE);
        assert!(raised(&irqs));
    }

    #[test]
    fn enabling_neither_raises_nothing() {
        let (mut m, _, irqs) = machine_with_irqs();
        m.write32(CR1_ADDR, UE | TE | RE | RXNEIE);
        assert!(nothing_raised(&irqs));
    }

    #[test]
    fn rewriting_cr1_with_the_condition_already_true_raises_nothing() {
        let (mut m, _, irqs) = machine_with_irqs();
        m.write32(CR1_ADDR, UE | TE | TCIE);
        assert!(raised(&irqs));
        m.write32(CR1_ADDR, UE | TE | TCIE);
        assert!(nothing_raised(&irqs));
    }

    #[test]
    fn enabling_again_after_disabling_raises_again() {
        let (mut m, _, irqs) = machine_with_irqs();
        m.write32(CR1_ADDR, UE | TE | TCIE);
        assert!(raised(&irqs));
        m.write32(CR1_ADDR, UE | TE);
        assert!(nothing_raised(&irqs));
        m.write32(CR1_ADDR, UE | TE | TCIE);
        assert!(raised(&irqs));
    }

    #[test]
    fn rxneie_raises_only_with_input_waiting() {
        let (mut m, console, irqs) = machine_with_irqs();
        m.write32(CR1_ADDR, UE | RE | RXNEIE);
        assert!(nothing_raised(&irqs));
        m.write32(CR1_ADDR, UE | RE);
        console.push_input(b"1");
        assert!(nothing_raised(&irqs));
        m.write32(CR1_ADDR, UE | RE | RXNEIE);
        assert!(raised(&irqs));
    }

    #[test]
    fn a_byte_sent_with_tcie_or_txeie_set_raises_the_line() {
        for enable in [TCIE, TXEIE] {
            let (mut m, console, irqs) = machine_with_irqs();
            m.write32(CR1_ADDR, UE | TE | enable);
            assert!(raised(&irqs));
            m.write32(TDR_ADDR, 0x41);
            assert!(raised(&irqs));
            assert_eq!(console.take_output(), b"A");
        }
    }

    #[test]
    fn a_byte_sent_with_neither_enable_raises_nothing() {
        let (mut m, _, irqs) = machine_with_irqs();
        m.write32(CR1_ADDR, UE | TE | RE | RXNEIE);
        m.write32(TDR_ADDR, 0x41);
        assert!(nothing_raised(&irqs));
    }

    #[test]
    fn a_byte_dropped_with_te_clear_raises_nothing() {
        let (mut m, _, irqs) = machine_with_irqs();
        m.write32(CR1_ADDR, UE | TCIE);
        assert!(raised(&irqs));
        m.write32(TDR_ADDR, 0x41);
        assert!(nothing_raised(&irqs));
    }

    #[test]
    fn reading_with_more_input_waiting_raises_the_line() {
        let (mut m, console, irqs) = machine_with_irqs();
        console.push_input(b"hi");
        m.write32(CR1_ADDR, UE | RE | RXNEIE);
        assert!(raised(&irqs));
        assert_eq!(m.read32(RDR_ADDR), u32::from(b'h'));
        assert!(raised(&irqs));
    }

    #[test]
    fn reading_the_last_byte_raises_nothing() {
        let (mut m, console, irqs) = machine_with_irqs();
        console.push_input(b"h");
        m.write32(CR1_ADDR, UE | RE | RXNEIE);
        assert!(raised(&irqs));
        m.read32(RDR_ADDR);
        assert!(nothing_raised(&irqs));
    }

    #[test]
    fn reading_with_rxneie_clear_raises_nothing() {
        let (mut m, console, irqs) = machine_with_irqs();
        console.push_input(b"hi");
        m.write32(CR1_ADDR, UE | RE);
        m.read32(RDR_ADDR);
        assert!(nothing_raised(&irqs));
    }

    #[test]
    fn pushed_input_raises_the_line_when_rxneie_is_set() {
        let (mut m, console, irqs) = machine_with_irqs();
        m.write32(CR1_ADDR, UE | RE | RXNEIE);
        console.push_input(b"1");
        assert!(raised(&irqs));
    }

    #[test]
    fn pushed_input_raises_nothing_with_rxneie_clear() {
        let (mut m, console, irqs) = machine_with_irqs();
        m.write32(CR1_ADDR, UE | RE);
        console.push_input(b"1");
        assert!(nothing_raised(&irqs));
    }

    #[test]
    fn a_second_push_while_input_waits_raises_nothing() {
        let (mut m, console, irqs) = machine_with_irqs();
        m.write32(CR1_ADDR, UE | RE | RXNEIE);
        console.push_input(b"1");
        assert!(raised(&irqs));
        // The first byte is still queued, so the line never dropped and has nothing new to say.
        console.push_input(b"2");
        assert!(nothing_raised(&irqs));
    }

    #[test]
    fn pushing_nothing_raises_nothing() {
        let (mut m, console, irqs) = machine_with_irqs();
        m.write32(CR1_ADDR, UE | RE | RXNEIE);
        console.push_input(b"");
        assert!(nothing_raised(&irqs));
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
