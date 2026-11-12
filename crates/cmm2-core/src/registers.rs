//! A 32-bit view of a peripheral's registers, for the models that need real register
//! behaviour (ready bits, read-only bits, write-one-to-clear).

use crate::peripheral::Peripheral;

/// Registers as the hardware sees them: whole 32-bit words. `offset` is a multiple of 4,
/// counted from the start of the peripheral's window.
pub trait Registers {
    fn read(&mut self, offset: u32) -> u32;

    /// Stores the bytes of `value` that `mask` selects (0xFF for each byte written) and
    /// leaves the other bytes of the register alone.
    fn write(&mut self, offset: u32, value: u32, mask: u32);
}

/// Turns the 1, 2 and 4 byte accesses of a `Peripheral` into word operations on `R`. An
/// access that crosses a 4-byte boundary becomes two.
#[derive(Debug, Default)]
pub struct WordAccess<R>(R);

impl<R> WordAccess<R> {
    pub fn wrap(registers: R) -> WordAccess<R> {
        WordAccess(registers)
    }

    pub fn inner(&self) -> &R {
        &self.0
    }

    pub fn inner_mut(&mut self) -> &mut R {
        &mut self.0
    }

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

impl<R: Registers> Peripheral for WordAccess<R> {
    fn read(&mut self, offset: u32, size: u8) -> u32 {
        let (base, shift, words) = Self::span(offset, size);
        let mut combined = 0u64;
        for i in 0..words {
            combined |= u64::from(self.0.read(base.wrapping_add(4 * i))) << (32 * i);
        }
        (combined >> shift) as u32 & (u32::MAX >> (32 - 8 * u32::from(size)))
    }

    fn write(&mut self, offset: u32, size: u8, value: u32) {
        let (base, shift, words) = Self::span(offset, size);
        let mask = u64::from(u32::MAX >> (32 - 8 * u32::from(size))) << shift;
        let bits = (u64::from(value) << shift) & mask;
        for i in 0..words {
            self.0.write(
                base.wrapping_add(4 * i),
                (bits >> (32 * i)) as u32,
                (mask >> (32 * i)) as u32,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Four words that remember what they were asked.
    #[derive(Default)]
    struct Toy {
        words: [u32; 4],
        reads: Vec<u32>,
        writes: Vec<(u32, u32, u32)>,
    }

    impl Registers for Toy {
        fn read(&mut self, offset: u32) -> u32 {
            self.reads.push(offset);
            self.words[(offset / 4) as usize]
        }

        fn write(&mut self, offset: u32, value: u32, mask: u32) {
            self.writes.push((offset, value, mask));
            let word = &mut self.words[(offset / 4) as usize];
            *word = *word & !mask | value & mask;
        }
    }

    fn toy() -> WordAccess<Toy> {
        let mut access = WordAccess::wrap(Toy::default());
        access.inner_mut().words = [0x4433_2211, 0x8877_6655, 0, 0];
        access
    }

    #[test]
    fn aligned_word_read_is_one_call() {
        let mut a = toy();
        assert_eq!(a.read(0, 4), 0x4433_2211);
        assert_eq!(a.inner().reads, [0]);
    }

    #[test]
    fn aligned_word_write_is_one_call_with_a_full_mask() {
        let mut a = toy();
        a.write(4, 4, 0xCAFE_F00D);
        assert_eq!(a.inner().writes, [(4, 0xCAFE_F00D, 0xFFFF_FFFF)]);
    }

    #[test]
    fn byte_reads_shift_right_at_every_offset() {
        let mut a = toy();
        for (offset, expected) in [(0, 0x11), (1, 0x22), (2, 0x33), (3, 0x44), (5, 0x66)] {
            assert_eq!(a.read(offset, 1), expected, "offset {offset}");
        }
        assert_eq!(a.inner().reads.len(), 5);
    }

    #[test]
    fn halfword_reads_shift_right_at_every_offset() {
        let mut a = toy();
        for (offset, expected) in [(0, 0x2211), (1, 0x3322), (2, 0x4433), (6, 0x8877)] {
            assert_eq!(a.read(offset, 2), expected, "offset {offset}");
        }
    }

    #[test]
    fn byte_writes_carry_the_mask_of_that_byte() {
        let mut a = toy();
        for offset in 0..4 {
            a.write(offset, 1, 0xAB);
        }
        let expected: Vec<(u32, u32, u32)> = (0..4)
            .map(|i| (0, 0xAB << (8 * i), 0xFF << (8 * i)))
            .collect();
        assert_eq!(a.inner().writes, expected);
    }

    #[test]
    fn halfword_writes_carry_the_mask_of_those_bytes() {
        let mut a = toy();
        a.write(0, 2, 0xBEEF);
        a.write(1, 2, 0xBEEF);
        a.write(2, 2, 0xBEEF);
        assert_eq!(
            a.inner().writes,
            [
                (0, 0x0000_BEEF, 0x0000_FFFF),
                (0, 0x00BE_EF00, 0x00FF_FF00),
                (0, 0xBEEF_0000, 0xFFFF_0000),
            ]
        );
    }

    #[test]
    fn write_only_changes_the_masked_bytes() {
        let mut a = toy();
        a.write(1, 1, 0xAB);
        assert_eq!(a.inner().words[0], 0x4433_AB11);
    }

    #[test]
    fn value_bits_above_the_access_size_are_dropped() {
        let mut a = toy();
        a.write(0, 1, 0x1234_56AB);
        assert_eq!(a.inner().writes, [(0, 0xAB, 0xFF)]);
    }

    #[test]
    fn unaligned_word_write_calls_two_words() {
        let mut a = toy();
        a.write(2, 4, 0xDDCC_BBAA);
        assert_eq!(
            a.inner().writes,
            [(0, 0xBBAA_0000, 0xFFFF_0000), (4, 0x0000_DDCC, 0x0000_FFFF)]
        );
    }

    #[test]
    fn read_across_a_word_boundary_combines_both_words() {
        let mut a = toy();
        assert_eq!(a.read(2, 4), 0x6655_4433);
        assert_eq!(a.inner().reads, [0, 4]);
        assert_eq!(a.read(3, 2), 0x5544);
    }

    #[test]
    fn access_inside_one_word_does_not_touch_the_next() {
        let mut a = toy();
        a.read(2, 2);
        a.write(2, 2, 1);
        assert_eq!(a.inner().reads, [0]);
        assert_eq!(a.inner().writes.len(), 1);
    }
}
