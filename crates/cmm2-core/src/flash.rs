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
mod tests {
    use super::*;
    use crate::flash_ops::FlashOp;
    use crate::{MemoryMap, ResetCause};

    const ACR: u32 = FLASH_REGS_BASE;
    const KEYR1: u32 = FLASH_REGS_BASE + 0x04;
    const CR1: u32 = FLASH_REGS_BASE + 0x0C;
    const SR1: u32 = FLASH_REGS_BASE + 0x10;
    const KEYR2: u32 = FLASH_REGS_BASE + 0x104;
    const CR2: u32 = FLASH_REGS_BASE + 0x10C;
    const SR2: u32 = FLASH_REGS_BASE + 0x110;
    const CCR2: u32 = FLASH_REGS_BASE + 0x114;

    // PSIZE 3, EOPIE and the error interrupt enables, as the firmware sets them.
    const IRQ_ENABLES: u32 = 0x006F_0000;
    const PSIZE: u32 = 0x30;

    struct Machine {
        map: MemoryMap,
        ops: FlashOps,
        irqs: Irqs,
    }

    fn machine() -> Machine {
        let mut map = MemoryMap::new();
        let ops = map.flash_ops();
        let irqs = Irqs::new();
        map.map_peripheral(
            FLASH_REGS_BASE,
            FLASH_REGS_SIZE,
            Box::new(FlashController::new(irqs.clone(), ops.clone())),
        )
        .unwrap();
        Machine { map, ops, irqs }
    }

    fn unlock(map: &mut MemoryMap, keyr: u32) {
        map.write32(keyr, KEY1);
        map.write32(keyr, KEY2);
    }

    fn erase(base: u32, len: u32) -> Vec<FlashOp> {
        vec![FlashOp::Erase { base, len }]
    }

    #[test]
    fn both_banks_read_the_reset_value_of_cr() {
        let mut m = machine();
        assert_eq!(m.map.read32(CR1), 0x31);
        assert_eq!(m.map.read32(CR2), 0x31);
    }

    #[test]
    fn the_keys_unlock_one_bank_only() {
        let mut m = machine();
        unlock(&mut m.map, KEYR2);
        assert_eq!(m.map.read32(CR2), 0x30);
        assert_eq!(m.map.read32(CR1), 0x31);
    }

    #[test]
    fn a_wrong_key_locks_the_bank_up_until_reset() {
        let mut m = machine();
        m.map.write32(KEYR2, KEY1);
        m.map.write32(KEYR2, 0x1234_5678);
        unlock(&mut m.map, KEYR2);
        assert_eq!(m.map.read32(CR2), 0x31);
    }

    #[test]
    fn key2_without_key1_locks_the_bank_up() {
        let mut m = machine();
        m.map.write32(KEYR2, KEY2);
        unlock(&mut m.map, KEYR2);
        assert_eq!(m.map.read32(CR2), 0x31);
    }

    #[test]
    fn a_wrong_key_locks_an_unlocked_bank() {
        let mut m = machine();
        unlock(&mut m.map, KEYR2);
        m.map.write32(KEYR2, 0);
        assert_eq!(m.map.read32(CR2), 0x31);
    }

    #[test]
    fn key1_does_not_lock_an_unlocked_bank() {
        let mut m = machine();
        unlock(&mut m.map, KEYR2);
        m.map.write32(KEYR2, KEY1);
        assert_eq!(m.map.read32(CR2), 0x30);
        m.map.write32(KEYR2, KEY2);
        assert_eq!(m.map.read32(CR2), 0x30);
    }

    #[test]
    fn cr_writes_are_ignored_while_locked() {
        let mut m = machine();
        m.map.write32(CR2, IRQ_ENABLES | PSIZE);
        assert_eq!(m.map.read32(CR2), 0x31);
    }

    #[test]
    fn cr_stores_writes_once_unlocked_and_start_reads_zero() {
        let mut m = machine();
        unlock(&mut m.map, KEYR2);
        m.map.write32(CR2, IRQ_ENABLES | PSIZE);
        assert_eq!(m.map.read32(CR2), IRQ_ENABLES | PSIZE);
    }

    #[test]
    fn writing_lock_locks_again() {
        let mut m = machine();
        unlock(&mut m.map, KEYR2);
        m.map.write32(CR2, PSIZE | LOCK);
        assert_eq!(m.map.read32(CR2), PSIZE | LOCK);
        m.map.write32(CR2, IRQ_ENABLES | PSIZE);
        assert_eq!(m.map.read32(CR2), PSIZE | LOCK);
        // The keys unlock it again, since nothing was wrong with them.
        unlock(&mut m.map, KEYR2);
        assert_eq!(m.map.read32(CR2), PSIZE);
    }

    // The firmware's writes to FLASH_CR2, except that LOCK is clear: the board reads CR with
    // LOCK clear after the keys and writes that back.
    #[test]
    fn the_firmwares_sequence_erases_sector_7_of_bank_2() {
        let mut m = machine();
        unlock(&mut m.map, KEYR2);
        for value in [0x006F_0030, 0x006F_0000, 0x006F_07A4] {
            m.map.write32(CR2, value);
        }
        assert_eq!(m.ops.take(), Some(erase(0x081E_0000, 0x2_0000)));
        assert_eq!(m.map.read32(SR2), EOP);
        assert_eq!(m.map.read32(CR2) & START, 0);
        assert_eq!(m.map.read32(SR2) & 0b111, 0);
    }

