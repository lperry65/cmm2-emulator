//! The generator behind the RNG's values: xoshiro256** (Blackman and Vigna, 2018), seeded
//! from one 64-bit number by splitmix64, as its authors recommend. The chip's source is real
//! noise; this is the stand-in. It is not for security, only for values that look random and
//! differ from run to run, and the same from one seed.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Prng {
    s: [u64; 4],
}

impl Prng {
    pub(super) fn new(seed: u64) -> Prng {
        let mut x = seed;
        let mut splitmix = || {
            x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        // Four splitmix outputs are never all zero, which xoshiro cannot start from.
        Prng {
            s: [splitmix(), splitmix(), splitmix(), splitmix()],
        }
    }

    /// A seed from the host: the standard library's hash keys, which it takes from the
    /// operating system's entropy source, so no crate is needed for it.
    pub(super) fn host_seed() -> u64 {
        RandomState::new().build_hasher().finish()
    }

    pub(super) fn next_u32(&mut self) -> u32 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        (result >> 32) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_values() {
        let (mut a, mut b) = (Prng::new(7), Prng::new(7));
        assert!((0..100).all(|_| a.next_u32() == b.next_u32()));
    }

    #[test]
    fn different_seeds_give_different_values() {
        let (mut a, mut b) = (Prng::new(1), Prng::new(2));
        assert_ne!(a.next_u32(), b.next_u32());
    }

    #[test]
    fn splitmix64_of_zero_matches_the_published_first_output() {
        // The reference splitmix64 from seed 0 starts 0xE220A8397B1DCDAF, which is the first
        // state word, so the generator is seeded as the authors give it.
        assert_eq!(Prng::new(0).s[0], 0xE220_A839_7B1D_CDAF);
    }

    #[test]
    fn host_seeds_differ_between_calls() {
        assert_ne!(Prng::host_seed(), Prng::host_seed());
    }
}
