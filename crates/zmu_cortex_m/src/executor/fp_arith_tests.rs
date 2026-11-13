//! The FPU's arithmetic against the host's own IEEE operations. The host rounds to nearest, and
//! that is exact for add, subtract, multiply, divide, root and fused multiply-add, so it is the
//! answer to compare with. For the directed roundings, and for the flags, the host's answer is
//! used to find the sign of the error of the rounding (TwoSum for a sum, a fused multiply-add for
//! the others), and that sign says which neighbour each direction picks and whether the result is
//! inexact. Cases the host cannot be asked about are worked out by hand.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::fmt::Debug;

use super::fp_generic::{FloatOps, FloatingPointPublicOperations};
use super::fp_test_support::*;
use crate::Processor;

// A host float type, and the format of the FPU that is the same one.
trait Host: Copy + Debug {
    type Fmt: FloatOps;
    // The range in which the error of a rounding is itself a float, so that the oracle holds.
    const SAFE_MIN: Self;
    const SAFE_MAX: Self;

    fn from_bits(bits: u64) -> Self;
    fn bits(self) -> u64;
    fn plus(self, other: Self) -> Self;
    fn minus(self, other: Self) -> Self;
    fn times(self, other: Self) -> Self;
    fn over(self, other: Self) -> Self;
    fn root(self) -> Self;
    fn fused(self, mul: Self, add: Self) -> Self;
    fn negated(self) -> Self;
    fn magnitude(self) -> Self;
    fn is_nan(self) -> bool;
    fn is_infinite(self) -> bool;
    fn compare(self, other: Self) -> Option<Ordering>;
}

macro_rules! host {
    ($float:ty, $bits:ty, $fmt:ty, $min:expr, $max:expr) => {
        impl Host for $float {
            type Fmt = $fmt;
            const SAFE_MIN: Self = $min;
            const SAFE_MAX: Self = $max;
            fn from_bits(bits: u64) -> Self {
                <$float>::from_bits(bits as $bits)
            }
            fn bits(self) -> u64 {
                u64::from(<$float>::to_bits(self))
            }
            fn plus(self, other: Self) -> Self {
                self + other
            }
            fn minus(self, other: Self) -> Self {
                self - other
            }
            fn times(self, other: Self) -> Self {
                self * other
            }
            fn over(self, other: Self) -> Self {
                self / other
            }
            fn root(self) -> Self {
                self.sqrt()
            }
            fn fused(self, mul: Self, add: Self) -> Self {
                self.mul_add(mul, add)
            }
            fn negated(self) -> Self {
                -self
            }
            fn magnitude(self) -> Self {
                self.abs()
            }
            fn is_nan(self) -> bool {
                <$float>::is_nan(self)
            }
            fn is_infinite(self) -> bool {
                <$float>::is_infinite(self)
            }
            fn compare(self, other: Self) -> Option<Ordering> {
                self.partial_cmp(&other)
            }
        }
    };
}

host!(f64, u64, u64, 1e-280, 1e300);
host!(f32, u32, u32, 1e-28, 1e30);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Sqrt,
}

const OPS: [Op; 5] = [Op::Add, Op::Sub, Op::Mul, Op::Div, Op::Sqrt];

thread_local! {
    // Making a processor allocates its memory, which is slow to do for every operation.
    static PROCESSOR: RefCell<Processor> = RefCell::new(Processor::new());
}

// Runs `operation` on a processor with the given FPSCR, and returns its result and the flags
// that it raised.
fn with_fpscr<R>(fpscr: u32, operation: impl FnOnce(&mut Processor) -> R) -> (R, u32) {
    PROCESSOR.with(|processor| {
        let mut processor = processor.borrow_mut();
        processor.fpscr = fpscr;
        let result = operation(&mut processor);
        (result, processor.fpscr & CUMULATIVE_FLAGS)
    })
}

// What the FPU gives for the operation, as bits, and the flags it raised.
fn fpu<H: Host>(op: Op, fpscr: u32, a: u64, b: u64) -> (u64, u32) {
    let (x, y) = (H::Fmt::from_integer(a), H::Fmt::from_integer(b));
    let (result, raised) = with_fpscr(fpscr, |p| match op {
        Op::Add => p.fp_add::<H::Fmt>(x, y, true),
        Op::Sub => p.fp_sub::<H::Fmt>(x, y, true),
        Op::Mul => p.fp_mul::<H::Fmt>(x, y, true),
        Op::Div => p.fp_div::<H::Fmt>(x, y, true),
        Op::Sqrt => p.fp_sqrt::<H::Fmt>(x, true),
    });
    (result.into(), raised)
}

fn fpu_fma<H: Host>(fpscr: u32, addend: u64, a: u64, b: u64) -> (u64, u32) {
    let [addend, a, b] = [addend, a, b].map(H::Fmt::from_integer);
    let (result, raised) = with_fpscr(fpscr, |p| p.fp_mul_add::<H::Fmt>(addend, a, b, true));
    (result.into(), raised)
}

fn host<H: Host>(op: Op, a: H, b: H) -> H {
    match op {
        Op::Add => a.plus(b),
        Op::Sub => a.minus(b),
        Op::Mul => a.times(b),
        Op::Div => a.over(b),
        Op::Sqrt => a.root(),
    }
}

fn zero<H: Host>() -> H {
    H::from_bits(0)
}

fn is_zero<H: Host>(x: H) -> bool {
    x.compare(zero()) == Some(Ordering::Equal)
}

// The sign of (the exact result - r), where r is the host's rounded result.
fn error_sign<H: Host>(op: Op, a: H, b: H, r: H) -> Ordering {
    let two_sum = |a: H, b: H| {
        let bb = r.minus(a);
        a.minus(r.minus(bb)).plus(b.minus(bb))
    };
    let error = match op {
        Op::Add => two_sum(a, b),
        Op::Sub => two_sum(a, b.negated()),
        Op::Mul => a.fused(b, r.negated()),
        Op::Div => r.negated().fused(b, a),
        Op::Sqrt => r.negated().fused(r, a),
    };
    let ordering = error.compare(zero()).unwrap();
    // The error of a quotient is the residual divided by the divisor.
    if op == Op::Div && b.compare(zero()) == Some(Ordering::Less) {
        ordering.reverse()
    } else {
        ordering
    }
}

