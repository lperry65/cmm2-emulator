//! USART1, the firmware's console (the USB-serial chip on the board). Nothing is ever busy:
//! the transmitter is always empty and the bytes go straight to the console.
//!
//! The interrupt line is a level on the real chip, but `Irqs` is edge like, so the line is
//! raised at every event that makes the level active or keeps it active after the handler has
//! been through: a CR1 write that turns a condition on, a byte sent (the transmitter is empty
//! again at once), a byte read with more waiting, and input arriving.

use crate::console::Console;
use crate::irq::Irqs;
use crate::peripheral::{ResetCause, StubRegisters};
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

    // A system reset puts the USART back, so CR1 is 0 and the receive interrupt is off. What
    // the console holds belongs to the host and stays.
    fn reset(&mut self, cause: ResetCause) {
        self.stored.reset(cause);
        self.console.set_rx_interrupt(false);
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
mod tests;
