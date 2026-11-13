//! What the floating-point tests in more than one file share: the values to try, the host's
//! own answers to compare with, and the FPSCR bits to look for.

use crate::core::fpregister::FPRounding;

pub(crate) const ALL_ROUNDINGS: [FPRounding; 5] = [
    FPRounding::TieEven,
    FPRounding::PosInf,
    FPRounding::NegInf,
    FPRounding::Zero,
    FPRounding::TieAway,
];

// What the host says each rounding does, for the same value.
pub(crate) fn host_rint64(x: f64, rounding: FPRounding) -> f64 {
    match rounding {
        FPRounding::TieEven => x.round_ties_even(),
        FPRounding::PosInf => x.ceil(),
        FPRounding::NegInf => x.floor(),
        FPRounding::Zero => x.trunc(),
        FPRounding::TieAway => x.round(),
    }
}

pub(crate) fn host_rint32(x: f32, rounding: FPRounding) -> f32 {
    match rounding {
        FPRounding::TieEven => x.round_ties_even(),
        FPRounding::PosInf => x.ceil(),
        FPRounding::NegInf => x.floor(),
        FPRounding::Zero => x.trunc(),
        FPRounding::TieAway => x.round(),
    }
}

// Ties, signs, zeros, the last values with a fraction, the first without, the largest finite
// values, subnormals and infinities. NaNs have tests of their own.
pub(crate) const F64_VALUES: [f64; 40] = [
    0.0,
    -0.0,
    0.25,
    -0.25,
    0.5,
    -0.5,
    0.75,
    1.5,
    -1.5,
    2.5,
    -2.5,
    3.5,
    -3.5,
    1.0,
    -1.0,
    2.7,
    -2.7,
    0.499_999_999_999_999_94,
    4_503_599_627_370_495.5,
    -4_503_599_627_370_495.5,
    4_503_599_627_370_496.0,
    4_503_599_627_370_497.0,
    9_007_199_254_740_992.0,
    1e15,
    -1e15,
    1e22,
    1e300,
    -1e300,
    f64::MAX,
    f64::MIN,
    f64::MIN_POSITIVE,
    -f64::MIN_POSITIVE,
    5e-324,
    -5e-324,
    f64::INFINITY,
    f64::NEG_INFINITY,
    2_147_483_647.5,
    -2_147_483_648.5,
    4_294_967_295.5,
    1e-300,
];

pub(crate) const F32_VALUES: [f32; 36] = [
    0.0,
    -0.0,
    0.25,
    -0.25,
    0.5,
    -0.5,
    0.75,
    1.5,
    -1.5,
    2.5,
    -2.5,
    3.5,
    -3.5,
    1.0,
    -1.0,
    2.7,
    -2.7,
    0.499_999_97,
    8_388_607.5,
    -8_388_607.5,
    8_388_608.0,
    8_388_609.0,
    16_777_216.0,
    1e10,
    -1e10,
    1e30,
    f32::MAX,
    f32::MIN,
    f32::MIN_POSITIVE,
    -f32::MIN_POSITIVE,
    1e-45,
    -1e-45,
    f32::INFINITY,
    f32::NEG_INFINITY,
    2_147_483_648.0,
    1e-30,
];

pub(crate) const IOC: u32 = 1 << 0;
pub(crate) const IXC: u32 = 1 << 4;
pub(crate) const IDC: u32 = 1 << 7;
pub(crate) const FZ: u32 = 1 << 24;
pub(crate) const DN: u32 = 1 << 25;