// Whether the error of this rounding can be found: the operands and the result are zero or in a
// range where the error is a float, and a zero result is exact.
fn reliable<H: Host>(op: Op, a: H, b: H, r: H) -> bool {
    let in_range = |x: H| {
        is_zero(x)
            || (matches!(
                x.magnitude().compare(H::SAFE_MIN),
                Some(Ordering::Greater | Ordering::Equal)
            ) && matches!(
                x.magnitude().compare(H::SAFE_MAX),
                Some(Ordering::Less | Ordering::Equal)
            ))
    };
    in_range(a)
        && in_range(b)
        && in_range(r)
        && (!is_zero(r) || matches!(op, Op::Add | Op::Sub) || is_zero(a) || is_zero(b))
}

// The neighbours of a rounded result, below and above, found on the bits.
fn neighbours<H: Host>(r: H, error: Ordering) -> (H, H) {
    let positive = r.compare(zero()) == Some(Ordering::Greater);
    let (toward_zero, away) = (H::from_bits(r.bits() - 1), H::from_bits(r.bits() + 1));
    let (below, above) = if positive {
        (toward_zero, away)
    } else {
        (away, toward_zero)
    };
    match error {
        Ordering::Equal => (r, r),
        Ordering::Greater => (r, above),
        Ordering::Less => (below, r),
    }
}

// A generator of pseudo-random numbers, the same on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

// A random number of the format with the edges overrepresented: sparse and nearly full
// fractions, the smallest and largest exponents, and exponents around 1.
fn operand<H: Host>(rng: &mut Rng) -> u64 {
    type F<H> = <H as Host>::Fmt;
    let (e, f) = (F::<H>::EXP_BITS, F::<H>::FRAC_BITS);
    let max_exp = (1 << e) - 1;
    let bias = F::<H>::BIAS as u64;
    let frac = match rng.next() % 4 {
        0 => rng.next() & ((1 << f) - 1),
        1 => (1 << (rng.next() % f as u64)) | (1 << (rng.next() % f as u64)),
        2 => 0,
        _ => ((1 << f) - 1) - (rng.next() % 4),
    };
    let exp = match rng.next() % 4 {
        0 => rng.next() % (max_exp + 1),
        1 => bias - 6 + rng.next() % 13,
        2 => rng.next() % 4,
        _ => max_exp - rng.next() % 4,
    };
    (rng.next() & 1) << (F::<H>::n() - 1) | exp << f | frac
}

// The second operand, half of the time with an exponent close to the first one's, so that sums
// cancel and products and quotients stay in range.
fn near<H: Host>(rng: &mut Rng, target: i64) -> u64 {
    type F<H> = <H as Host>::Fmt;
    let f = F::<H>::FRAC_BITS;
    let b = operand::<H>(rng);
    if rng.next() % 2 == 0 {
        return b;
    }
    let max_exp = (1i64 << F::<H>::EXP_BITS) - 1;
    let exp = (target + (rng.next() % 7) as i64 - 3).clamp(0, max_exp - 1) as u64;
    (b & !(((1 << F::<H>::EXP_BITS) - 1) << f)) | exp << f
}

fn exponent_of<H: Host>(bits: u64) -> i64 {
    ((bits >> H::Fmt::FRAC_BITS) & ((1 << H::Fmt::EXP_BITS) - 1)) as i64
}

fn default_nan<H: Host>() -> u64 {
    H::Fmt::fp_default_nan().into()
}

// The nearest-mode flags the host's result implies, if they can be told.
fn nearest_flags<H: Host>(op: Op, a: H, b: H, r: H) -> Option<u32> {
    let inputs_finite = !a.is_infinite() && !b.is_infinite();
    if r.is_nan() {
        Some(IOC)
    } else if op == Op::Div && is_zero(b) && inputs_finite && !is_zero(a) {
        Some(DZC)
    } else if r.is_infinite() && inputs_finite {
        Some(OFC | IXC)
    } else if reliable(op, a, b, r) {
        Some(if error_sign(op, a, b, r) == Ordering::Equal {
            0
        } else {
            IXC
        })
    } else {
        None
    }
}

fn sweep<H: Host>(op: Op, iterations: u32) {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let bias = H::Fmt::BIAS as i64;
    for _ in 0..iterations {
        let a_bits = operand::<H>(&mut rng);
        let target = match op {
            Op::Mul => bias - exponent_of::<H>(a_bits) + bias,
            Op::Div => exponent_of::<H>(a_bits) - bias + bias,
            _ => exponent_of::<H>(a_bits),
        };
        let b_bits = near::<H>(&mut rng, target);
        let (a, b) = (H::from_bits(a_bits), H::from_bits(b_bits));
        if a.is_nan() || b.is_nan() {
            continue;
        }
        let r = host(op, a, b);
        let expected = if r.is_nan() {
            default_nan::<H>()
        } else {
            r.bits()
        };

        let (bits, raised) = fpu::<H>(op, 0, a_bits, b_bits);
        assert_eq!(bits, expected, "{op:?} {a:?} {b:?} to nearest");
        if let Some(flags) = nearest_flags(op, a, b, r) {
            assert_eq!(raised, flags, "{op:?} {a:?} {b:?} flags");
        }

        if reliable(op, a, b, r) && r.compare(zero()) != Some(Ordering::Equal) {
            let error = error_sign(op, a, b, r);
            let (below, above) = neighbours(r, error);
            let toward_zero = if r.compare(zero()) == Some(Ordering::Greater) {
                below
            } else {
                above
            };
            for (mode, expected) in [(RP, above), (RM, below), (RZ, toward_zero)] {
                let (bits, raised) = fpu::<H>(op, mode, a_bits, b_bits);
                assert_eq!(bits, expected.bits(), "{op:?} {a:?} {b:?} mode {mode:#x}");
                let inexact = if error == Ordering::Equal { 0 } else { IXC };
                assert_eq!(raised, inexact, "{op:?} {a:?} {b:?} mode {mode:#x} flags");
            }
        }
    }
}

#[test]
fn arithmetic_matches_the_host_and_its_error_sign_on_random_f64() {
    for op in OPS {
        sweep::<f64>(op, 40_000);
    }
}

#[test]
fn arithmetic_matches_the_host_and_its_error_sign_on_random_f32() {
    for op in OPS {
        sweep::<f32>(op, 40_000);
    }
}