    #[test]
    fn a_sector_erase_of_bank_1() {
        let mut m = machine();
        unlock(&mut m.map, KEYR1);
        m.map.write32(CR1, SER | START | 7 << 8);
        assert_eq!(m.ops.take(), Some(erase(0x080E_0000, 0x2_0000)));
        assert_eq!(m.map.read32(SR1), EOP);
        assert_eq!(m.map.read32(SR2), 0);
    }

    #[test]
    fn a_bank_erase_covers_the_whole_bank() {
        let mut m = machine();
        unlock(&mut m.map, KEYR2);
        m.map.write32(CR2, BER | START);
        assert_eq!(m.ops.take(), Some(erase(0x0810_0000, 0x10_0000)));
    }

    #[test]
    fn start_while_locked_does_nothing() {
        let mut m = machine();
        m.map.write32(CR2, SER | START | 7 << 8);
        assert_eq!(m.ops.take(), None);
        assert_eq!(m.map.read32(SR2), 0);
    }

    // The real chip flags an error, which the model does not.
    #[test]
    fn start_without_an_erase_does_nothing() {
        let mut m = machine();
        unlock(&mut m.map, KEYR2);
        m.map.write32(CR2, START);
        assert_eq!(m.ops.take(), None);
        assert_eq!(m.map.read32(SR2), 0);
    }

    #[test]
    fn ccr_clears_only_the_flags_written() {
        let mut m = machine();
        unlock(&mut m.map, KEYR2);
        m.map.write32(CR2, SER | START);
        m.map.write32(CCR2, 1 << 17);
        assert_eq!(m.map.read32(SR2), EOP);
        m.map.write32(CCR2, EOP);
        assert_eq!(m.map.read32(SR2), 0);
    }

    #[test]
    fn sr_ignores_writes() {
        let mut m = machine();
        unlock(&mut m.map, KEYR2);
        m.map.write32(CR2, SER | START);
        m.map.write32(SR2, 0);
        assert_eq!(m.map.read32(SR2), EOP);
        m.map.write32(SR2, 0xFFFF_FFFF);
        assert_eq!(m.map.read32(SR2), EOP);
    }

    #[test]
    fn the_interrupt_is_raised_once_per_erase_when_eopie_is_set() {
        let mut m = machine();
        unlock(&mut m.map, KEYR2);
        m.map.write32(CR2, EOPIE | SER | START);
        assert_eq!(m.irqs.take(), Some([1 << 4, 0, 0, 0, 0, 0, 0, 0]));
        assert_eq!(m.irqs.take(), None);
        m.map.write32(CR2, EOPIE | SER | START);
        assert_eq!(m.irqs.take(), Some([1 << 4, 0, 0, 0, 0, 0, 0, 0]));
    }

    #[test]
    fn no_interrupt_without_eopie() {
        let mut m = machine();
        unlock(&mut m.map, KEYR2);
        m.map.write32(CR2, SER | START);
        assert_eq!(m.irqs.take(), None);
        assert_eq!(m.map.read32(SR2), EOP);
    }

    #[test]
    fn acr_stores_writes() {
        let mut m = machine();
        assert_eq!(m.map.read32(ACR), 0x37);
        m.map.write32(ACR, 0x32);
        assert_eq!(m.map.read32(ACR), 0x32);
    }

    #[test]
    fn reset_relocks_an_unlocked_bank() {
        let mut m = machine();
        unlock(&mut m.map, KEYR2);
        m.map.write32(KEYR1, KEY1);
        m.map.reset_peripherals(ResetCause::Software);
        assert_eq!(m.map.read32(CR2), 0x31);
        // The key progress went too: KEY2 alone is a wrong key.
        m.map.write32(KEYR1, KEY2);
        unlock(&mut m.map, KEYR1);
        assert_eq!(m.map.read32(CR1), 0x31);
    }

    #[test]
    fn reset_unlocks_a_locked_up_bank_for_the_keys() {
        let mut m = machine();
        m.map.write32(KEYR2, 0);
        m.map.reset_peripherals(ResetCause::Software);
        unlock(&mut m.map, KEYR2);
        assert_eq!(m.map.read32(CR2), 0x30);
    }

    #[test]
    fn reset_clears_status_flags_control_and_the_stub() {
        let mut m = machine();
        unlock(&mut m.map, KEYR2);
        m.map.write32(CR2, EOPIE | PSIZE | BER | START);
        assert_eq!(m.map.read32(SR2) & EOP, EOP);
        m.map.write32(ACR, 0x32);
        m.map.reset_peripherals(ResetCause::Software);
        assert_eq!(m.map.read32(SR2), 0);
        assert_eq!(m.map.read32(CR2), 0x31);
        assert_eq!(m.map.read32(ACR), 0x37);
    }
}
