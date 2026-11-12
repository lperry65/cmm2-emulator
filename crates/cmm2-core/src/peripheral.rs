//! Memory-mapped peripherals: the trait a device implements, and the stand-in for the ones
//! that are not modelled yet.

use std::collections::HashMap;

use crate::registers::{Registers, WordAccess};
use crate::svd::Svd;

/// A device mapped into the address space. `offset` is the byte offset from the start of
/// the window the device is mapped at, and `size` is 1, 2 or 4 bytes. The access is passed
/// whole, never split into bytes.
pub trait Peripheral {
    /// The value is zero-extended to 32 bits.
    fn read(&mut self, offset: u32, size: u8) -> u32;
    fn write(&mut self, offset: u32, size: u8, value: u32);
}

/// One access to a peripheral, as seen by the trace callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeripheralAccess {
    /// Absolute address.
    pub addr: u32,
    pub size: u8,
    pub write: bool,
    /// What the peripheral returned for a read, or what was written, masked to `size` bytes.
    /// Only the trace is masked: the device is handed the written value as it came in.
    pub value: u32,
    /// Address of the instruction that made the access.
    pub pc: u32,
}

/// The registers of a peripheral that is not modelled: every register just remembers what
/// was written. A word starts at zero, or at the reset values from the SVD if it has them.
#[derive(Debug, Default)]
pub struct StubRegisters {
    // Keyed by the offset of the word, which is a multiple of 4.
    words: HashMap<u32, u32>,
    // The absolute address the stub is mapped at, and the registers found there.
    resets: Option<(u32, &'static Svd)>,
}

impl StubRegisters {
    /// `base` is the absolute address of the start of the window the registers are mapped
    /// at. Bytes that no register in the SVD covers start at zero.
    pub fn with_reset_values(base: u32, svd: &'static Svd) -> StubRegisters {
        StubRegisters {
            resets: Some((base, svd)),
            ..StubRegisters::default()
        }
    }

    // The word at `offset` (a multiple of 4), set to its reset value the first time it is
    // touched so that polling a register does not look it up again.
    fn word(&mut self, offset: u32) -> &mut u32 {
        let resets = self.resets;
        self.words.entry(offset).or_insert_with(|| {
            let Some((base, svd)) = resets else {
                return 0;
            };
            (0..4).fold(0, |word, i| {
                let addr = base.wrapping_add(offset).wrapping_add(i);
                let byte = svd
                    .lookup(addr)
                    .map_or(0, |found| found.reset_value >> (8 * found.offset) & 0xFF);
                word | byte << (8 * i)
            })
        })
    }
}

impl Registers for StubRegisters {
    fn read(&mut self, offset: u32) -> u32 {
        *self.word(offset)
    }

    fn write(&mut self, offset: u32, value: u32, mask: u32) {
        let word = self.word(offset);
        *word = *word & !mask | value & mask;
    }
}

/// For peripherals that are not modelled. Registers are 32 bits wide; narrower accesses read
/// and merge into the word they fall in.
pub type RegisterStub = WordAccess<StubRegisters>;

impl RegisterStub {
    pub fn new() -> RegisterStub {
        RegisterStub::default()
    }