fn fma_sweep<H: Host>(iterations: u32) {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    let bias = H::Fmt::BIAS as i64;
    for _ in 0..iterations {
        let a_bits = operand::<H>(&mut rng);
        let b_bits = near::<H>(&mut rng, bias);
        let product = exponent_of::<H>(a_bits) + exponent_of::<H>(b_bits) - bias;
        let c_bits = near::<H>(&mut rng, product);
        let [c, a, b] = [c_bits, a_bits, b_bits].map(H::from_bits);
        if a.is_nan() || b.is_nan() || c.is_nan() {
            continue;
        }
        let r = a.fused(b, c);
        let expected = if r.is_nan() {
            default_nan::<H>()
        } else {
            r.bits()
        };
        let (bits, _) = fpu_fma::<H>(0, c_bits, a_bits, b_bits);
        assert_eq!(bits, expected, "fma {a:?} * {b:?} + {c:?}");
    }
}

#[test]
fn fused_multiply_add_matches_the_host_on_random_f64() {
    fma_sweep::<f64>(60_000);
}

#[test]
fn fused_multiply_add_matches_the_host_on_random_f32() {
    fma_sweep::<f32>(60_000);
}

// Every pair of the edge values, to nearest.
fn edge_values<H: Host>(values: &[H]) {
    for op in OPS {
        for &a in values {
            for &b in values {
                let r = host(op, a, b);
                let expected = if r.is_nan() {
                    default_nan::<H>()
                } else {
                    r.bits()
                };
                let (bits, raised) = fpu::<H>(op, 0, a.bits(), b.bits());
                assert_eq!(bits, expected, "{op:?} {a:?} {b:?}");
                if let Some(flags) = nearest_flags(op, a, b, r) {
                    assert_eq!(raised, flags, "{op:?} {a:?} {b:?} flags");
                }
            }
        }
    }
}

#[test]
fn arithmetic_matches_the_host_on_the_edge_values_of_f64() {
    edge_values::<f64>(&F64_VALUES);
}

#[test]
fn arithmetic_matches_the_host_on_the_edge_values_of_f32() {
    edge_values::<f32>(&F32_VALUES);
}

// What the FPU gives and raises for a double operation.
fn d(op: Op, fpscr: u32, a: f64, b: f64) -> (f64, u32) {
    let (bits, raised) = fpu::<f64>(op, fpscr, a.to_bits(), b.to_bits());
    (f64::from_bits(bits), raised)
}

fn same(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits()
}

#[track_caller]
fn assert_d(op: Op, fpscr: u32, a: f64, b: f64, expected: f64, raised: u32) {
    let (result, flags) = d(op, fpscr, a, b);
    assert!(
        same(result, expected) && flags == raised,
        "{op:?} {a:e} {b:e} fpscr {fpscr:#x}: got {result:e} ({:#x}) flags {flags:#x}, \
         expected {expected:e} ({:#x}) flags {raised:#x}",
        result.to_bits(),
        expected.to_bits()
    );
}

const TWO_POW_M53: f64 = 1.0 / 9_007_199_254_740_992.0;
const TWO_POW_M1074: f64 = 5e-324;
const NEXT_AFTER_ONE: f64 = 1.0 + f64::EPSILON;

#[test]
fn a_sum_that_is_a_tie_goes_to_the_even_neighbour() {
    // Half an ulp above 1.0 is a tie, and 1.0 has the even mantissa.
    assert_d(Op::Add, 0, 1.0, TWO_POW_M53, 1.0, IXC);
    // Half an ulp above the next float is a tie too, and this time the one above is even.
    assert_d(
        Op::Add,
        0,
        NEXT_AFTER_ONE,
        TWO_POW_M53,
        1.0 + 2.0 * f64::EPSILON,
        IXC,
    );
    // Above the tie, it goes up.
    assert_d(Op::Add, 0, 1.0, 1.5 * TWO_POW_M53, NEXT_AFTER_ONE, IXC);
    assert_d(Op::Add, 0, 0.1, 0.2, 0.300_000_000_000_000_04, IXC);
    assert_d(Op::Add, 0, 1.0, 2.0, 3.0, 0);
}

#[test]
fn a_sum_keeps_what_lies_far_below_the_last_bit() {
    let tiny = 2f64.powi(-300);
    assert_d(Op::Add, 0, 1.0, tiny, 1.0, IXC);
    assert_d(Op::Add, RP, 1.0, tiny, NEXT_AFTER_ONE, IXC);
    assert_d(Op::Add, RM, 1.0, tiny, 1.0, IXC);
    assert_d(Op::Add, RZ, 1.0, tiny, 1.0, IXC);
    assert_d(Op::Sub, 0, 1.0, tiny, 1.0, IXC);
    assert_d(Op::Sub, RP, 1.0, tiny, 1.0, IXC);
    assert_d(Op::Sub, RM, 1.0, tiny, 1.0 - TWO_POW_M53, IXC);
    assert_d(Op::Sub, RZ, 1.0, tiny, 1.0 - TWO_POW_M53, IXC);
    // A tie with a bit below it is not a tie: 1 + 2^-53 + 2^-105 goes up, to odd.
    assert_d(
        Op::Add,
        0,
        1.0,
        TWO_POW_M53 * NEXT_AFTER_ONE,
        NEXT_AFTER_ONE,
        IXC,
    );
}

#[test]
fn a_sum_that_overflows_gives_infinity_or_the_largest_number_by_mode() {
    // Half an ulp of the largest number is a tie, and the even neighbour is 2^1024.
    let half_ulp = 2f64.powi(970);
    assert_d(Op::Add, 0, f64::MAX, half_ulp, f64::INFINITY, OFC | IXC);
    assert_d(Op::Add, 0, f64::MAX, half_ulp / 2.0, f64::MAX, IXC);
    assert_d(
        Op::Add,
        RP,
        f64::MAX,
        half_ulp / 2.0,
        f64::INFINITY,
        OFC | IXC,
    );
    // Rounding down, a value below 2^1024 is the largest number and nothing overflows.
    assert_d(Op::Add, RZ, f64::MAX, half_ulp, f64::MAX, IXC);
    assert_d(Op::Add, RM, f64::MAX, half_ulp, f64::MAX, IXC);
    assert_d(Op::Add, RP, f64::MIN, -half_ulp, f64::MIN, IXC);
    assert_d(
        Op::Add,
        RM,
        f64::MIN,
        -half_ulp,
        f64::NEG_INFINITY,
        OFC | IXC,
    );
    // 2^1024 itself is out of range in every mode.
    assert_d(Op::Add, RZ, f64::MAX, 2.0 * half_ulp, f64::MAX, OFC | IXC);
    assert_d(Op::Add, RM, f64::MAX, 2.0 * half_ulp, f64::MAX, OFC | IXC);
    assert_d(
        Op::Add,
        RP,
        f64::MAX,
        2.0 * half_ulp,
        f64::INFINITY,
        OFC | IXC,
    );
    assert_d(Op::Add, RP, f64::MIN, -2.0 * half_ulp, f64::MIN, OFC | IXC);
}

