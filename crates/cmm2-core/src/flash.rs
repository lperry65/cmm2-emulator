//! The flash controller. Both banks unlock with the key sequence and erase by sector or whole
//! bank. An erase completes the moment it starts, which is all the firmware waits for.
//!
//! The erase itself is queued for the CPU, because the flash lives in the core. Programming
//! needs nothing here: the core's flash takes the stores. It does not check that a store only
//! clears bits (a real program cannot set a bit back without an erase), so a program that
//! relies on that is not caught.
//!
//! A wrong key locks a bank until reset, and on the real chip it also gives a bus error. The
//! model does not raise that error.

use crate::flash_ops::FlashOps;
use crate::irq::Irqs;
use crate::peripheral::{ResetCause, StubRegisters};
use crate::registers::{Registers, WordAccess};
use crate::svd::Svd;

pub const FLASH_REGS_BASE: u32 = 0x5200_2000;
pub const FLASH_REGS_SIZE: u32 = 0x400;

/// `FLASH_IRQn`.
const FLASH_IRQ: u16 = 4;

const FLASH_BANK1: u32 = 0x0800_0000;
const FLASH_BANK2: u32 = 0x0810_0000;
const BANK_SIZE: u32 = 0x10_0000;
const SECTOR_SIZE: u32 = 0x2_0000;

// The registers of a bank, from the start of the bank. Bank 2 starts 0x100 after bank 1.
const BANK_STRIDE: u32 = 0x100;
const KEYR: u32 = 0x04;
const CR: u32 = 0x0C;
const SR: u32 = 0x10;
const CCR: u32 = 0x14;

const KEY1: u32 = 0x4567_0123;
const KEY2: u32 = 0xCDEF_89AB;

const LOCK: u32 = 1 << 0; // FLASH_CR
const SER: u32 = 1 << 2;
const BER: u32 = 1 << 3;
const START: u32 = 1 << 7;
const SNB_SHIFT: u32 = 8;
const SNB: u32 = 0b111 << SNB_SHIFT;
const EOPIE: u32 = 1 << 16;
const EOP: u32 = 1 << 16; // FLASH_SR

// FLASH_CR is 0x31 at reset: locked, with PSIZE 3. The SVD has 0x01 for bank 1, which is wrong.
const CR_RESET: u32 = 0x30;

#[derive(Debug)]
struct Bank {
    base: u32,
    locked: bool,
    // Set by a wrong key, and only a reset clears it.
    locked_up: bool,
    key1_seen: bool,
    // FLASH_CR without LOCK and START, which are worked out on every read.
    cr: u32,
    sr: u32,
}

/// Plain registers with the SVD reset values, except the key, control, status and clear
/// registers of the two banks.
pub struct FlashRegisters {
    stored: StubRegisters,
    banks: [Bank; 2],
    irqs: Irqs,
    ops: FlashOps,
}

impl FlashRegisters {
    fn bank(&mut self, offset: u32) -> Option<(&mut Bank, u32)> {
        let index = usize::try_from(offset / BANK_STRIDE).ok()?;
        let register = offset % BANK_STRIDE;
        match register {
            KEYR | CR | SR | CCR => Some((self.banks.get_mut(index)?, register)),
            _ => None,
        }
    }
}

impl Bank {
    fn new(base: u32) -> Bank {
        Bank {
            base,
            locked: true,
            locked_up: false,
            key1_seen: false,
            cr: CR_RESET,
            sr: 0,
        }
    }

    fn key(&mut self, key: u32) {
        if key == KEY1 {
            self.key1_seen = true;
        } else if key == KEY2 && self.key1_seen {
            self.key1_seen = false;
            self.locked = self.locked_up;
        } else {
            self.key1_seen = false;
            self.locked_up = true;
            self.locked = true;
        }
    }

    fn write_cr(&mut self, value: u32, mask: u32, irqs: &Irqs, ops: &FlashOps) {
        if self.locked {
            return;
        }
        let value = self.cr & !mask | value & mask;
        self.cr = value & !(LOCK | START);
        if value & LOCK != 0 {
            self.locked = true;
        } else if value & START != 0 {
            self.start(irqs, ops);
        }
    }

    fn start(&mut self, irqs: &Irqs, ops: &FlashOps) {
        let sector = (self.cr & SNB) >> SNB_SHIFT;
        if self.cr & BER != 0 {
            ops.erase(self.base, BANK_SIZE);
        } else if self.cr & SER != 0 {
            ops.erase(self.base + sector * SECTOR_SIZE, SECTOR_SIZE);
        } else {
            // No erase asked for: the real chip flags an error, which the model does not.
            return;
        }
        self.sr |= EOP;
        if self.cr & EOPIE != 0 {
            irqs.raise(FLASH_IRQ);
        }
    }
}

impl Registers for FlashRegisters {
    fn read(&mut self, offset: u32) -> u32 {
        match self.bank(offset) {
            Some((bank, CR)) => bank.cr | u32::from(bank.locked),
            Some((bank, SR)) => bank.sr,
            _ => self.stored.read(offset),
        }
    }

    fn write(&mut self, offset: u32, value: u32, mask: u32) {
        let (irqs, ops) = (self.irqs.clone(), self.ops.clone());
        match self.bank(offset) {
            Some((bank, KEYR)) => bank.key(value),
            Some((bank, CR)) => bank.write_cr(value, mask, &irqs, &ops),
            Some((_, SR)) => {}
            Some((bank, CCR)) => bank.sr &= !(value & mask),
            _ => self.stored.write(offset, value, mask),
        }
    }

    // A system reset locks both banks again and clears what a wrong key did.
    fn reset(&mut self, cause: ResetCause) {
        self.stored.reset(cause);
        self.banks = [Bank::new(FLASH_BANK1), Bank::new(FLASH_BANK2)];
    }
}

pub type FlashController = WordAccess<FlashRegisters>;

impl FlashController {
    pub fn new(irqs: Irqs, ops: FlashOps) -> FlashController {
        WordAccess::wrap(FlashRegisters {
            stored: StubRegisters::with_reset_values(FLASH_REGS_BASE, Svd::stm32h743()),
            banks: [Bank::new(FLASH_BANK1), Bank::new(FLASH_BANK2)],
            irqs,
            ops,
        })
    }
}

#[cfg(test)]
mod tests;