    /// See `StubRegisters::with_reset_values`.
    pub fn with_reset_values(base: u32, svd: &'static Svd) -> RegisterStub {
        WordAccess::wrap(StubRegisters::with_reset_values(base, svd))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::svd::Svd;

    #[test]
    fn unwritten_registers_read_zero() {
        let mut s = RegisterStub::new();
        assert_eq!(s.read(0x10, 4), 0);
        assert_eq!(s.read(0x11, 1), 0);
    }

    #[test]
    fn word_write_reads_back() {
        let mut s = RegisterStub::new();
        s.write(0x20, 4, 0xDEAD_BEEF);
        assert_eq!(s.read(0x20, 4), 0xDEAD_BEEF);
    }

    #[test]
    fn halfword_write_into_the_upper_half_merges() {
        let mut s = RegisterStub::new();
        s.write(0x20, 4, 0x1111_2222);
        s.write(0x22, 2, 0xAAAA);
        assert_eq!(s.read(0x20, 4), 0xAAAA_2222);
        assert_eq!(s.read(0x22, 2), 0xAAAA);
        assert_eq!(s.read(0x20, 2), 0x2222);
    }

    #[test]
    fn byte_read_extracts_the_right_byte() {
        let mut s = RegisterStub::new();
        s.write(0x20, 4, 0x4433_2211);
        assert_eq!(s.read(0x20, 1), 0x11);
        assert_eq!(s.read(0x21, 1), 0x22);
        assert_eq!(s.read(0x22, 1), 0x33);
        assert_eq!(s.read(0x23, 1), 0x44);
    }

    #[test]
    fn byte_write_merges() {
        let mut s = RegisterStub::new();
        s.write(0x21, 1, 0xFF);
        assert_eq!(s.read(0x20, 4), 0x0000_FF00);
    }

    #[test]
    fn unaligned_word_write_touches_two_words() {
        let mut s = RegisterStub::new();
        s.write(0x22, 4, 0x4433_2211);
        assert_eq!(s.read(0x20, 4), 0x2211_0000);
        assert_eq!(s.read(0x24, 4), 0x0000_4433);
        assert_eq!(s.read(0x22, 4), 0x4433_2211);
    }

    #[test]
    fn registers_do_not_interfere() {
        let mut s = RegisterStub::new();
        s.write(0x00, 4, 1);
        s.write(0x04, 4, 2);
        assert_eq!(s.read(0x00, 4), 1);
        assert_eq!(s.read(0x04, 4), 2);
    }

    // Byte-array model of the same registers, to check the word arithmetic against.
    fn model_write(mem: &mut [u8; 64], offset: u32, size: u8, value: u32) {
        for i in 0..usize::from(size) {
            mem[offset as usize + i] = (value >> (8 * i)) as u8;
        }
    }

    fn model_read(mem: &[u8; 64], offset: u32, size: u8) -> u32 {
        (0..usize::from(size)).fold(0, |v, i| v | u32::from(mem[offset as usize + i]) << (8 * i))
    }

    #[test]
    fn matches_a_byte_array_at_every_offset_and_size() {
        let values = [0xDEAD_BEEF, 0x0102_0304, 0xFFFF_FFFF, 0x8000_0001];
        for (w_off, w_size, value) in (0..=15u32).flat_map(|o| {
            [1u8, 2, 4]
                .into_iter()
                .flat_map(move |s| values.into_iter().map(move |v| (o, s, v)))
        }) {
            let mut stub = RegisterStub::new();
            let mut model = [0u8; 64];
            // Something already there, so merges are visible.
            stub.write(0, 4, 0x5566_7788);
            stub.write(4, 4, 0x99AA_BBCC);
            stub.write(8, 4, 0x1357_9BDF);
            stub.write(12, 4, 0x2468_ACE0);
            model[..16].copy_from_slice(&[
                0x88, 0x77, 0x66, 0x55, 0xCC, 0xBB, 0xAA, 0x99, 0xDF, 0x9B, 0x57, 0x13, 0xE0, 0xAC,
                0x68, 0x24,
            ]);
            stub.write(w_off, w_size, value);
            model_write(&mut model, w_off, w_size, value);
            for r_off in 0..=15u32 {
                for r_size in [1u8, 2, 4] {
                    assert_eq!(
                        stub.read(r_off, r_size),
                        model_read(&model, r_off, r_size),
                        "write {w_size}@{w_off} {value:#x}, read {r_size}@{r_off}"
                    );
                }
            }
        }
    }

    #[test]
    fn aligned_word_write_makes_one_entry() {
        let mut s = RegisterStub::new();
        s.write(0x20, 4, 1);
        assert_eq!(s.inner().words.len(), 1);
        s.write(0x22, 2, 1);
        s.write(0x21, 1, 1);
        assert_eq!(s.inner().words.len(), 1);
        s.write(0x23, 2, 1);
        assert_eq!(s.inner().words.len(), 2);
    }

    // Registers at 0x1000: R32 (reset 0x11223344) at 0, R16A (0xAABB) at 4, R16B (0xCCDD) at 6,
    // R8 (0x5E) at 9. Bytes 8, 10 and 11 are not covered.
    fn with_resets() -> RegisterStub {
        let reg = |name: &str, offset: u32, bits: u32, reset: u32| {
            format!(
                "<register><name>{name}</name><addressOffset>{offset}</addressOffset>\
                 <size>{bits}</size><resetValue>{reset}</resetValue></register>"
            )
        };
        let xml = format!(
            "<device><peripherals><peripheral><name>P</name><baseAddress>0x1000</baseAddress>\
             <registers>{}{}{}{}</registers></peripheral></peripherals></device>",
            reg("R32", 0, 32, 0x1122_3344),
            reg("R16A", 4, 16, 0xAABB),
            reg("R16B", 6, 16, 0xCCDD),
            reg("R8", 9, 8, 0x5E),
        );
        let svd: &'static Svd = Box::leak(Box::new(Svd::parse(&xml).unwrap()));
        // Mapped at 0x1000 - 0x100, so the registers sit at offset 0x100 in the window.
        RegisterStub::with_reset_values(0xF00, svd)
    }

    #[test]
    fn unwritten_register_reads_its_reset_value() {
        let mut s = with_resets();
        assert_eq!(s.read(0x100, 4), 0x1122_3344);
    }

    #[test]
    fn written_register_reads_back_without_the_reset_value() {
        let mut s = with_resets();
        s.write(0x100, 4, 0x0000_00FF);
        assert_eq!(s.read(0x100, 4), 0x0000_00FF);
    }

    #[test]
    fn byte_write_keeps_the_rest_of_the_reset_value() {
        let mut s = with_resets();
        s.write(0x101, 1, 0x99);
        assert_eq!(s.read(0x100, 4), 0x1122_9944);
    }

    #[test]
    fn registers_sharing_a_word_both_read_right() {
        let mut s = with_resets();
        assert_eq!(s.read(0x104, 2), 0xAABB);
        assert_eq!(s.read(0x106, 2), 0xCCDD);
        assert_eq!(s.read(0x104, 4), 0xCCDD_AABB);
    }

    #[test]
    fn uncovered_bytes_read_zero() {
        let mut s = with_resets();
        assert_eq!(s.read(0x108, 4), 0x0000_5E00);
        assert_eq!(s.read(0x110, 4), 0);
    }

    #[test]
    fn halfword_read_of_the_upper_half_of_a_reset_word() {
        let mut s = with_resets();
        assert_eq!(s.read(0x102, 2), 0x1122);
    }

    #[test]
    fn reset_value_is_looked_up_once_per_word() {
        let mut s = with_resets();
        let first = s.read(0x100, 4);
        assert_eq!(s.read(0x100, 4), first);
        assert_eq!(s.inner().words.len(), 1);
    }

    #[test]
    fn address_outside_every_register_reads_zero() {
        let mut s = with_resets();
        assert_eq!(s.read(0, 4), 0);
    }
}