#[test]
fn an_exact_zero_sum_has_the_sign_the_mode_gives_it() {
    assert_d(Op::Add, 0, 1.0, -1.0, 0.0, 0);
    assert_d(Op::Add, RP, 1.0, -1.0, 0.0, 0);
    assert_d(Op::Add, RZ, 1.0, -1.0, 0.0, 0);
    assert_d(Op::Add, RM, 1.0, -1.0, -0.0, 0);
    assert_d(Op::Sub, 0, 3.0, 3.0, 0.0, 0);
    assert_d(Op::Sub, RM, 3.0, 3.0, -0.0, 0);
    // Zeros of one sign keep it, whatever the mode.
    assert_d(Op::Add, RM, 0.0, 0.0, 0.0, 0);
    assert_d(Op::Add, 0, -0.0, -0.0, -0.0, 0);
    assert_d(Op::Sub, 0, -0.0, 0.0, -0.0, 0);
    assert_d(Op::Sub, RM, 0.0, -0.0, 0.0, 0);
    assert_d(Op::Add, 0, 0.0, -0.0, 0.0, 0);
    assert_d(Op::Add, RM, 0.0, -0.0, -0.0, 0);
}

#[test]
fn infinities_add_and_subtract_as_the_manual_says() {
    let inf = f64::INFINITY;
    let nan = f64::from_bits(0x7FF8_0000_0000_0000);
    assert_d(Op::Add, 0, inf, 1.0, inf, 0);
    assert_d(Op::Add, 0, 1.0, -inf, -inf, 0);
    assert_d(Op::Add, 0, inf, -inf, nan, IOC);
    assert_d(Op::Sub, 0, inf, inf, nan, IOC);
    assert_d(Op::Sub, 0, -inf, -inf, nan, IOC);
    assert_d(Op::Sub, 0, inf, -inf, inf, 0);
    assert_d(Op::Sub, 0, -inf, inf, -inf, 0);
    assert_d(Op::Sub, 0, 1.0, inf, -inf, 0);
    assert_d(Op::Sub, 0, 1.0, -inf, inf, 0);
    assert_d(Op::Sub, 0, inf, 1.0, inf, 0);
    assert_d(Op::Sub, 0, -inf, 1.0, -inf, 0);
}

#[test]
fn a_third_rounds_by_mode_and_the_neighbours_differ_by_one_ulp() {
    let nearest = 0x3FD5_5555_5555_5555u64;
    for (mode, expected) in [
        (0, nearest),
        (RP, nearest + 1),
        (RM, nearest),
        (RZ, nearest),
    ] {
        let (bits, raised) = fpu::<f64>(Op::Div, mode, 1.0f64.to_bits(), 3.0f64.to_bits());
        assert_eq!((bits, raised), (expected, IXC), "mode {mode:#x}");
    }
    // Below zero the directions swap.
    for (mode, expected) in [
        (0, nearest),
        (RP, nearest),
        (RM, nearest + 1),
        (RZ, nearest),
    ] {
        let (bits, raised) = fpu::<f64>(Op::Div, mode, (-1.0f64).to_bits(), 3.0f64.to_bits());
        assert_eq!((bits ^ 1 << 63, raised), (expected, IXC), "mode {mode:#x}");
    }
    // The same for single precision: 0x3EAAAAAB is above a third.
    for (mode, expected) in [
        (0, 0x3EAA_AAABu64),
        (RP, 0x3EAA_AAAB),
        (RM, 0x3EAA_AAAA),
        (RZ, 0x3EAA_AAAA),
    ] {
        let (bits, raised) = fpu::<f32>(
            Op::Div,
            mode,
            1.0f32.to_bits().into(),
            3.0f32.to_bits().into(),
        );
        assert_eq!((bits, raised), (expected, IXC), "mode {mode:#x}");
    }
}

#[test]
fn a_product_that_overflows_gives_infinity_or_the_largest_number_by_mode() {
    let max = f64::MAX;
    assert_d(Op::Mul, 0, 1e308, 10.0, f64::INFINITY, OFC | IXC);
    assert_d(Op::Mul, RZ, 1e308, 10.0, max, OFC | IXC);
    assert_d(Op::Mul, RM, 1e308, 10.0, max, OFC | IXC);
    assert_d(Op::Mul, RP, 1e308, 10.0, f64::INFINITY, OFC | IXC);
    assert_d(Op::Mul, 0, -1e308, 10.0, f64::NEG_INFINITY, OFC | IXC);
    assert_d(Op::Mul, RZ, -1e308, 10.0, -max, OFC | IXC);
    assert_d(Op::Mul, RP, -1e308, 10.0, -max, OFC | IXC);
    assert_d(Op::Mul, RM, -1e308, 10.0, f64::NEG_INFINITY, OFC | IXC);
    assert_d(Op::Div, 0, max, 0.5, f64::INFINITY, OFC | IXC);
    // Big, but it fits.
    assert_d(Op::Mul, 0, 1e300, 10.0, 1e301, 0);
    assert_d(Op::Mul, 0, 1e300, 7.0, 1e300 * 7.0, IXC);
    assert_d(
        Op::Mul,
        0,
        2f64.powi(1000),
        2f64.powi(23),
        2f64.powi(1023),
        0,
    );
}

