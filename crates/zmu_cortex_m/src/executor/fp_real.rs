//! The real numbers the floating-point pseudocode computes in, kept exact on integers.
//!
//! The pseudocode adds, multiplies, divides and takes roots of the operands as real numbers and
//! rounds the result once. A `Real` is `sig * 2^exp` with a 128-bit `sig`, and the operations
//! here return a `Real` that rounds to the same float as the infinitely precise result would:
//! a product is exact, and a sum, a quotient and a root keep a sticky bit in the lowest bit of
//! `sig` when they drop anything. That bit sits far below the last bit of either format, so it
//! decides a tie and a directed rounding the way the dropped bits would, and nothing else.

use super::fp_generic::FloatOps;
use crate::core::fpregister::FPRounding;

// A value `sig * 2^exp` of the given sign. A `sig` of 0 is a zero of either sign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Real {
    pub(crate) sign: bool,
    pub(crate) sig: u128,
    pub(crate) exp: i32,
}

// The significand of a sum is kept with its top bit here, which leaves room for a carry and,
// with a 106-bit product, at least 18 bits below it.
const SUM_TOP: u32 = 124;

// The position of the top bit of a nonzero value.
fn top_bit(sig: u128) -> i32 {
    127 - sig.leading_zeros() as i32
}

// Shifts right, and sets the lowest bit if any set bit was shifted out.
fn shift_right_sticky(sig: u128, by: u32) -> u128 {
    match by {
        0 => sig,
        1..=127 => (sig >> by) | u128::from(sig << (128 - by) != 0),
        _ => u128::from(sig != 0),
    }
}

impl Real {
    pub(crate) const ZERO: Real = Real {
        sign: false,
        sig: 0,
        exp: 0,
    };

    pub(crate) fn is_zero(self) -> bool {
        self.sig == 0
    }

    pub(crate) fn negate(self) -> Real {
        Real {
            sign: !self.sign,
            ..self
        }
    }

    // The value of a whole number scaled by 2^exp.
    pub(crate) fn from_integer(sign: bool, magnitude: u64, exp: i32) -> Real {
        Real {
            sign,
            sig: u128::from(magnitude),
            exp,
        }
    }

    // Exact for operands of up to 64 significant bits, which is every float and integer the FPU
    // takes.
    pub(crate) fn mul(self, other: Real) -> Real {
        Real {
            sign: self.sign != other.sign,
            sig: self.sig * other.sig,
            exp: self.exp + other.exp,
        }
    }

    // The sum, with the top bit of both addends moved to the same place first, so that the
    // smaller one only loses bits below the ones the larger one can use.
    pub(crate) fn add(self, other: Real) -> Real {
        if self.is_zero() {
            return other;
        }
        if other.is_zero() {
            return self;
        }
        let align = |value: Real| {
            let shift = SUM_TOP as i32 - top_bit(value.sig);
            Real {
                sig: value.sig << shift,
                exp: value.exp - shift,
                ..value
            }
        };
        let (mut big, mut small) = (align(self), align(other));
        if (big.exp, big.sig) < (small.exp, small.sig) {
            (big, small) = (small, big);
        }
        let small_sig = shift_right_sticky(small.sig, (big.exp - small.exp) as u32);
        // A difference of equal values has no bits, which is a zero.
        let sig = if big.sign == small.sign {
            big.sig + small_sig
        } else {
            big.sig - small_sig
        };
        Real { sig, ..big }
    }

    // The quotient of two nonzero values. The numerator has its top bit at 126 and the
    // denominator at 63, so the quotient has 63 or 64 bits.
    pub(crate) fn div(self, other: Real) -> Real {
        let up = 126 - top_bit(self.sig);
        let down = 63 - top_bit(other.sig);
        let numerator = self.sig << up;
        let denominator = other.sig << down;
        Real {
            sign: self.sign != other.sign,
            sig: (numerator / denominator) | u128::from(!numerator.is_multiple_of(denominator)),
            exp: self.exp - up - (other.exp - down),
        }
    }

