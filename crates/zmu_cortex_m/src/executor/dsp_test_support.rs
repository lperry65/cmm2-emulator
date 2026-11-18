//! What the tests of the DSP instructions share.

pub(super) struct Rng(pub(super) u64);

impl Rng {
    pub(super) fn next(&mut self) -> u32 {
        // xorshift64, seeded, so the cases are the same on every run
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
}

/// Values at the edges of a 32-bit signed or unsigned number, for the instructions that saturate
/// or take the high word of a product.
#[cfg(feature = "has-dsp-ext")]
pub(super) const EDGES32: [u32; 16] = [
    0x0000_0000,
    0x0000_0001,
    0xffff_ffff,
    0x7fff_ffff,
    0x8000_0000,
    0x8000_0001,
    0x7fff_fffe,
    0x4000_0000,
    0x3fff_ffff,
    0xc000_0000,
    0xbfff_ffff,
    0x0001_0000,
    0x0000_8000,
    0xffff_8000,
    0x0000_ffff,
    0x2000_0000,
];