#[test]
fn a_result_below_the_normal_range_is_flagged_only_when_it_is_inexact() {
    let min = f64::MIN_POSITIVE;
    // Exact subnormals are not an underflow.
    assert_d(Op::Mul, 0, min, 0.5, min / 2.0, 0);
    assert_d(Op::Div, 0, min, 2.0, min / 2.0, 0);
    assert_d(
        Op::Add,
        0,
        TWO_POW_M1074,
        TWO_POW_M1074,
        2.0 * TWO_POW_M1074,
        0,
    );
    assert_d(Op::Sub, 0, 1.5 * min, min, min / 2.0, 0);
    assert_d(Op::Sqrt, 0, TWO_POW_M1074, 0.0, 2f64.powi(-537), 0);
    // Inexact ones are, with the inexact flag.
    assert_d(Op::Mul, 0, TWO_POW_M1074, 0.5, 0.0, UFC | IXC);
    assert_d(Op::Mul, RP, TWO_POW_M1074, 0.5, TWO_POW_M1074, UFC | IXC);
    assert_d(Op::Mul, 0, TWO_POW_M1074, 0.75, TWO_POW_M1074, UFC | IXC);
    assert_d(Op::Mul, 0, -TWO_POW_M1074, 0.5, -0.0, UFC | IXC);
    assert_d(Op::Mul, RM, -TWO_POW_M1074, 0.5, -TWO_POW_M1074, UFC | IXC);
    assert_d(Op::Div, 0, 1e-300, 1e10, 1e-310, UFC | IXC);
    // Below the normal range before rounding is an underflow, even when the rounding makes the
    // result the smallest normal number.
    assert_d(Op::Mul, 0, min, 1.0 - TWO_POW_M53, min, UFC | IXC);
}

#[test]
fn square_roots_round_by_mode_and_signal_a_negative_operand() {
    let nearest = 0x3FF6_A09E_667F_3BCDu64;
    for (mode, expected) in [
        (0, nearest),
        (RP, nearest),
        (RM, nearest - 1),
        (RZ, nearest - 1),
    ] {
        let (bits, raised) = fpu::<f64>(Op::Sqrt, mode, 2.0f64.to_bits(), 0);
        assert_eq!((bits, raised), (expected, IXC), "mode {mode:#x}");
    }
    let nan = f64::from_bits(0x7FF8_0000_0000_0000);
    assert_d(Op::Sqrt, 0, 4.0, 0.0, 2.0, 0);
    assert_d(Op::Sqrt, 0, 1e300, 0.0, 1e150, IXC);
    assert_d(Op::Sqrt, 0, -1.0, 0.0, nan, IOC);
    assert_d(Op::Sqrt, 0, f64::NEG_INFINITY, 0.0, nan, IOC);
    assert_d(Op::Sqrt, 0, -0.0, 0.0, -0.0, 0);
    assert_d(Op::Sqrt, 0, f64::INFINITY, 0.0, f64::INFINITY, 0);
    assert_d(Op::Sqrt, 0, f64::MAX, 0.0, f64::MAX.sqrt(), IXC);
}

#[test]
fn division_and_multiplication_by_zero_and_infinity_signal_as_the_manual_says() {
    let inf = f64::INFINITY;
    let nan = f64::from_bits(0x7FF8_0000_0000_0000);
    assert_d(Op::Div, 0, 1.0, 0.0, inf, DZC);
    assert_d(Op::Div, 0, -1.0, 0.0, -inf, DZC);
    assert_d(Op::Div, 0, 1.0, -0.0, -inf, DZC);
    assert_d(Op::Div, 0, 0.0, 0.0, nan, IOC);
    assert_d(Op::Div, 0, inf, inf, nan, IOC);
    assert_d(Op::Div, 0, inf, 0.0, inf, 0);
    assert_d(Op::Div, 0, 1.0, inf, 0.0, 0);
    assert_d(Op::Div, 0, -1.0, inf, -0.0, 0);
    assert_d(Op::Div, 0, 0.0, 5.0, 0.0, 0);
    assert_d(Op::Mul, 0, 0.0, inf, nan, IOC);
    assert_d(Op::Mul, 0, inf, -0.0, nan, IOC);
    assert_d(Op::Mul, 0, -0.0, 5.0, -0.0, 0);
    assert_d(Op::Mul, 0, inf, -2.0, -inf, 0);
}

#[test]
fn a_fused_multiply_add_rounds_once() {
    let fma = |mode: u32, addend: f64, a: f64, b: f64| {
        let (bits, raised) = fpu_fma::<f64>(mode, addend.to_bits(), a.to_bits(), b.to_bits());
        (f64::from_bits(bits), raised)
    };
    // (1 + e)(1 - e/2) is 1 + 2^-53 - 2^-105, which a rounded product would make 1, and the
    // difference from 1 would be 0.
    let e = f64::EPSILON;
    assert_eq!(
        fma(0, -1.0, 1.0 + e, 1.0 - e / 2.0),
        (TWO_POW_M53 - 2f64.powi(-105), 0)
    );
    // The product overflows, and the addend does not help.
    assert_eq!(fma(0, 0.0, 1e300, 1e300), (f64::INFINITY, OFC | IXC));
    assert_eq!(fma(RZ, 0.0, 1e300, 1e300), (f64::MAX, OFC | IXC));
    // A tie in the sum, and a bit below the tie in the product.
    assert_eq!(fma(0, 1.0, TWO_POW_M53, 1.0), (1.0, IXC));
    assert_eq!(
        fma(0, 1.0, TWO_POW_M53, NEXT_AFTER_ONE),
        (NEXT_AFTER_ONE, IXC)
    );
    assert_eq!(fma(RZ, 1.0, TWO_POW_M53, NEXT_AFTER_ONE), (1.0, IXC));
    assert_eq!(fma(RP, 1.0, TWO_POW_M53, 1.0), (NEXT_AFTER_ONE, IXC));
    // The addend cancels the product exactly: the sign of the zero follows the mode.
    for (mode, negative) in [(0, false), (RP, false), (RZ, false), (RM, true)] {
        let (result, raised) = fma(mode, -6.0, 2.0, 3.0);
        assert_eq!(
            (result.is_sign_negative(), result == 0.0, raised),
            (negative, true, 0)
        );
    }
    // A zero product keeps the sign rule of a sum of zeros.
    assert_eq!(fma(0, -0.0, -0.0, 5.0).0.to_bits(), (-0.0f64).to_bits());
    assert_eq!(fma(0, 0.0, -0.0, 5.0).0.to_bits(), 0.0f64.to_bits());
    assert_eq!(fma(RM, 0.0, -0.0, 5.0).0.to_bits(), (-0.0f64).to_bits());
}

const QNAN_A: u64 = 0x7FF8_0000_0000_0123;
const QNAN_B: u64 = 0xFFF8_0000_0000_0456;
const SNAN_A: u64 = 0x7FF0_0000_0000_0789;
const SNAN_B: u64 = 0xFFF0_0000_0000_0ABC;
const DEFAULT_NAN: u64 = 0x7FF8_0000_0000_0000;
const ONE: u64 = 0x3FF0_0000_0000_0000;

