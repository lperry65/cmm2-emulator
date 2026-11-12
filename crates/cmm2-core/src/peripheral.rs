//! Memory-mapped peripherals: the trait a device implements, and the stand-in for the ones
//! that are not modelled yet.

use std::collections::HashMap;

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

/// For peripherals that are not modelled: every register just remembers what was written.
/// Registers are 32 bits wide; narrower accesses read and merge into the word they fall in.
#[derive(Debug, Default)]
pub struct RegisterStub {
    // Keyed by the offset of the word, which is a multiple of 4.
    words: HashMap<u32, u32>,
}

impl RegisterStub {
    pub fn new() -> RegisterStub {
        RegisterStub::default()
    }
}

impl RegisterStub {
    /// The offset of the word an access starts in, the bit shift of its first byte within
    /// that word, and how many words it touches (two only when it crosses a boundary).
    fn span(offset: u32, size: u8) -> (u32, u32, u32) {
        let shift = 8 * (offset & 3);
        let words = if shift + 8 * u32::from(size) > 32 {
            2
        } else {
            1
        };
        (offset & !3, shift, words)
    }
}

impl Peripheral for RegisterStub {
    fn read(&mut self, offset: u32, size: u8) -> u32 {
        let (base, shift, words) = Self::span(offset, size);
        let mut combined = 0u64;
        for i in 0..words {
            let word = self.words.get(&base.wrapping_add(4 * i)).copied();
            combined |= u64::from(word.unwrap_or(0)) << (32 * i);
        }
        (combined >> shift) as u32 & (u32::MAX >> (32 - 8 * u32::from(size)))
    }

    fn write(&mut self, offset: u32, size: u8, value: u32) {
        let (base, shift, words) = Self::span(offset, size);
        let mask = u64::from(u32::MAX >> (32 - 8 * u32::from(size))) << shift;
        let bits = (u64::from(value) << shift) & mask;
        for i in 0..words {
            let word = self.words.entry(base.wrapping_add(4 * i)).or_insert(0);
            *word = (*word & !((mask >> (32 * i)) as u32)) | (bits >> (32 * i)) as u32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(s.words.len(), 1);
        s.write(0x22, 2, 1);
        s.write(0x21, 1, 1);
        assert_eq!(s.words.len(), 1);
        s.write(0x23, 2, 1);
        assert_eq!(s.words.len(), 2);
    }
}