    // The root of a positive value. The exponent is made even first, and the integer root of
    // the significand, which has 63 or 64 bits, takes half of it.
    pub(crate) fn sqrt(self) -> Real {
        let mut up = 126 - top_bit(self.sig);
        if (self.exp - up) & 1 != 0 {
            up -= 1;
        }
        let radicand = self.sig << up;
        let root = radicand.isqrt();
        Real {
            sign: false,
            sig: root | u128::from(radicand != root * root),
            exp: (self.exp - up) / 2,
        }
    }
}

// The significand and the exponent of a finite value, such that it is `significand * 2^exponent`.
// It reads the bits, so it is exact for the whole range of the format.
pub(crate) fn significand<T: FloatOps>(op: T::Bits) -> (u128, i32) {
    let bits: u64 = op.into();
    let exp = ((bits >> T::FRAC_BITS) & ((1 << T::EXP_BITS) - 1)) as i32;
    let frac = u128::from(bits & ((1 << T::FRAC_BITS) - 1));
    if exp == 0 {
        // A subnormal has no leading one, and the exponent of the smallest normal.
        (frac, 1 - T::BIAS - T::FRAC_BITS as i32)
    } else {
        (
            frac | 1 << T::FRAC_BITS,
            exp - T::BIAS - T::FRAC_BITS as i32,
        )
    }
}

// A value rounded to an integer: the magnitude, and whether rounding lost anything.
pub(crate) struct Rounded {
    pub(crate) magnitude: u128,
    pub(crate) inexact: bool,
}

// Rounds the magnitude `significand * 2^exponent` of a value of the given sign to an integer.
// The rounding is done on the bits, so a tie, or a whole number, is seen as one. A magnitude too
// large for 128 bits comes back as the largest.
pub(crate) fn round_magnitude(
    significand: u128,
    exponent: i32,
    sign: bool,
    rounding: FPRounding,
) -> Rounded {
    if exponent >= 0 {
        let magnitude = if exponent <= 64 {
            significand << exponent
        } else {
            u128::MAX
        };
        return Rounded {
            magnitude,
            inexact: false,
        };
    }

    // The bits below the units are the remainder, to compare with a half.
    let shift = exponent.unsigned_abs();
    let (whole, above_half, at_half, inexact) = if shift >= 128 {
        (0, false, false, significand != 0)
    } else {
        let remainder = significand & ((1 << shift) - 1);
        let half = 1 << (shift - 1);
        (
            significand >> shift,
            remainder > half,
            remainder == half,
            remainder != 0,
        )
    };
    let up = match rounding {
        FPRounding::TieEven => above_half || (at_half && whole & 1 == 1),
        FPRounding::TieAway => above_half || at_half,
        FPRounding::PosInf => inexact && !sign,
        FPRounding::NegInf => inexact && sign,
        FPRounding::Zero => false,
    };
    Rounded {
        magnitude: whole + u128::from(up),
        inexact,
    }
}

// The bits of the whole number `magnitude`, which has to be exactly representable.
pub(crate) fn whole_number_bits<T: FloatOps>(sign: bool, magnitude: u64) -> T::Bits {
    let top = 63 - magnitude.leading_zeros() as usize;
    let fraction = (magnitude << (T::FRAC_BITS - top)) & ((1 << T::FRAC_BITS) - 1);
    T::from_integer(
        u64::from(sign) << (T::n() - 1) | (T::BIAS as u64 + top as u64) << T::FRAC_BITS | fraction,
    )
}

// A real value rounded to a float, and what the rounding signalled.
pub(crate) struct Packed<T: FloatOps> {
    pub(crate) bits: T::Bits,
    pub(crate) underflow: bool,
    pub(crate) overflow: bool,
    pub(crate) inexact: bool,
}