#[track_caller]
fn assert_bits(op: Op, fpscr: u32, a: u64, b: u64, expected: u64, raised: u32) {
    assert_eq!(
        fpu::<f64>(op, fpscr, a, b),
        (expected, raised),
        "{op:?} {a:#x} {b:#x} fpscr {fpscr:#x}"
    );
}

#[test]
fn a_nan_operand_comes_through_quieted_and_the_first_signalling_one_wins() {
    for op in [Op::Add, Op::Sub, Op::Mul, Op::Div] {
        // A quiet NaN passes unchanged, whichever side it is on, and raises nothing.
        assert_bits(op, 0, QNAN_A, ONE, QNAN_A, 0);
        assert_bits(op, 0, ONE, QNAN_B, QNAN_B, 0);
        assert_bits(op, 0, QNAN_A, QNAN_B, QNAN_A, 0);
        // A signalling one is quieted and signals, with its sign and payload kept.
        assert_bits(op, 0, SNAN_A, ONE, SNAN_A | 1 << 51, IOC);
        assert_bits(op, 0, ONE, SNAN_B, SNAN_B | 1 << 51, IOC);
        assert_bits(op, 0, SNAN_A, SNAN_B, SNAN_A | 1 << 51, IOC);
        // It wins over a quiet NaN, from either side.
        assert_bits(op, 0, QNAN_A, SNAN_B, SNAN_B | 1 << 51, IOC);
        assert_bits(op, 0, SNAN_A, QNAN_B, SNAN_A | 1 << 51, IOC);
        // With default NaN mode every one of them gives the default NaN, and the signalling
        // ones still signal.
        assert_bits(op, DN, QNAN_A, ONE, DEFAULT_NAN, 0);
        assert_bits(op, DN, ONE, SNAN_B, DEFAULT_NAN, IOC);
    }
    // A NaN wins over the invalid operation too: it is what a NaN operand asks for.
    let inf = f64::INFINITY.to_bits();
    assert_bits(Op::Mul, 0, QNAN_A, inf, QNAN_A, 0);
    assert_bits(Op::Sqrt, 0, QNAN_B, 0, QNAN_B, 0);
    assert_bits(Op::Sqrt, 0, SNAN_A, 0, SNAN_A | 1 << 51, IOC);
    assert_bits(Op::Sqrt, DN, QNAN_A, 0, DEFAULT_NAN, 0);
    // A NaN in a single is the same: the quiet bit is 22.
    let (bits, raised) = fpu::<f32>(Op::Add, 0, 0x7F80_0001, 0x3F80_0000);
    assert_eq!((bits, raised), (0x7FC0_0001, IOC));
    let (bits, raised) = fpu::<f32>(Op::Mul, DN, 0xFFC0_0005, 0x3F80_0000);
    assert_eq!((bits, raised), (0x7FC0_0000, 0));
}

#[test]
fn a_fused_multiply_add_takes_its_nan_from_the_addend_then_the_first_operand() {
    let fma = |fpscr, addend, a, b| fpu_fma::<f64>(fpscr, addend, a, b);
    let quiet = |nan: u64| nan | 1 << 51;
    assert_eq!(fma(0, QNAN_A, QNAN_B, ONE), (QNAN_A, 0));
    assert_eq!(fma(0, ONE, QNAN_B, QNAN_A), (QNAN_B, 0));
    assert_eq!(fma(0, ONE, ONE, QNAN_A), (QNAN_A, 0));
    // A signalling NaN beats a quiet one, and the addend comes first of the signalling ones.
    assert_eq!(fma(0, QNAN_A, ONE, SNAN_B), (quiet(SNAN_B), IOC));
    assert_eq!(fma(0, ONE, SNAN_A, SNAN_B), (quiet(SNAN_A), IOC));
    assert_eq!(fma(0, SNAN_B, SNAN_A, SNAN_A), (quiet(SNAN_B), IOC));
    assert_eq!(fma(DN, ONE, ONE, QNAN_A), (DEFAULT_NAN, 0));
    // Infinity times zero is invalid, with a quiet NaN addend too, which does not win here.
    let (inf, zero) = (f64::INFINITY.to_bits(), 0);
    assert_eq!(fma(0, ONE, inf, zero), (DEFAULT_NAN, IOC));
    assert_eq!(fma(0, QNAN_A, zero, inf), (DEFAULT_NAN, IOC));
    // The sum of an infinite product and an infinite addend of the other sign is invalid.
    let (neg_one, neg_inf) = ((-1.0f64).to_bits(), f64::NEG_INFINITY.to_bits());
    assert_eq!(fma(0, inf, neg_one, inf), (DEFAULT_NAN, IOC));
    assert_eq!(fma(0, inf, ONE, inf), (inf, 0));
    assert_eq!(fma(0, neg_inf, ONE, ONE), (neg_inf, 0));
    assert_eq!(fma(0, ONE, neg_one, inf), (neg_inf, 0));
}

#[test]
fn flush_to_zero_flushes_subnormal_inputs_and_results() {
    let min = f64::MIN_POSITIVE;
    let sub = TWO_POW_M1074;
    // A subnormal input is a zero, and says so with the input denormal flag.
    assert_d(Op::Add, FZ, sub, 1.0, 1.0, IDC);
    assert_d(Op::Add, FZ, 1.0, -sub, 1.0, IDC);
    assert_d(Op::Add, FZ, sub, sub, 0.0, IDC);
    assert_d(Op::Add, FZ, -sub, -sub, -0.0, IDC);
    assert_d(Op::Mul, FZ, sub, 3.0, 0.0, IDC);
    assert_d(Op::Div, FZ, 3.0, sub, f64::INFINITY, IDC | DZC);
    assert_d(Op::Sqrt, FZ, sub, 0.0, 0.0, IDC);
    // Zeros and normal numbers raise nothing, and neither does a subnormal with FZ off.
    assert_d(Op::Add, FZ, min, 0.0, min, 0);
    assert_d(Op::Mul, FZ, -0.0, min, -0.0, 0);
    assert_d(Op::Add, 0, sub, 1.0, 1.0, IXC);
    assert_d(Op::Add, 0, sub, sub, 2.0 * sub, 0);
    // A result below the normal range is flushed, to a zero of its sign, with the underflow
    // flag and no inexact flag. It is decided before rounding.
    assert_d(Op::Mul, FZ, min, 0.5, 0.0, UFC);
    assert_d(Op::Mul, FZ, -min, 0.5, -0.0, UFC);
    assert_d(Op::Div, FZ, min, 4.0, 0.0, UFC);
    assert_d(Op::Sub, FZ, 1.5 * min, min, 0.0, UFC);
    assert_d(Op::Mul, FZ, min, 1.0 - TWO_POW_M53, 0.0, UFC);
    assert_d(Op::Mul, FZ | RP, min, 0.5, 0.0, UFC);
    // The smallest normal number is not flushed.
    assert_d(Op::Mul, FZ, min, 1.0, min, 0);
    assert_d(Op::Add, FZ, min, 0.0, min, 0);
    // The same in a single.
    let (bits, raised) = fpu::<f32>(Op::Add, FZ, 1, 0x3F80_0000);
    assert_eq!((bits, raised), (0x3F80_0000, IDC));
    let (bits, raised) = fpu::<f32>(Op::Mul, FZ, 0x0080_0000, 0x3F00_0000);
    assert_eq!((bits, raised), (0, UFC));
    // The single operations of the FPU that are not told to follow the FPSCR use its standard
    // value, which has FZ and DN set.
    let mut p = Processor::new();
    let result = p.fp_add::<u64>(sub.to_bits(), 1.0f64.to_bits(), false);
    assert_eq!((result, p.fpscr & CUMULATIVE_FLAGS), (ONE, IDC));
    let result = p.fp_add::<u64>(QNAN_A, ONE, false);
    assert_eq!(result, DEFAULT_NAN);
}

