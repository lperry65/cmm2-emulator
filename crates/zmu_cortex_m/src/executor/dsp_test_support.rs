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

/// Values at the edges of the byte and halfword lanes and of the word, for the instructions that
/// work on lanes: each lane edge on its own, with the other lane at an edge too.
pub(super) const LANE_EDGES: [u32; 12] = [
    0x0000_0000,
    0x0000_0001,
    0x7fff_ffff,
    0x8000_0000,
    0xffff_ffff,
    0x8000_7fff,
    0x0000_7fff,
    0x0000_8000,
    0x0000_ffff,
    0x0000_007f,
    0x0000_0080,
    0x0000_00ff,
];

/// A random word, or one at an edge of the lanes, or one with an edge in each halfword
pub(super) fn mixed(rng: &mut Rng) -> u32 {
    let edge = |rng: &mut Rng| LANE_EDGES[rng.next() as usize % LANE_EDGES.len()];
    match rng.next() % 4 {
        0 => edge(rng),
        1 => (edge(rng) << 16) | (edge(rng) & 0xffff),
        _ => rng.next(),
    }
}

/// N, Z, C, V, Q and the four GE bits: the flags that no instruction here may move except Q
pub(super) const FLAG_BITS: u32 = 0xf80f_0000;
pub(super) const Q_BIT: u32 = 1 << 27;

/// What every register holds before the seeds are written, a different value in each, so that a
/// write to the wrong one shows
pub(super) fn sentinel(register: u32) -> u32 {
    0x0101_0101u32.wrapping_mul(register + 0x11)
}

/// A core with R0 to R12 holding their sentinels, then the seeds written in order (a later seed
/// wins when two registers share a number), and the PSR set
pub(super) fn core_with(seeds: &[(u32, u32)], psr: u32) -> crate::Processor {
    let mut core = crate::Processor::new();
    reset_core(&mut core, seeds, psr);
    core
}

/// The same on a core that is already made, which is much cheaper to do many thousand times
pub(super) fn reset_core(core: &mut crate::Processor, seeds: &[(u32, u32)], psr: u32) {
    use crate::core::register::{BaseReg, Reg};
    for register in 0..13 {
        core.set_r(Reg::from(register as u8), sentinel(register));
    }
    for &(register, value) in seeds {
        core.set_r(Reg::from(register as u8), value);
    }
    core.psr.value = psr;
}

/// Decodes and executes the opcode, and says which opcode it was if the core faults
pub(super) fn run(core: &mut crate::Processor, opcode: u32) {
    use super::ExecutorHelper;
    core.execute_internal(&crate::decoder::decode_32(opcode))
        .unwrap_or_else(|fault| panic!("{opcode:#010x} faulted with {fault:?}"));
}

/// Every register of R0 to R12 that is not in `written` still holds what it held before
pub(super) fn assert_untouched(
    core: &crate::Processor,
    before: &[u32; 13],
    written: &[u32],
    context: &str,
) {
    for register in 0..13u32 {
        if !written.contains(&register) {
            assert_eq!(
                core.r0_12[register as usize], before[register as usize],
                "R{register} moved: {context}"
            );
        }
    }
}

pub(super) fn is_udf(instruction: &crate::core::instruction::Instruction) -> bool {
    matches!(
        instruction,
        crate::core::instruction::Instruction::UDF { .. }
    )
}

#[cfg(feature = "has-dsp-ext")]
/// One run of an instruction: the register numbers it uses (in the order of the instruction's
/// operands), what each is seeded with, and the flags the PSR starts with.
pub(super) struct Case {
    pub layout: [u32; 4],
    pub values: [u32; 4],
    pub psr: u32,
}

#[cfg(feature = "has-dsp-ext")]
impl Case {
    pub(super) fn load(&self, core: &mut crate::Processor) {
        let seeds: Vec<(u32, u32)> = self.layout.iter().copied().zip(self.values).collect();
        reset_core(core, &seeds, self.psr);
    }

    pub(super) fn describe(&self) -> String {
        format!(
            "layout {:?} values {:08x?} psr {:#010x}",
            self.layout, self.values, self.psr
        )
    }
}

#[cfg(feature = "has-dsp-ext")]
/// Register layouts for the instructions with Rd, Rn, Rm and, where there is one, Ra (the fourth
/// is ignored by those with three): all different, then Rd = Rn, Rd = Rm, Rd = Ra, Rn = Rm,
/// Ra = Rn, Ra = Rm, and all the same.
pub(super) const LAYOUTS: [[u32; 4]; 8] = [
    [1, 2, 3, 4],
    [2, 2, 3, 4],
    [3, 2, 3, 4],
    [4, 2, 3, 4],
    [1, 2, 2, 4],
    [1, 2, 3, 2],
    [1, 2, 3, 3],
    [2, 2, 2, 2],
];

#[cfg(feature = "has-dsp-ext")]
/// The layouts of the long multiplies, `RdLo`, `RdHi`, Rn, Rm: `RdLo` and `RdHi` are never the same
/// register. All different, then each of `RdLo` and `RdHi` equal to Rn, to Rm, and to both.
pub(super) const LONG_LAYOUTS: [[u32; 4]; 8] = [
    [1, 2, 3, 4],
    [1, 2, 1, 4],
    [1, 2, 2, 4],
    [1, 2, 3, 1],
    [1, 2, 3, 2],
    [1, 2, 3, 3],
    [1, 2, 1, 1],
    [1, 2, 2, 2],
];

#[cfg(feature = "has-dsp-ext")]
/// The cases for a family: the edge values in every operand in every layout, then random ones
/// (`random` of them for each layout) with random flags. `operands` is how many of the four
/// registers count (three or four).
pub(super) fn cases(layouts: &[[u32; 4]], operands: usize, random: usize, seed: u64) -> Vec<Case> {
    const FLAG_PATTERNS: [u32; 4] = [0, FLAG_BITS, 0xa005_0000, 0x5a0a_0000 | Q_BIT];
    let mut cases = Vec::new();
    for &layout in layouts {
        let mut counter = vec![0usize; operands];
        'edges: loop {
            let mut values = [0; 4];
            for (slot, &index) in counter.iter().enumerate() {
                values[slot] = LANE_EDGES[index];
            }
            let psr = FLAG_PATTERNS[cases.len() % FLAG_PATTERNS.len()] & FLAG_BITS;
            cases.push(Case {
                layout,
                values,
                psr,
            });
            for (slot, digit) in counter.iter_mut().enumerate() {
                // the fourth operand, an accumulate, takes only the first six edges
                let count = if slot == 3 { 6 } else { LANE_EDGES.len() };
                *digit += 1;
                if *digit < count {
                    continue 'edges;
                }
                *digit = 0;
            }
            break;
        }
    }
    let mut rng = Rng(seed);
    for &layout in layouts {
        for _ in 0..random {
            let values = [
                mixed(&mut rng),
                mixed(&mut rng),
                mixed(&mut rng),
                mixed(&mut rng),
            ];
            let psr = rng.next() & FLAG_BITS;
            cases.push(Case {
                layout,
                values,
                psr,
            });
        }
    }
    cases
}

/// `SignedSatQ` and `UnsignedSatQ` of the manual on an unbounded integer: the value clamped to the
/// `bits`-bit signed or unsigned range, and whether it had to be.
pub(super) fn saturate_model(value: i128, bits: u32, signed: bool) -> (i128, bool) {
    let (low, high) = if signed {
        (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
    } else {
        (0, (1i128 << bits) - 1)
    };
    let result = value.clamp(low, high);
    (result, result != value)
}