// FPRound: the float nearest to a nonzero value in the given direction, or zero for a value
// that is too small for a normal number when flush-to-zero is on. Whether a result is
// subnormal is decided before rounding, as the pseudocode has it.
pub(crate) fn round<T: FloatOps>(value: Real, rounding: FPRounding, flush: bool) -> Packed<T> {
    let sign = value.sign;
    let exponent = value.exp + top_bit(value.sig);
    let min_exponent = 1 - T::BIAS;
    let tiny = exponent < min_exponent;
    if tiny && flush {
        return Packed {
            bits: T::fp_zero(sign),
            underflow: true,
            overflow: false,
            inexact: false,
        };
    }

    // The weight of the last bit kept is the one of a subnormal below the smallest normal.
    let last_bit = if tiny { min_exponent } else { exponent } - T::FRAC_BITS as i32;
    let rounded = round_magnitude(value.sig, value.exp - last_bit, sign, rounding);
    // The exponent field is added to the significand, which holds the implicit bit when the
    // value is normal, so a carry out of the significand carries into the exponent.
    let field = if tiny { 0 } else { exponent + T::BIAS - 1 };
    let packed = (exponent <= T::BIAS)
        .then(|| (field as u64) << T::FRAC_BITS)
        .map(|bits| bits + rounded.magnitude as u64)
        .filter(|bits| bits >> T::FRAC_BITS < (1 << T::EXP_BITS) - 1);
    let Some(packed) = packed else {
        let infinity = match rounding {
            FPRounding::PosInf => !sign,
            FPRounding::NegInf => sign,
            FPRounding::Zero => false,
            FPRounding::TieEven | FPRounding::TieAway => true,
        };
        return Packed {
            bits: if infinity {
                T::fp_infinity(sign)
            } else {
                T::fp_max_normal(sign)
            },
            underflow: false,
            overflow: true,
            inexact: true,
        };
    };
    Packed {
        bits: T::from_integer(u64::from(sign) << (T::n() - 1) | packed),
        underflow: tiny && rounded.inexact,
        overflow: false,
        inexact: rounded.inexact,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn real(sign: bool, sig: u128, exp: i32) -> Real {
        Real { sign, sig, exp }
    }

    // The value of a real as a double, for a real whose significand fits one and whose
    // exponent is in range.
    fn value(real: Real) -> f64 {
        let magnitude = real.sig as f64 * 2f64.powi(real.exp);
        if real.sign { -magnitude } else { magnitude }
    }

    #[test]
    fn shifting_right_keeps_a_sticky_bit_for_what_is_lost() {
        assert_eq!(shift_right_sticky(0b1010, 0), 0b1010);
        assert_eq!(shift_right_sticky(0b1010, 1), 0b101);
        assert_eq!(shift_right_sticky(0b1011, 1), 0b101 | 1);
        assert_eq!(shift_right_sticky(0b1000, 2), 0b10);
        assert_eq!(shift_right_sticky(0b1001, 2), 0b10 | 1);
        assert_eq!(shift_right_sticky(1 << 127, 127), 1);
        assert_eq!(shift_right_sticky((1 << 127) | 1, 127), 1);
        assert_eq!(shift_right_sticky(1 << 127, 128), 1);
        assert_eq!(shift_right_sticky(1, 500), 1);
        assert_eq!(shift_right_sticky(0, 500), 0);
    }

    #[test]
    fn a_sum_is_exact_when_nothing_has_to_be_dropped() {
        assert_eq!(value(real(false, 3, 0).add(real(false, 5, 0))), 8.0);
        assert_eq!(value(real(false, 3, 0).add(real(true, 5, 0))), -2.0);
        assert_eq!(value(real(true, 3, 0).add(real(false, 5, 0))), 2.0);
        assert_eq!(value(real(true, 3, 0).add(real(true, 5, 0))), -8.0);
        assert_eq!(value(real(false, 3, -2).add(real(false, 1, 1))), 2.75);
        assert_eq!(value(real(false, 1, 1).add(real(false, 3, -2))), 2.75);
        // Equal and opposite is a zero, and a zero is the identity.
        assert!(real(false, 7, 3).add(real(true, 7, 3)).is_zero());
        assert!(real(false, 7, 3).add(real(true, 14, 2)).is_zero());
        assert_eq!(real(true, 7, 3).add(Real::ZERO), real(true, 7, 3));
        assert_eq!(Real::ZERO.add(real(true, 7, 3)), real(true, 7, 3));
    }

    #[test]
    fn a_sum_with_a_distant_term_keeps_it_as_a_sticky_bit() {
        // 1 + 2^-1000: the small term only sets the lowest bit, whichever way the sum goes.
        let one = real(false, 1, 0);
        let sum = one.add(real(false, 1, -1000));
        assert_eq!(sum.sig & 1, 1);
        assert_eq!(sum.sig >> 1, 1 << (SUM_TOP - 1));
        assert_eq!(sum.exp + SUM_TOP as i32, 0);
        let difference = one.add(real(true, 1, -1000));
        assert_eq!(difference.sig & 1, 1);
        assert_eq!(difference.sig >> 1, (1 << (SUM_TOP - 1)) - 1);
        // The big term keeps its sign when it is the one that is subtracted from.
        assert!(!sum.sign && !difference.sign);
        assert!(real(true, 1, 0).add(real(false, 1, -1000)).sign);
    }

    #[test]
    fn a_product_is_exact() {
        let product = real(true, (1 << 53) - 1, -52).mul(real(false, (1 << 53) - 3, 10));
        assert_eq!(product.sig, ((1u128 << 53) - 1) * ((1 << 53) - 3));
        assert_eq!(product.exp, -42);
        assert!(product.sign);
    }

    #[test]
    fn a_quotient_has_a_sticky_bit_only_when_it_is_inexact() {
        let exact = real(false, 12, 0).div(real(true, 3, 0));
        assert!(exact.sign);
        assert_eq!(value(exact), -4.0);
        assert!(exact.sig >= 1 << 62);
        let third = real(false, 1, 0).div(real(false, 3, 0));
        assert_eq!(third.sig & 1, 1);
        assert!(third.sig >= 1 << 62 && third.sig < 1 << 64);
        // 2^64 / 3 = 0x5555555555555555.55...
        assert_eq!(value(third) * 3.0, 1.0);
        // The denominator of 64 bits, and a numerator of one bit.
        let small = real(false, 1, 0).div(real(false, u128::from(u64::MAX), 0));
        assert!(small.sig >= 1 << 62 && small.sig < 1 << 64);
    }

    #[test]
    fn a_root_halves_an_even_exponent_and_makes_an_odd_one_even() {
        let four = real(false, 4, 0).sqrt();
        assert_eq!(value(four), 2.0);
        assert_eq!(four.sig & 1, 0);
        // An odd exponent: 2^3 = 8 has the root 2.828..., and the root squared is not 8.
        let eight = real(false, 1, 3).sqrt();
        assert_eq!(eight.sig & 1, 1);
        assert!((value(eight) - 8f64.sqrt()).abs() < 1e-15);
        let nine_over_four = real(false, 9, -2).sqrt();
        assert_eq!(value(nine_over_four), 1.5);
        // A root is a 63 or 64-bit number whatever the operand.
        for sig in [1u128, 2, 3, (1 << 53) - 1, 1 << 52] {
            for exp in [-1074, -3, -2, 0, 1, 970] {
                let root = real(false, sig, exp).sqrt();
                assert!(root.sig >= 1 << 62 && root.sig < 1 << 64, "{sig} {exp}");
            }
        }
    }

    fn rounded<T: FloatOps>(value: Real, rounding: FPRounding) -> (u64, bool, bool, bool) {
        let packed = round::<T>(value, rounding, false);
        (
            packed.bits.into(),
            packed.underflow,
            packed.overflow,
            packed.inexact,
        )
    }

    #[test]
    fn rounding_keeps_an_exact_value_and_signals_nothing() {
        assert_eq!(
            rounded::<u64>(real(false, 1, 0), FPRounding::TieEven),
            (1.0f64.to_bits(), false, false, false)
        );
        assert_eq!(
            rounded::<u64>(real(true, 3, -1), FPRounding::Zero),
            ((-1.5f64).to_bits(), false, false, false)
        );
        // A significand with trailing zeros and one that has to be shifted up are the same value.
        assert_eq!(
            rounded::<u32>(real(false, 1 << 100, -100), FPRounding::PosInf),
            (1.0f32.to_bits().into(), false, false, false)
        );
        assert_eq!(
            rounded::<u32>(real(false, 5, 1), FPRounding::PosInf),
            (10.0f32.to_bits().into(), false, false, false)
        );
    }

    #[test]
    fn rounding_a_tie_goes_to_even_and_the_other_modes_to_their_side() {
        // 2^53 + 1 is halfway between two doubles, 2^53 and 2^53 + 2.
        let tie = real(false, (1 << 53) + 1, 0);
        let below = 9_007_199_254_740_992.0f64.to_bits();
        let above = 9_007_199_254_740_994.0f64.to_bits();
        let at = |rounding| rounded::<u64>(tie, rounding).0;
        assert_eq!(at(FPRounding::TieEven), below);
        assert_eq!(at(FPRounding::TieAway), above);
        assert_eq!(at(FPRounding::PosInf), above);
        assert_eq!(at(FPRounding::NegInf), below);
        assert_eq!(at(FPRounding::Zero), below);
        // The odd neighbour's tie goes up: 2^53 + 3.
        let tie = real(false, (1 << 53) + 3, 0);
        assert_eq!(
            rounded::<u64>(tie, FPRounding::TieEven).0,
            9_007_199_254_740_996.0f64.to_bits()
        );
        // Below zero the directions swap.
        let tie = real(true, (1 << 53) + 1, 0);
        assert_eq!(
            rounded::<u64>(tie, FPRounding::PosInf).0,
            (-9_007_199_254_740_992.0f64).to_bits()
        );
        assert_eq!(
            rounded::<u64>(tie, FPRounding::NegInf).0,
            (-9_007_199_254_740_994.0f64).to_bits()
        );
        // A sticky bit below the tie makes it above half.
        let above_tie = real(false, ((1 << 53) + 1) << 20 | 1, -20);
        assert_eq!(rounded::<u64>(above_tie, FPRounding::TieEven).0, above);
        assert!(rounded::<u64>(above_tie, FPRounding::TieEven).3);
    }

    #[test]
    fn rounding_up_carries_into_the_exponent() {
        // The largest 24-bit significand plus a bit more rounds up to the next power of two.
        let nearly_two = real(false, (1 << 25) - 1, -24);
        assert_eq!(
            rounded::<u32>(nearly_two, FPRounding::TieEven),
            (2.0f32.to_bits().into(), false, false, true)
        );
        let nearly_two = real(false, (1 << 54) - 1, -53);
        assert_eq!(
            rounded::<u64>(nearly_two, FPRounding::TieEven),
            (2.0f64.to_bits(), false, false, true)
        );
    }

    #[test]
    fn rounding_a_subnormal_decides_underflow_before_rounding() {
        // Half the smallest subnormal is a tie, to even, which is zero.
        let half = real(false, 1, -1075);
        assert_eq!(
            rounded::<u64>(half, FPRounding::TieEven),
            (0, true, false, true)
        );
        assert_eq!(
            rounded::<u64>(half, FPRounding::PosInf),
            (1, true, false, true)
        );
        // Exactly the smallest subnormal is not inexact, so it does not underflow.
        assert_eq!(
            rounded::<u64>(real(false, 1, -1074), FPRounding::TieEven),
            (1, false, false, false)
        );
        // Just below the smallest normal rounds up to it, and still underflows.
        let almost = real(false, (1 << 53) - 1, -1075);
        assert_eq!(
            rounded::<u64>(almost, FPRounding::TieEven),
            (f64::MIN_POSITIVE.to_bits(), true, false, true)
        );
        assert_eq!(
            rounded::<u32>(real(false, 1, -150), FPRounding::TieEven),
            (0, true, false, true)
        );
        // The smallest normal itself is not subnormal.
        assert_eq!(
            rounded::<u32>(real(false, 1, -126), FPRounding::TieEven),
            (0x0080_0000, false, false, false)
        );
        // A tiny value far below everything underflows to a zero that has its sign.
        assert_eq!(
            rounded::<u64>(real(true, 1, -5000), FPRounding::TieEven),
            (1 << 63, true, false, true)
        );
        assert_eq!(
            rounded::<u64>(real(true, 1, -5000), FPRounding::NegInf),
            (1 << 63 | 1, true, false, true)
        );
    }

    #[test]
    fn rounding_flushes_a_value_below_the_normal_range_when_asked() {
        let flush = |value: Real| {
            let packed = round::<u64>(value, FPRounding::TieEven, true);
            (
                packed.bits,
                packed.underflow,
                packed.overflow,
                packed.inexact,
            )
        };
        assert_eq!(flush(real(false, 1, -1074)), (0, true, false, false));
        assert_eq!(flush(real(true, 3, -1080)), (1 << 63, true, false, false));
        // Below the normal range before rounding is flushed, though it would round up to it.
        assert_eq!(
            flush(real(false, (1 << 53) - 1, -1075)),
            (0, true, false, false)
        );
        assert_eq!(
            flush(real(false, 1, -1022)),
            (f64::MIN_POSITIVE.to_bits(), false, false, false)
        );
    }

    #[test]
    fn rounding_an_overflow_gives_infinity_or_the_largest_number_by_mode() {
        let max = f64::MAX.to_bits();
        let inf = f64::INFINITY.to_bits();
        let big = real(false, 1, 1024);
        assert_eq!(
            rounded::<u64>(big, FPRounding::TieEven),
            (inf, false, true, true)
        );
        assert_eq!(
            rounded::<u64>(big, FPRounding::TieAway),
            (inf, false, true, true)
        );
        assert_eq!(
            rounded::<u64>(big, FPRounding::PosInf),
            (inf, false, true, true)
        );
        assert_eq!(
            rounded::<u64>(big, FPRounding::NegInf),
            (max, false, true, true)
        );
        assert_eq!(
            rounded::<u64>(big, FPRounding::Zero),
            (max, false, true, true)
        );
        let negative = real(true, 1, 1024);
        assert_eq!(
            rounded::<u64>(negative, FPRounding::NegInf),
            (inf | 1 << 63, false, true, true)
        );
        assert_eq!(
            rounded::<u64>(negative, FPRounding::PosInf),
            (max | 1 << 63, false, true, true)
        );
        // The largest number is not an overflow, and a tie above it is.
        let largest = real(false, (1 << 53) - 1, 971);
        assert_eq!(
            rounded::<u64>(largest, FPRounding::TieEven),
            (max, false, false, false)
        );
        let tie = real(false, (1 << 54) - 1, 970);
        assert_eq!(
            rounded::<u64>(tie, FPRounding::TieEven),
            (inf, false, true, true)
        );
        assert_eq!(
            rounded::<u64>(tie, FPRounding::Zero),
            (max, false, false, true)
        );
        // The same for a single, and for an exponent far out of range.
        assert_eq!(
            rounded::<u32>(real(false, 1, 128), FPRounding::TieEven),
            (0x7F80_0000, false, true, true)
        );
        assert_eq!(
            rounded::<u32>(real(false, 1, 100_000), FPRounding::Zero),
            (0x7F7F_FFFF, false, true, true)
        );
    }

    #[test]
    fn whole_numbers_and_significands_are_read_from_the_bits() {
        assert_eq!(significand::<u64>(1.0f64.to_bits()), (1 << 52, -52));
        assert_eq!(significand::<u64>(1), (1, -1074));
        assert_eq!(significand::<u32>(0x7F7F_FFFF), (0xFF_FFFF, 104));
        assert_eq!(significand::<u32>(0x0000_0003), (3, -149));
        assert_eq!(significand::<u32>(0x0080_0000), (1 << 23, -149));
        assert_eq!(whole_number_bits::<u32>(true, 5), (-5.0f32).to_bits());
        assert_eq!(
            whole_number_bits::<u64>(false, 1 << 40),
            ((1u64 << 40) as f64).to_bits()
        );
        assert_eq!(whole_number_bits::<u64>(false, 3), 3.0f64.to_bits());
    }
}