fn compare<H: Host>(
    fpscr: u32,
    quiet_nan_exc: bool,
    a: u64,
    b: u64,
) -> ((bool, bool, bool, bool), u32) {
    let (x, y) = (H::Fmt::from_integer(a), H::Fmt::from_integer(b));
    with_fpscr(fpscr, |p| p.fp_compare::<H::Fmt>(x, y, quiet_nan_exc, true))
}

// The condition flags of a compare, N, Z, C, V.
fn expected_flags(order: Option<Ordering>) -> (bool, bool, bool, bool) {
    match order {
        Some(Ordering::Less) => (true, false, false, false),
        Some(Ordering::Equal) => (false, true, true, false),
        Some(Ordering::Greater) => (false, false, true, false),
        None => (false, false, true, true),
    }
}

fn compare_edge_values<H: Host>(values: &[H]) {
    for &a in values {
        for &b in values {
            assert_eq!(
                compare::<H>(0, false, a.bits(), b.bits()),
                (expected_flags(a.compare(b)), 0),
                "{a:?} {b:?}"
            );
        }
    }
}

#[test]
fn compare_orders_every_pair_of_edge_values_like_the_host() {
    compare_edge_values::<f64>(&F64_VALUES);
    compare_edge_values::<f32>(&F32_VALUES);
}

#[test]
fn compare_with_a_nan_is_unordered_and_signals_by_kind() {
    let one = ONE;
    // VCMP signals only for a signalling NaN, VCMPE for any NaN.
    for (quiet_nan_exc, nan, raised) in [
        (false, QNAN_A, 0),
        (true, QNAN_A, IOC),
        (false, SNAN_A, IOC),
        (true, SNAN_B, IOC),
    ] {
        let unordered = (false, false, true, true);
        assert_eq!(
            compare::<f64>(0, quiet_nan_exc, nan, one),
            (unordered, raised)
        );
        assert_eq!(
            compare::<f64>(0, quiet_nan_exc, one, nan),
            (unordered, raised)
        );
        assert_eq!(
            compare::<f64>(0, quiet_nan_exc, nan, nan),
            (unordered, raised)
        );
    }
    // A zero is equal to a zero of the other sign.
    assert_eq!(
        compare::<f64>(0, false, 0, 1 << 63),
        ((false, true, true, false), 0)
    );
}

#[test]
fn compare_flushes_subnormals_when_fz_is_set() {
    let sub = TWO_POW_M1074.to_bits();
    assert_eq!(
        compare::<f64>(0, false, sub, 0),
        ((false, false, true, false), 0)
    );
    assert_eq!(
        compare::<f64>(FZ, false, sub, 0),
        ((false, true, true, false), IDC)
    );
    assert_eq!(
        compare::<f64>(FZ, false, sub, ONE),
        ((true, false, false, false), IDC)
    );
    assert_eq!(
        compare::<f32>(FZ, false, 1, 0x8000_0000),
        ((false, true, true, false), IDC)
    );
}

fn convert<S: Host, D: Host>(fpscr: u32, bits: u64) -> (u64, u32) {
    let source = S::Fmt::from_integer(bits);
    let (result, raised) = with_fpscr(fpscr, |p| p.fp_convert::<S::Fmt, D::Fmt>(source));
    (result.into(), raised)
}

#[test]
fn converting_a_double_to_a_single_rounds_like_the_host_and_flags_what_it_loses() {
    for &x in &F64_VALUES {
        let expected = (x as f32).to_bits();
        let (bits, raised) = convert::<f64, f32>(0, x.to_bits());
        assert_eq!(bits, u64::from(expected), "{x:e}");
        let lost = f64::from(x as f32) != x && x.is_finite();
        let overflow = (x as f32).is_infinite() && x.is_finite();
        let tiny = lost && x.abs() < f64::from(f32::MIN_POSITIVE);
        let expected_flags = if overflow {
            OFC | IXC
        } else if lost {
            IXC | if tiny { UFC } else { 0 }
        } else {
            0
        };
        assert_eq!(raised, expected_flags, "{x:e}");
    }
    // Directed: 1e300 is too large for a single, and 1/3 sits between two.
    assert_eq!(
        convert::<f64, f32>(RZ, 1e300f64.to_bits()),
        (0x7F7F_FFFF, OFC | IXC)
    );
    assert_eq!(
        convert::<f64, f32>(RP, 1e300f64.to_bits()),
        (0x7F80_0000, OFC | IXC)
    );
    let third = (1.0f64 / 3.0).to_bits();
    assert_eq!(convert::<f64, f32>(RP, third), (0x3EAA_AAAB, IXC));
    assert_eq!(convert::<f64, f32>(RM, third), (0x3EAA_AAAA, IXC));
    // Flush to zero takes a subnormal input and a subnormal result.
    assert_eq!(convert::<f64, f32>(FZ, 5e-324f64.to_bits()), (0, IDC));
    assert_eq!(convert::<f64, f32>(FZ, 1e-40f64.to_bits()), (0, UFC));
}

