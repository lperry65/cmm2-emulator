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