#[test]
fn converting_a_single_to_a_double_is_exact_and_flushes_only_when_fz_is_set() {
    for &x in &F32_VALUES {
        assert_eq!(
            convert::<f32, f64>(0, u64::from(x.to_bits())),
            (f64::from(x).to_bits(), 0),
            "{x:e}"
        );
    }
    assert_eq!(convert::<f32, f64>(FZ, 1), (0, IDC));
    assert_eq!(convert::<f32, f64>(FZ, 0x8000_0001), (1 << 63, IDC));
}

#[test]
fn converting_a_nan_keeps_its_sign_and_top_fraction_bits_and_quiets_it() {
    // A quiet NaN: the top 22 bits of the fraction go across.
    assert_eq!(
        convert::<f64, f32>(0, 0xFFF8_0000_2000_0000),
        (0xFFC0_0001, 0)
    );
    assert_eq!(
        convert::<f32, f64>(0, 0x7FC0_0001),
        (0x7FF8_0000_2000_0000, 0)
    );
    // A signalling NaN is quieted, and signals. Narrowed it can have nothing left in the
    // fraction but the quiet bit.
    assert_eq!(convert::<f64, f32>(0, SNAN_A), (0x7FC0_0000, IOC));
    assert_eq!(
        convert::<f32, f64>(0, 0xFF80_0001),
        (0xFFF8_0000_2000_0000, IOC)
    );
    // Default NaN mode gives the default NaN, signalling or not.
    assert_eq!(convert::<f64, f32>(DN, QNAN_B), (0x7FC0_0000, 0));
    assert_eq!(convert::<f32, f64>(DN, 0x7F80_0001), (DEFAULT_NAN, IOC));
    // An infinity and a zero keep their sign.
    assert_eq!(
        convert::<f64, f32>(0, f64::NEG_INFINITY.to_bits()),
        (0xFF80_0000, 0)
    );
    assert_eq!(convert::<f32, f64>(0, 0x8000_0000), (1 << 63, 0));
}

fn to_float<H: Host>(fpscr: u32, raw: u32, fraction_bits: usize, unsigned: bool) -> (u64, u32) {
    let (result, raised) = with_fpscr(fpscr, |p| {
        p.fixed_to_fp::<H::Fmt, u32>(raw, fraction_bits, unsigned, false, true)
    });
    (result.into(), raised)
}

#[test]
fn converting_an_integer_to_a_float_rounds_by_the_fpscr_mode() {
    let integers = [
        0,
        1,
        2,
        0xFFFF,
        0x00FF_FFFF,
        0x0100_0000,
        0x0100_0001,
        0x0100_0002,
        0x0100_0003,
        0x7FFF_FFFF,
        0x8000_0000,
        0x8000_0001,
        0xFFFF_FFFF,
        0xFFFF_FF80,
        0xFFFF_FFC0,
    ];
    for raw in integers {
        for unsigned in [true, false] {
            let value = if unsigned {
                f64::from(raw)
            } else {
                f64::from(raw as i32)
            };
            // A double holds every 32-bit integer.
            assert_eq!(
                to_float::<f64>(0, raw, 0, unsigned),
                (value.to_bits(), 0),
                "{raw:#x}"
            );

            // A single rounds to nearest, a tie to even, as the host does.
            let nearest = value as f32;
            let exact = f64::from(nearest) == value;
            let inexact = if exact { 0 } else { IXC };
            assert_eq!(
                to_float::<f32>(0, raw, 0, unsigned),
                (u64::from(nearest.to_bits()), inexact),
                "{raw:#x} {unsigned}"
            );

            // The neighbours of the magnitude, below and above it, are one ulp apart.
            let near = nearest.abs().to_bits();
            let (below, above) = match (exact, f64::from(nearest.abs()) > value.abs()) {
                (true, _) => (near, near),
                (false, true) => (near - 1, near),
                (false, false) => (near, near + 1),
            };
            let negative = value < 0.0;
            let signed = |magnitude: u32| u64::from(magnitude | u32::from(negative) << 31);
            let (toward_minus, toward_plus) = if negative {
                (above, below)
            } else {
                (below, above)
            };
            for (mode, expected) in [
                (RM, signed(toward_minus)),
                (RP, signed(toward_plus)),
                (RZ, signed(below)),
            ] {
                assert_eq!(
                    to_float::<f32>(mode, raw, 0, unsigned),
                    (expected, inexact),
                    "{raw:#x} {unsigned} mode {mode:#x}"
                );
            }
        }
    }
}

#[test]
fn converting_an_integer_with_fraction_bits_scales_it_exactly() {
    // 3 with one fraction bit is 1.5; a negative signed value keeps its sign.
    assert_eq!(to_float::<f64>(0, 3, 1, false), (1.5f64.to_bits(), 0));
    assert_eq!(
        to_float::<f64>(0, 0xFFFF_FFFD, 1, false),
        ((-1.5f64).to_bits(), 0)
    );
    assert_eq!(
        to_float::<f64>(0, 0xFFFF_FFFD, 1, true),
        (2_147_483_646.5f64.to_bits(), 0)
    );
    assert_eq!(to_float::<f32>(0, 1, 32, true), (0x2F80_0000, 0));
    // 2^-32 * (2^32 - 1) is not a single: it rounds up to 1.
    assert_eq!(
        to_float::<f32>(0, 0xFFFF_FFFF, 32, true),
        (0x3F80_0000, IXC)
    );
    assert_eq!(
        to_float::<f32>(RZ, 0xFFFF_FFFF, 32, true),
        (0x3F7F_FFFF, IXC)
    );
    // Zero is a positive zero, whatever the mode.
    assert_eq!(to_float::<f64>(RM, 0, 5, false), (0, 0));
}

#[test]
fn a_value_far_out_of_the_old_range_takes_a_bounded_time() {
    // These used to loop for ever in the decimal type. Every operation takes the same steps for
    // any operands, so a thousand of them take a moment.
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        assert_d(Op::Mul, 0, 1e300, 10.0, 1e301, 0);
        assert_d(Op::Div, 0, 1e-120, 10.0, 1e-121, 0);
        assert_d(Op::Div, 0, 1e-300, 1e10, 1e-310, UFC | IXC);
        assert_d(
            Op::Mul,
            0,
            1e308 * 10.0,
            0.0,
            f64::from_bits(DEFAULT_NAN),
            IOC,
        );
        assert_d(Op::Add, 0, f64::MAX, f64::MAX, f64::INFINITY, OFC | IXC);
        assert_d(Op::Sqrt, 0, 1e-320, 0.0, 1e-320f64.sqrt(), IXC);
    }
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
}
