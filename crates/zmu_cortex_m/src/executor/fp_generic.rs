use std::cmp::Ordering;

use crate::{
    Processor,
    bus::Bus,
    core::{
        bits::Bits,
        fault::Fault,
        fpregister::{FPRounding, FPSCRRounding, Fpscr},
        register::{ExtensionRegOperations, SingleReg},
    },
    peripheral::{
        mpu::{AccType, Mpu},
        scb::{
            FPCCR_ASPEN, FPCCR_LSPACT, FPCCR_USER, FPSCR_STATUS_CONTROL_END,
            FPSCR_STATUS_CONTROL_START,
        },
    },
};

use super::fp_real::{Real, Rounded, round, round_magnitude, significand, whole_number_bits};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FPType {
    Nonzero,
    Zero,
    Infinity,
    QNaN,
    SNaN,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FPExc {
    InvalidOp,
    DivideByZero,
    Overflow,
    Underflow,
    Inexact,
    InputDenorm,
}

/// The two formats the FPU works in, binary32 as `u32` and binary64 as `u64`. Everything about
/// a format follows from the widths of its exponent and fraction.
pub trait FloatOps {
    /// The bits of a value of the format
    type Bits: PartialOrd + Into<u64> + Copy;

    const EXP_BITS: usize;
    const FRAC_BITS: usize;
    /// The exponent field of 1.0
    const BIAS: i32 = (1 << (Self::EXP_BITS - 1)) - 1;

    /// The low bits of `value`
    fn from_integer(value: u64) -> Self::Bits;

    /// Number of bits in the format
    fn n() -> usize {
        1 + Self::EXP_BITS + Self::FRAC_BITS
    }

    fn zero() -> Self::Bits {
        Self::from_integer(0)
    }

    fn set_bit(value: Self::Bits, bit: usize, set: bool) -> Self::Bits {
        let value: u64 = value.into();
        Self::from_integer(if set {
            value | 1 << bit
        } else {
            value & !(1 << bit)
        })
    }

    /// Returns the default NaN value
    fn fp_default_nan() -> Self::Bits {
        Self::from_integer(((1 << (Self::EXP_BITS + 1)) - 1) << (Self::FRAC_BITS - 1))
    }

    fn fp_infinity(sign: bool) -> Self::Bits {
        Self::from_integer(
            u64::from(sign) << (Self::n() - 1) | ((1 << Self::EXP_BITS) - 1) << Self::FRAC_BITS,
        )
    }

    fn fp_zero(sign: bool) -> Self::Bits {
        Self::from_integer(u64::from(sign) << (Self::n() - 1))
    }

    fn fp_abs(value: Self::Bits) -> Self::Bits {
        Self::from_integer(value.into() & !(1 << (Self::n() - 1)))
    }

    fn fp_max_normal(sign: bool) -> Self::Bits {
        Self::from_integer(Self::fp_infinity(sign).into() - 1)
    }
}

impl FloatOps for u32 {
    type Bits = u32;
    const EXP_BITS: usize = 8;
    const FRAC_BITS: usize = 23;

    fn from_integer(value: u64) -> u32 {
        value as u32
    }
}

impl FloatOps for u64 {
    type Bits = u64;
    const EXP_BITS: usize = 11;
    const FRAC_BITS: usize = 52;

    fn from_integer(value: u64) -> u64 {
        value
    }
}

pub trait FloatingPointChecks {
    fn check_vfp_enabled(&mut self) -> Result<(), Fault>;
    fn execute_fp_check(&mut self) -> Result<(), Fault>;
    fn fp_process_exception(&mut self, exc: FPExc);
}

impl Processor {
    // The FPSCR an operation reads its modes from: the register's, or the standard value that
    // has flush-to-zero, default NaN and round to nearest set.
    fn fpscr_value(&self, fpscr_controlled: bool) -> u32 {
        if fpscr_controlled {
            self.fpscr
        } else {
            standard_fpscr_value(self.fpscr)
        }
    }

    fn preserve_fp_state(&mut self) -> Result<(), Fault> {
        let acctype = if !self.fpccr.get_bit(FPCCR_USER) {
            AccType::Normal
        } else {
            AccType::UnPriv
        };

        for i in 0..16 {
            let memaddrdesc =
                self.validate_address(self.fpcar.wrapping_add(i * 4), acctype, true)?;
            let reg = SingleReg::from(i as u8);
            let value = self.get_sr(reg);
            self.write32(memaddrdesc, value)?;
        }
        let memaddrdesc = self.validate_address(self.fpcar.wrapping_add(0x40), acctype, true)?;
        self.write32(memaddrdesc, self.fpscr)?;
        self.fpccr.set_bit(FPCCR_LSPACT, false);
        Ok(())
    }
}

pub trait FloatingPointPublicOperations {
    fn fp_add<T: FloatOps>(
        &mut self,
        op1: T::Bits,
        op2: T::Bits,
        fpscr_controlled: bool,
    ) -> T::Bits;

    fn fp_sub<T: FloatOps>(
        &mut self,
        op1: T::Bits,
        op2: T::Bits,
        fpscr_controlled: bool,
    ) -> T::Bits;

    fn fp_mul_add<T: FloatOps>(
        &mut self,
        addend: T::Bits,
        op1: T::Bits,
        op2: T::Bits,
        fpscr_controlled: bool,
    ) -> T::Bits;

    fn fp_mul<T: FloatOps>(
        &mut self,
        op1: T::Bits,
        op2: T::Bits,
        fpscr_controlled: bool,
    ) -> T::Bits;

    fn fp_div<T: FloatOps>(
        &mut self,
        op1: T::Bits,
        op2: T::Bits,
        fpscr_controlled: bool,
    ) -> T::Bits;

    fn fp_compare<T: FloatOps>(
        &mut self,
        op1: T::Bits,
        op2: T::Bits,
        quiet_nan_exc: bool,
        fpscr_controlled: bool,
    ) -> (bool, bool, bool, bool);

    fn fp_abs<T: FloatOps>(&mut self, op: T::Bits) -> T::Bits;

    fn fp_sqrt<T: FloatOps>(&mut self, op: T::Bits, fpscr_controlled: bool) -> T::Bits;

    /// Rounds to an integral value in the same format. `rounding` is the mode the instruction
    /// names, or `None` for the one in the FPSCR.
    fn fp_round_int<T: FloatOps>(
        &mut self,
        op: T::Bits,
        rounding: Option<FPRounding>,
        exact: bool,
        fpscr_controlled: bool,
    ) -> T::Bits;

    /// VMAXNM and VMINNM: the larger or the smaller of the two, where a quiet NaN gives way to
    /// a number. Takes its flush and NaN modes from the FPSCR.
    fn fp_min_max_num<T: FloatOps>(&mut self, op1: T::Bits, op2: T::Bits, max: bool) -> T::Bits;

    /// Converts to a fixed-point integer of `size` bits with saturation. `rounding` is the mode the instruction
    /// names, or `None` for the one in the FPSCR.
    fn fp_to_fixed<N: FloatOps>(
        &mut self,
        op: N::Bits,
        size: usize,
        fraction_bits: usize,
        unsigned: bool,
        rounding: Option<FPRounding>,
        fpscr_controlled: bool,
    ) -> u64;

    fn fixed_to_fp<N: FloatOps, M: FloatOps>(
        &mut self,
        op: M::Bits,
        fraction_bits: usize,
        unsigned: bool,
        round_to_nearest: bool,
        fpscr_controlled: bool,
    ) -> N::Bits;

    /// Converts between the two formats, `S` to `D`, with the modes of the FPSCR. A NaN keeps
    /// the top bits of its fraction and its sign.
    fn fp_convert<S: FloatOps, D: FloatOps>(&mut self, op: S::Bits) -> D::Bits;
}

trait FloatingPointHiddenOperations {
    fn fp_process_nan<T: FloatOps>(
        &mut self,
        type1: FPType,
        op: T::Bits,
        fpscr_val: u32,
    ) -> T::Bits;

    fn fp_process_nans<T: FloatOps>(
        &mut self,
        type1: FPType,
        type2: FPType,
        op1: T::Bits,
        op2: T::Bits,
        fpscr_val: u32,
    ) -> (bool, T::Bits);

    fn fp_process_nans3<T: FloatOps>(
        &mut self,
        typea: FPType,
        type1: FPType,
        type2: FPType,
        addend: T::Bits,
        op1: T::Bits,
        op2: T::Bits,
        fpscr_val: u32,
    ) -> (bool, T::Bits);

    fn fp_unpack<T: FloatOps>(&mut self, fpval: T::Bits, fpscr_val: u32) -> (FPType, bool, Real);

    fn fp_round<T: FloatOps>(&mut self, value: Real, fpscr_val: u32) -> T::Bits;

    /// The sum of the operands, or their difference when `subtract` is set. A difference is a
    /// sum with the sign of the second operand turned, but a NaN comes through as it was.
    fn fp_add_sub<T: FloatOps>(
        &mut self,
        op1: T::Bits,
        op2: T::Bits,
        subtract: bool,
        fpscr_val: u32,
    ) -> T::Bits;
}

// A number that runs in the order of the values it stands for. The magnitude of the bits runs in
// the order of the magnitudes, and a zero, which a flushed subnormal is too, is 0 whichever its
// sign. A NaN has no place in the order.
fn order_key<T: FloatOps>(fptype: FPType, sign: bool, op: T::Bits) -> i128 {
    let magnitude = if fptype == FPType::Zero {
        0
    } else {
        i128::from(T::fp_abs(op).into())
    };
    if sign { -magnitude } else { magnitude }
}

fn standard_fpscr_value(fpscr: u32) -> u32 {
    (u32::from(fpscr.get_bit(26)) << 26) | 0b11_0000_0000_0000_0000_0000_0000
}

/// saturate i to n bits, return the result and a boolean indicating if saturation occurred (up to 64 bits)
fn satq(i: i128, n: usize, unsigned: bool) -> (u64, bool) {
    debug_assert!(n <= 64);

    if unsigned {
        let max_unsigned = 1u128
            .checked_shl(n as u32)
            .unwrap_or(u128::MAX)
            .saturating_sub(1);
        let max_unsigned_i128 = i128::try_from(max_unsigned).unwrap_or(i128::MAX);

        let clamped = i.clamp(0, max_unsigned_i128);
        let saturated = clamped != i;
        let result = clamped as u128;

        (result.get_bits(0..n) as u64, saturated)
    } else {
        let signed_bits = n.saturating_sub(1) as u32;
        let signed_scale = 1i128.checked_shl(signed_bits).unwrap_or(i128::MAX);
        let max_signed = signed_scale.saturating_sub(1);
        let min_signed = signed_scale.saturating_neg();

        let result = i.clamp(min_signed, max_signed);
        let saturated = result != i;

        ((result as u64).get_bits(0..n), saturated)
    }
}

impl FloatingPointHiddenOperations for Processor {
    fn fp_process_nan<T: FloatOps>(
        &mut self,
        type1: FPType,
        op: T::Bits,
        fpscr_val: u32,
    ) -> T::Bits {
        let topfrac = if T::n() == 32 { 22 } else { 51 };
        let mut result = op;
        if type1 == FPType::SNaN {
            result = T::set_bit(result, topfrac, true);
            self.fp_process_exception(FPExc::InvalidOp);
        }
        if fpscr_val.get_dn() {
            result = T::fp_default_nan();
        }
        result
    }

    fn fp_process_nans<T: FloatOps>(
        &mut self,
        type1: FPType,
        type2: FPType,
        op1: T::Bits,
        op2: T::Bits,
        fpscr_val: u32,
    ) -> (bool, T::Bits) {
        if type1 == FPType::SNaN {
            let result = self.fp_process_nan::<T>(type1, op1, fpscr_val);
            (true, result)
        } else if type2 == FPType::SNaN {
            let result = self.fp_process_nan::<T>(type2, op2, fpscr_val);
            (true, result)
        } else if type1 == FPType::QNaN {
            let result = self.fp_process_nan::<T>(type1, op1, fpscr_val);
            (true, result)
        } else if type2 == FPType::QNaN {
            let result = self.fp_process_nan::<T>(type2, op2, fpscr_val);
            (true, result)
        } else {
            (false, T::zero())
        }
    }

    fn fp_process_nans3<T: FloatOps>(
        &mut self,
        typea: FPType,
        type1: FPType,
        type2: FPType,
        addend: T::Bits,
        op1: T::Bits,
        op2: T::Bits,
        fpscr_val: u32,
    ) -> (bool, T::Bits) {
        if typea == FPType::SNaN {
            (true, self.fp_process_nan::<T>(typea, addend, fpscr_val))
        } else if type1 == FPType::SNaN {
            (true, self.fp_process_nan::<T>(type1, op1, fpscr_val))
        } else if type2 == FPType::SNaN {
            (true, self.fp_process_nan::<T>(type2, op2, fpscr_val))
        } else if typea == FPType::QNaN {
            (true, self.fp_process_nan::<T>(typea, addend, fpscr_val))
        } else if type1 == FPType::QNaN {
            (true, self.fp_process_nan::<T>(type1, op1, fpscr_val))
        } else if type2 == FPType::QNaN {
            (true, self.fp_process_nan::<T>(type2, op2, fpscr_val))
        } else {
            (false, T::zero())
        }
    }

    fn fp_unpack<T: FloatOps>(&mut self, fpval: T::Bits, fpscr_val: u32) -> (FPType, bool, Real) {
        let bits: u64 = fpval.into();
        let sign = bits.get_bit(T::n() - 1);
        let exp = bits.get_bits(T::FRAC_BITS..T::n() - 1);
        let frac = bits.get_bits(0..T::FRAC_BITS);
        let (fptype, finite) = if exp == (1 << T::EXP_BITS) - 1 {
            let fptype = match (frac == 0, frac.get_bit(T::FRAC_BITS - 1)) {
                (true, _) => FPType::Infinity,
                (false, true) => FPType::QNaN,
                (false, false) => FPType::SNaN,
            };
            (fptype, false)
        } else if exp == 0 && frac != 0 && fpscr_val.get_fz() {
            // A subnormal input is flushed to zero.
            self.fp_process_exception(FPExc::InputDenorm);
            (FPType::Zero, false)
        } else if bits.get_bits(0..T::n() - 1) == 0 {
            (FPType::Zero, false)
        } else {
            (FPType::Nonzero, true)
        };
        let value = if finite {
            let (sig, exp) = significand::<T>(fpval);
            Real { sign, sig, exp }
        } else {
            Real { sign, ..Real::ZERO }
        };
        (fptype, sign, value)
    }

    fn fp_round<T: FloatOps>(&mut self, value: Real, fpscr_val: u32) -> T::Bits {
        let packed = round::<T>(
            value,
            fpscr_val.get_rounding_mode().into(),
            fpscr_val.get_fz(),
        );
        if packed.underflow {
            self.fp_process_exception(FPExc::Underflow);
        }
        if packed.overflow {
            self.fp_process_exception(FPExc::Overflow);
        }
        if packed.inexact {
            self.fp_process_exception(FPExc::Inexact);
        }
        packed.bits
    }

    fn fp_add_sub<T: FloatOps>(
        &mut self,
        op1: T::Bits,
        op2: T::Bits,
        subtract: bool,
        fpscr_val: u32,
    ) -> T::Bits {
        let (type1, sign1, value1) = self.fp_unpack::<T>(op1, fpscr_val);
        let (type2, sign2, value2) = self.fp_unpack::<T>(op2, fpscr_val);
        let (done, result) = self.fp_process_nans::<T>(type1, type2, op1, op2, fpscr_val);
        if done {
            return result;
        }
        let (sign2, value2) = if subtract {
            (!sign2, value2.negate())
        } else {
            (sign2, value2)
        };

        let inf1 = type1 == FPType::Infinity;
        let inf2 = type2 == FPType::Infinity;
        let zero1 = type1 == FPType::Zero;
        let zero2 = type2 == FPType::Zero;

        if inf1 && inf2 && sign1 != sign2 {
            self.fp_process_exception(FPExc::InvalidOp);
            T::fp_default_nan()
        } else if (inf1 && !sign1) || (inf2 && !sign2) {
            T::fp_infinity(false)
        } else if (inf1 && sign1) || (inf2 && sign2) {
            T::fp_infinity(true)
        } else if zero1 && zero2 && sign1 == sign2 {
            T::fp_zero(sign1)
        } else {
            let sum = value1.add(value2);
            if sum.is_zero() {
                // The sign of an exact zero result depends on the rounding mode.
                T::fp_zero(
                    fpscr_val.get_rounding_mode() == FPSCRRounding::RoundTowardsMinusInfinity,
                )
            } else {
                self.fp_round::<T>(sum, fpscr_val)
            }
        }
    }
}

impl FloatingPointPublicOperations for Processor {
    fn fp_add<T: FloatOps>(
        &mut self,
        op1: T::Bits,
        op2: T::Bits,
        fpscr_controlled: bool,
    ) -> T::Bits {
        let fpscr_val = self.fpscr_value(fpscr_controlled);
        self.fp_add_sub::<T>(op1, op2, false, fpscr_val)
    }

    fn fp_sub<T: FloatOps>(
        &mut self,
        op1: T::Bits,
        op2: T::Bits,
        fpscr_controlled: bool,
    ) -> T::Bits {
        let fpscr_val = self.fpscr_value(fpscr_controlled);
        self.fp_add_sub::<T>(op1, op2, true, fpscr_val)
    }

    fn fp_mul_add<T: FloatOps>(
        &mut self,
        addend: T::Bits,
        op1: T::Bits,
        op2: T::Bits,
        fpscr_controlled: bool,
    ) -> T::Bits {
        let fpscr_val = self.fpscr_value(fpscr_controlled);

        let (typea, signa, valuea) = self.fp_unpack::<T>(addend, fpscr_val);
        let (type1, sign1, value1) = self.fp_unpack::<T>(op1, fpscr_val);
        let (type2, sign2, value2) = self.fp_unpack::<T>(op2, fpscr_val);

        let inf1 = type1 == FPType::Infinity;
        let zero1 = type1 == FPType::Zero;
        let inf2 = type2 == FPType::Infinity;
        let zero2 = type2 == FPType::Zero;

        let (done, mut result) =
            self.fp_process_nans3::<T>(typea, type1, type2, addend, op1, op2, fpscr_val);

        if typea == FPType::QNaN && ((inf1 && zero2) || (zero1 && inf2)) {
            result = T::fp_default_nan();
            self.fp_process_exception(FPExc::InvalidOp);
            return result;
        }

        if done {
            return result;
        }

        let infa = typea == FPType::Infinity;
        let zeroa = typea == FPType::Zero;

        let signp = sign1 != sign2;
        let infp = inf1 || inf2;
        let zerop = zero1 || zero2;

        if (inf1 && zero2) || (zero1 && inf2) || (infa && infp && signa != signp) {
            self.fp_process_exception(FPExc::InvalidOp);
            T::fp_default_nan()
        } else if (infa && !signa) || (infp && !signp) {
            T::fp_infinity(false)
        } else if (infa && signa) || (infp && signp) {
            T::fp_infinity(true)
        } else if zeroa && zerop && signa == signp {
            T::fp_zero(signa)
        } else {
            let result_value = valuea.add(value1.mul(value2));
            if result_value.is_zero() {
                let result_sign =
                    fpscr_val.get_rounding_mode() == FPSCRRounding::RoundTowardsMinusInfinity;
                T::fp_zero(result_sign)
            } else {
                self.fp_round::<T>(result_value, fpscr_val)
            }
        }
    }

    fn fp_mul<T: FloatOps>(
        &mut self,
        op1: T::Bits,
        op2: T::Bits,
        fpscr_controlled: bool,
    ) -> T::Bits {
        let fpscr_val = self.fpscr_value(fpscr_controlled);

        let (type1, sign1, value1) = self.fp_unpack::<T>(op1, fpscr_val);
        let (type2, sign2, value2) = self.fp_unpack::<T>(op2, fpscr_val);
        let (done, result) = self.fp_process_nans::<T>(type1, type2, op1, op2, fpscr_val);
        if done {
            return result;
        }

        let inf1 = type1 == FPType::Infinity;
        let zero1 = type1 == FPType::Zero;
        let inf2 = type2 == FPType::Infinity;
        let zero2 = type2 == FPType::Zero;
        let sign = sign1 != sign2;

        if (inf1 && zero2) || (zero1 && inf2) {
            self.fp_process_exception(FPExc::InvalidOp);
            T::fp_default_nan()
        } else if inf1 || inf2 {
            T::fp_infinity(sign)
        } else if zero1 || zero2 {
            T::fp_zero(sign)
        } else {
            self.fp_round::<T>(value1.mul(value2), fpscr_val)
        }
    }

    fn fp_div<T: FloatOps>(
        &mut self,
        op1: T::Bits,
        op2: T::Bits,
        fpscr_controlled: bool,
    ) -> T::Bits {
        let fpscr_val = self.fpscr_value(fpscr_controlled);

        let (type1, sign1, value1) = self.fp_unpack::<T>(op1, fpscr_val);
        let (type2, sign2, value2) = self.fp_unpack::<T>(op2, fpscr_val);
        let (done, result) = self.fp_process_nans::<T>(type1, type2, op1, op2, fpscr_val);
        if done {
            return result;
        }

        let inf1 = type1 == FPType::Infinity;
        let zero1 = type1 == FPType::Zero;
        let inf2 = type2 == FPType::Infinity;
        let zero2 = type2 == FPType::Zero;
        let sign = sign1 != sign2;

        if (zero1 && zero2) || (inf1 && inf2) {
            self.fp_process_exception(FPExc::InvalidOp);
            T::fp_default_nan()
        } else if inf1 {
            T::fp_infinity(sign)
        } else if inf2 {
            T::fp_zero(sign)
        } else if zero2 {
            self.fp_process_exception(FPExc::DivideByZero);
            T::fp_infinity(sign)
        } else if zero1 {
            T::fp_zero(sign)
        } else {
            self.fp_round::<T>(value1.div(value2), fpscr_val)
        }
    }

    fn fp_compare<T: FloatOps>(
        &mut self,
        op1: T::Bits,
        op2: T::Bits,
        quiet_nan_exc: bool,
        fpscr_controlled: bool,
    ) -> (bool, bool, bool, bool) {
        let fpscr_val = self.fpscr_value(fpscr_controlled);

        let (type1, sign1, _) = self.fp_unpack::<T>(op1, fpscr_val);
        let (type2, sign2, _) = self.fp_unpack::<T>(op2, fpscr_val);

        if type1 == FPType::SNaN
            || type1 == FPType::QNaN
            || type2 == FPType::SNaN
            || type2 == FPType::QNaN
        {
            let result = (false, false, true, true);
            if type1 == FPType::SNaN || type2 == FPType::SNaN || quiet_nan_exc {
                self.fp_process_exception(FPExc::InvalidOp);
            }
            result
        } else {
            match order_key::<T>(type1, sign1, op1).cmp(&order_key::<T>(type2, sign2, op2)) {
                Ordering::Equal => (false, true, true, false),
                Ordering::Less => (true, false, false, false),
                Ordering::Greater => (false, false, true, false),
            }
        }
    }

    fn fp_abs<T: FloatOps>(&mut self, op: T::Bits) -> T::Bits {
        T::fp_abs(op)
    }

    fn fp_sqrt<T: FloatOps>(&mut self, op: T::Bits, fpscr_controlled: bool) -> T::Bits {
        let fpscr_val = self.fpscr_value(fpscr_controlled);

        let (fptype, sign, value) = self.fp_unpack::<T>(op, fpscr_val);

        match fptype {
            FPType::SNaN | FPType::QNaN => self.fp_process_nan::<T>(fptype, op, fpscr_val),
            FPType::Infinity => {
                if sign {
                    self.fp_process_exception(FPExc::InvalidOp);
                    T::fp_default_nan()
                } else {
                    T::fp_infinity(false)
                }
            }
            FPType::Zero => T::fp_zero(sign),
            FPType::Nonzero => {
                if sign {
                    self.fp_process_exception(FPExc::InvalidOp);
                    return T::fp_default_nan();
                }

                self.fp_round::<T>(value.sqrt(), fpscr_val)
            }
        }
    }

    fn fp_round_int<T: FloatOps>(
        &mut self,
        op: T::Bits,
        rounding: Option<FPRounding>,
        exact: bool,
        fpscr_controlled: bool,
    ) -> T::Bits {
        let fpscr_val = self.fpscr_value(fpscr_controlled);

        let rounding = rounding.unwrap_or_else(|| fpscr_val.get_rounding_mode().into());

        let (fptype, sign, _) = self.fp_unpack::<T>(op, fpscr_val);

        if fptype == FPType::SNaN || fptype == FPType::QNaN {
            return self.fp_process_nan::<T>(fptype, op, fpscr_val);
        }

        if fptype == FPType::Infinity {
            return T::fp_infinity(sign);
        }

        if fptype == FPType::Zero {
            return T::fp_zero(sign);
        }

        let (significand, exponent) = significand::<T>(op);
        let rounded = round_magnitude(significand, exponent, sign, rounding);
        if !rounded.inexact {
            // A whole number is its own result.
            return op;
        }

        if exact {
            self.fp_process_exception(FPExc::Inexact);
        }

        // Rounding a value that has a fraction gives a whole number that fits the format, or
        // zero, which keeps the sign.
        if rounded.magnitude == 0 {
            T::fp_zero(sign)
        } else {
            whole_number_bits::<T>(sign, rounded.magnitude as u64)
        }
    }

    fn fp_min_max_num<T: FloatOps>(&mut self, op1: T::Bits, op2: T::Bits, max: bool) -> T::Bits {
        let fpscr_val = self.fpscr;
        let (type1, sign1, _) = self.fp_unpack::<T>(op1, fpscr_val);
        let (type2, sign2, _) = self.fp_unpack::<T>(op2, fpscr_val);

        // A quiet NaN gives way to anything that is not one, so the other operand is the only
        // one that counts. It is still a signalling NaN that wins over it.
        let drop1 = type1 == FPType::QNaN && type2 != FPType::QNaN;
        let drop2 = type2 == FPType::QNaN && type1 != FPType::QNaN;
        let counted = |dropped: bool, fptype: FPType| {
            if dropped { FPType::Infinity } else { fptype }
        };
        let (done, nan) = self.fp_process_nans::<T>(
            counted(drop1, type1),
            counted(drop2, type2),
            op1,
            op2,
            fpscr_val,
        );
        if done {
            return nan;
        }

        let (key1, key2) = (
            order_key::<T>(type1, sign1, op1),
            order_key::<T>(type2, sign2, op2),
        );
        let first = if drop1 {
            false
        } else if drop2 {
            true
        } else if max {
            key1 > key2
        } else {
            key1 < key2
        };
        let (fptype, sign, op) = if first {
            (type1, sign1, op1)
        } else {
            (type2, sign2, op2)
        };
        match fptype {
            FPType::Infinity => T::fp_infinity(sign),
            // Of two zeros, +0 is the larger.
            FPType::Zero => T::fp_zero(if drop1 {
                sign2
            } else if drop2 {
                sign1
            } else if max {
                sign1 && sign2
            } else {
                sign1 || sign2
            }),
            _ => op,
        }
    }

    fn fixed_to_fp<N: FloatOps, M: FloatOps>(
        &mut self,
        op: M::Bits,
        fraction_bits: usize,
        unsigned: bool,
        round_to_nearest: bool,
        fpscr_controlled: bool,
    ) -> N::Bits {
        let mut fpscr_val = self.fpscr_value(fpscr_controlled);
        if round_to_nearest {
            fpscr_val.set_rounding_mode(FPSCRRounding::RoundToNearest);
        }

        // The operand is the value of the integer, with the sign extended from the width of the
        // source when it is signed, times 2^-fraction_bits.
        let raw: u64 = op.into();
        let shift = 64 - M::n();
        let (sign, magnitude) = if unsigned {
            (false, raw)
        } else {
            let value = ((raw << shift) as i64) >> shift;
            (value < 0, value.unsigned_abs())
        };

        if magnitude == 0 {
            N::zero()
        } else {
            let real = Real::from_integer(sign, magnitude, -(fraction_bits as i32));
            self.fp_round::<N>(real, fpscr_val)
        }
    }

    fn fp_convert<S: FloatOps, D: FloatOps>(&mut self, op: S::Bits) -> D::Bits {
        let fpscr_val = self.fpscr;
        let (fptype, sign, value) = self.fp_unpack::<S>(op, fpscr_val);
        match fptype {
            FPType::SNaN | FPType::QNaN => {
                let frac = op.into() & ((1 << S::FRAC_BITS) - 1);
                let frac = if D::FRAC_BITS >= S::FRAC_BITS {
                    frac << (D::FRAC_BITS - S::FRAC_BITS)
                } else {
                    frac >> (S::FRAC_BITS - D::FRAC_BITS)
                };
                let nan = D::from_integer(D::fp_infinity(sign).into() | frac);
                self.fp_process_nan::<D>(fptype, nan, fpscr_val)
            }
            FPType::Infinity => D::fp_infinity(sign),
            FPType::Zero => D::fp_zero(sign),
            FPType::Nonzero => self.fp_round::<D>(value, fpscr_val),
        }
    }

    fn fp_to_fixed<N: FloatOps>(
        &mut self,
        op: N::Bits,
        size: usize,
        fraction_bits: usize,
        unsigned: bool,
        rounding: Option<FPRounding>,
        fpscr_controlled: bool,
    ) -> u64 {
        let fpscr_val = self.fpscr_value(fpscr_controlled);

        let rounding = rounding.unwrap_or_else(|| fpscr_val.get_rounding_mode().into());
        let (type_t, sign, _) = self.fp_unpack::<N>(op, fpscr_val);

        let rounded = match type_t {
            FPType::SNaN | FPType::QNaN => {
                self.fp_process_exception(FPExc::InvalidOp);
                Rounded {
                    magnitude: 0,
                    inexact: false,
                }
            }
            FPType::Zero => Rounded {
                magnitude: 0,
                inexact: false,
            },
            // An infinity is too large for any integer.
            FPType::Infinity => Rounded {
                magnitude: u128::MAX,
                inexact: false,
            },
            FPType::Nonzero => {
                let (significand, exponent) = significand::<N>(op);
                round_magnitude(significand, exponent + fraction_bits as i32, sign, rounding)
            }
        };
        let magnitude = i128::try_from(rounded.magnitude).unwrap_or(i128::MAX);
        let (result, overflow) = satq(if sign { -magnitude } else { magnitude }, size, unsigned);

        if overflow {
            self.fp_process_exception(FPExc::InvalidOp);
        } else if rounded.inexact {
            self.fp_process_exception(FPExc::Inexact);
        }

        result
    }
}

impl FloatingPointChecks for Processor {
    fn check_vfp_enabled(&mut self) -> Result<(), Fault> {
        let cp10 = self.cpacr.get_bits(20..22);
        let cp11 = self.cpacr.get_bits(22..24);

        if cp10 != cp11 {
            return Err(Fault::Forced);
        }

        match cp10 {
            0b00 => {
                self.cfsr.set_bit(19, true);
                Err(Fault::Nocp)
            }
            0b01 => {
                if !self.current_mode_is_privileged() {
                    self.cfsr.set_bit(19, true);
                    Err(Fault::Nocp)
                } else {
                    Ok(())
                }
            }
            0b10 => {
                self.cfsr.set_bit(19, true);
                Err(Fault::Nocp)
            }
            // 0b11, the last value of the two bit field: full access
            _ => Ok(()),
        }
    }
    fn execute_fp_check(&mut self) -> Result<(), Fault> {
        self.check_vfp_enabled()?;

        if self.fpccr.get_bit(FPCCR_LSPACT) {
            self.preserve_fp_state()?;
        }

        if self.fpccr.get_bit(FPCCR_ASPEN) && !self.control.fpca {
            let fpdscr_bits = self
                .fpdscr
                .get_bits(FPSCR_STATUS_CONTROL_START..FPSCR_STATUS_CONTROL_END);
            self.fpscr.set_bits(
                FPSCR_STATUS_CONTROL_START..FPSCR_STATUS_CONTROL_END,
                fpdscr_bits,
            );
            self.control.fpca = true;
        }
        Ok(())
    }

    fn fp_process_exception(&mut self, exc: FPExc) {
        // The trap enables are reserved on this core, so the cumulative flag is all there is.
        let cumul = match exc {
            FPExc::InvalidOp => 0,
            FPExc::DivideByZero => 1,
            FPExc::Overflow => 2,
            FPExc::Underflow => 3,
            FPExc::Inexact => 4,
            FPExc::InputDenorm => 7,
        };
        self.fpscr.set_bit(cumul, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Processor;
    use crate::bus::Bus;
    use crate::core::bits::Bits;
    use crate::core::fault::Fault;
    use crate::core::register::{ExtensionRegOperations, SingleReg};
    use crate::executor::fp_test_support::*;

    #[test]
    fn test_fp_compare_f32() {
        let mut processor = Processor::new();
        assert_eq!(
            processor.fp_compare::<u32>(0x3F80_0000, 0x3F80_0000, false, false),
            (false, true, true, false)
        ); // 1.0 == 1.0
        assert_eq!(
            processor.fp_compare::<u32>(0x3F80_0000, 0x4000_0000, false, false),
            (true, false, false, false)
        ); // 1.0 < 2.0
        assert_eq!(
            processor.fp_compare::<u32>(0x4000_0000, 0x3F80_0000, false, false),
            (false, false, true, false)
        ); // 2.0 > 1.0
    }

    #[test]
    fn test_fp_compare_f64() {
        let mut processor = Processor::new();

        // 1.0 == 1.0
        assert_eq!(
            processor.fp_compare::<u64>(0x3FF0_0000_0000_0000, 0x3FF0_0000_0000_0000, false, false),
            (false, true, true, false)
        );

        // 1.0 < 2.0
        assert_eq!(
            processor.fp_compare::<u64>(0x3FF0_0000_0000_0000, 0x4000_0000_0000_0000, false, false),
            (true, false, false, false)
        );
        // 2.0 > 1.0
        assert_eq!(
            processor.fp_compare::<u64>(0x4000_0000_0000_0000, 0x3FF0_0000_0000_0000, false, false),
            (false, false, true, false)
        );
    }

    #[test]
    fn test_fpabs_32() {
        let mut processor = Processor::new();

        // -0.0 -> 0.0
        assert_eq!(processor.fp_abs::<u32>(0x8000_0000), 0x0000_0000);

        // -1.0 -> 1.0
        assert_eq!(processor.fp_abs::<u32>(0xFFFF_FFFF), 0x7FFF_FFFF);

        // 1.0 -> 1.0
        assert_eq!(processor.fp_abs::<u32>(0x7FFF_FFFF), 0x7FFF_FFFF);

        // most negative value:
        assert_eq!(
            processor.fp_abs::<u32>((-f32::MAX).to_bits()),
            f32::MAX.to_bits()
        );
    }
    #[test]
    fn test_fpabs_64() {
        let mut processor = Processor::new();
        // -0.0 -> 0.0
        assert_eq!(
            processor.fp_abs::<u64>(0x8000_0000_0000_0000),
            0x0000_0000_0000_0000
        );
        // -1.0 -> 1.0
        assert_eq!(
            processor.fp_abs::<u64>(0xFFFF_FFFF_FFFF_FFFF),
            0x7FFF_FFFF_FFFF_FFFF
        );
        // 1.0 -> 1.0
        assert_eq!(
            processor.fp_abs::<u64>(0x7FFF_FFFF_FFFF_FFFF),
            0x7FFF_FFFF_FFFF_FFFF
        );

        // min value
        assert_eq!(
            processor.fp_abs::<u64>((-f64::MAX).to_bits()),
            f64::MAX.to_bits()
        );
    }

    fn real(sign: bool, sig: u128, exp: i32) -> Real {
        Real { sign, sig, exp }
    }

    #[test]
    fn test_fp_unpack_f32() {
        let mut processor = Processor::new();
        let cases = [
            (
                0x3F80_0000,
                FPType::Nonzero,
                false,
                real(false, 1 << 23, -23),
            ),
            (0xBF80_0000, FPType::Nonzero, true, real(true, 1 << 23, -23)),
            (
                0x4000_0000,
                FPType::Nonzero,
                false,
                real(false, 1 << 23, -22),
            ),
            (
                0x7F7F_FFFF,
                FPType::Nonzero,
                false,
                real(false, 0xFF_FFFF, 104),
            ),
            // The smallest normal, and the smallest and largest subnormal.
            (
                0x0080_0000,
                FPType::Nonzero,
                false,
                real(false, 1 << 23, -149),
            ),
            (0x0000_0001, FPType::Nonzero, false, real(false, 1, -149)),
            (
                0x807F_FFFF,
                FPType::Nonzero,
                true,
                real(true, 0x7F_FFFF, -149),
            ),
            (0x0000_0000, FPType::Zero, false, real(false, 0, 0)),
            (0x8000_0000, FPType::Zero, true, real(true, 0, 0)),
            (0x7F80_0000, FPType::Infinity, false, real(false, 0, 0)),
            (0xFF80_0000, FPType::Infinity, true, real(true, 0, 0)),
            (0x7FC0_0000, FPType::QNaN, false, real(false, 0, 0)),
            (0x7F80_0001, FPType::SNaN, false, real(false, 0, 0)),
            (0xFFBF_FFFF, FPType::SNaN, true, real(true, 0, 0)),
        ];
        for (bits, fptype, sign, value) in cases {
            assert_eq!(
                processor.fp_unpack::<u32>(bits, 0),
                (fptype, sign, value),
                "{bits:#x}"
            );
        }
        assert_eq!(processor.fpscr, 0);
    }

    #[test]
    fn test_fp_unpack_f64() {
        let mut processor = Processor::new();
        let cases = [
            (
                1.0f64.to_bits(),
                FPType::Nonzero,
                false,
                real(false, 1 << 52, -52),
            ),
            (
                (-1.0f64).to_bits(),
                FPType::Nonzero,
                true,
                real(true, 1 << 52, -52),
            ),
            (
                2.0f64.to_bits(),
                FPType::Nonzero,
                false,
                real(false, 1 << 52, -51),
            ),
            (
                f64::MAX.to_bits(),
                FPType::Nonzero,
                false,
                real(false, (1 << 53) - 1, 971),
            ),
            (
                f64::MIN_POSITIVE.to_bits(),
                FPType::Nonzero,
                false,
                real(false, 1 << 52, -1074),
            ),
            (1, FPType::Nonzero, false, real(false, 1, -1074)),
            (0, FPType::Zero, false, real(false, 0, 0)),
            ((-0.0f64).to_bits(), FPType::Zero, true, real(true, 0, 0)),
            (
                f64::INFINITY.to_bits(),
                FPType::Infinity,
                false,
                real(false, 0, 0),
            ),
            (
                f64::NEG_INFINITY.to_bits(),
                FPType::Infinity,
                true,
                real(true, 0, 0),
            ),
            (
                0x7FF8_0000_0000_0000,
                FPType::QNaN,
                false,
                real(false, 0, 0),
            ),
            (
                0x7FF0_0000_0000_0001,
                FPType::SNaN,
                false,
                real(false, 0, 0),
            ),
        ];
        for (bits, fptype, sign, value) in cases {
            assert_eq!(
                processor.fp_unpack::<u64>(bits, 0),
                (fptype, sign, value),
                "{bits:#x}"
            );
        }
        assert_eq!(processor.fpscr, 0);
    }

    #[test]
    fn fp_unpack_flushes_a_subnormal_input_and_signals_it_when_fz_is_set() {
        let mut processor = Processor::new();
        assert_eq!(
            processor.fp_unpack::<u64>(0x8000_0000_0000_0001, FZ),
            (FPType::Zero, true, real(true, 0, 0))
        );
        assert_eq!(processor.fpscr, IDC);

        // A zero and a normal number are left alone.
        let mut processor = Processor::new();
        processor.fp_unpack::<u32>(0x8000_0000, FZ);
        processor.fp_unpack::<u32>(0x0080_0000, FZ);
        assert_eq!(processor.fpscr, 0);
    }

    #[test]
    fn test_fp_round_f32() {
        let mut processor = Processor::new();
        // 1.0 is exact.
        assert_eq!(processor.fp_round::<u32>(real(false, 1, 0), 0), 0x3F80_0000);
        // A third rounds up to the nearest float, and that is inexact.
        let third = real(false, 1, 0).div(real(false, 3, 0));
        assert_eq!(processor.fp_round::<u32>(third, 0), 0x3eaa_aaab);
        assert_eq!(processor.fpscr, IXC);
    }

    #[test]
    fn test_fp_round_f64() {
        let mut processor = Processor::new();
        assert_eq!(
            processor.fp_round::<u64>(real(false, 1, 0), 0),
            0x3FF0_0000_0000_0000
        );
        assert_eq!(processor.fpscr, 0);
    }

    #[test]
    fn test_fp_add_f32() {
        let mut processor = Processor::new();

        // 1.0 + 1.0 = 2.0
        assert_eq!(
            processor.fp_add::<u32>(0x3F80_0000, 0x3F80_0000, true),
            0x4000_0000
        );

        // 1.0 + 2.0 = 3.0
        assert_eq!(
            processor.fp_add::<u32>(0x3F80_0000, 0x4000_0000, true),
            0x4040_0000
        );

        // -1.0 + 2.0 = 1.0
        assert_eq!(
            processor.fp_add::<u32>(0xBF80_0000, 0x4000_0000, true),
            0x3F80_0000
        );

        // 0.9038018 + epsilon should round back to the original sigma value.
        assert_eq!(
            processor.fp_add::<u32>(0x3F67_5F8E, 0x3194_BDC1, true),
            0x3F67_5F8E
        );
    }

    #[test]
    fn test_fp_sub_f32() {
        let mut processor = Processor::new();

        // 1.0 - 1.0 = 0.0
        assert_eq!(
            processor.fp_sub::<u32>(0x3F80_0000, 0x3F80_0000, true),
            0x0000_0000
        );

        // 2.0 - 1.0 = 1.0
        assert_eq!(
            processor.fp_sub::<u32>(0x4000_0000, 0x3F80_0000, true),
            0x3F80_0000
        );

        // 1.0 - 2.0 = -1.0
        assert_eq!(
            processor.fp_sub::<u32>(0x3F80_0000, 0x4000_0000, true),
            0xBF80_0000
        );

        // 1.5 - 1.4539529 should preserve the small positive difference.
        assert_eq!(
            processor.fp_sub::<u32>(0x3FC0_0000, 0x3FBA_1B21, true),
            0x3D3C_9BE0
        );
    }

    #[test]
    fn test_fp_mul_add_f32_basic() {
        let mut processor = Processor::new();

        assert_eq!(
            processor.fp_mul_add::<u32>(1.0f32.to_bits(), 2.0f32.to_bits(), 3.0f32.to_bits(), true),
            7.0f32.to_bits()
        );
    }

    #[test]
    fn test_fp_mul_add_zero_sign_uses_rounding_mode() {
        let mut processor = Processor::new();
        processor
            .fpscr
            .set_rounding_mode(FPSCRRounding::RoundTowardsMinusInfinity);

        let result = processor.fp_mul_add::<u32>(
            (-0.0f32).to_bits(),
            0.0f32.to_bits(),
            5.0f32.to_bits(),
            true,
        );

        assert_eq!(result, (-0.0f32).to_bits());
    }

    #[test]
    fn test_fp_mul_add_inf_times_zero_is_invalid() {
        let mut processor = Processor::new();
        processor.fpscr = 0;

        let result = processor.fp_mul_add::<u32>(
            1.0f32.to_bits(),
            f32::INFINITY.to_bits(),
            0.0f32.to_bits(),
            true,
        );

        assert_eq!(result, <u32 as FloatOps>::fp_default_nan());
        assert!(processor.fpscr.get_bit(0));
    }

    #[test]
    fn test_fp_mul_f32_basic() {
        let mut processor = Processor::new();

        assert_eq!(
            processor.fp_mul::<u32>(2.0f32.to_bits(), 3.0f32.to_bits(), true),
            6.0f32.to_bits()
        );
    }

    #[test]
    fn test_fp_div_f32_basic() {
        let mut processor = Processor::new();

        assert_eq!(
            processor.fp_div::<u32>(6.0f32.to_bits(), 2.0f32.to_bits(), true),
            3.0f32.to_bits()
        );
    }

    #[test]
    fn test_fp_div_f32_by_zero_sets_exception() {
        let mut processor = Processor::new();
        processor.fpscr = 0;

        let result = processor.fp_div::<u32>(1.0f32.to_bits(), 0.0f32.to_bits(), true);

        assert_eq!(result, f32::INFINITY.to_bits());
        assert!(processor.fpscr.get_bit(1));
    }

    #[test]
    fn test_fp_mul_add_qnan_addend_still_reports_invalid_for_inf_times_zero() {
        let mut processor = Processor::new();
        processor.fpscr = 0;

        let result = processor.fp_mul_add::<u32>(
            0x7fc0_1234,
            f32::INFINITY.to_bits(),
            0.0f32.to_bits(),
            true,
        );

        assert_eq!(result, <u32 as FloatOps>::fp_default_nan());
        assert!(processor.fpscr.get_bit(0));
    }

    #[test]
    fn test_fp_sqrt_f32_round_towards_plus_infinity_rounds_up() {
        let mut processor = Processor::new();
        processor
            .fpscr
            .set_rounding_mode(FPSCRRounding::RoundTowardsPlusInfinity);

        // The root of 2 is 1.41421356237..., and the nearest float, 0x3FB504F3, is below it.
        let result = processor.fp_sqrt::<u32>(2.0f32.to_bits(), true);

        assert_eq!(result, 0x3FB5_04F4);
        assert_eq!(processor.fpscr & IXC, IXC);
    }

    #[test]
    fn test_fp_sqrt_f64_round_towards_zero_rounds_down() {
        let mut processor = Processor::new();
        processor
            .fpscr
            .set_rounding_mode(FPSCRRounding::RoundTowardsZero);

        // The nearest double, 0x3FF6A09E667F3BCD, is above the root of 2.
        let result = processor.fp_sqrt::<u64>(2.0f64.to_bits(), true);

        assert_eq!(result, 0x3FF6_A09E_667F_3BCC);
        assert_eq!(processor.fpscr & IXC, IXC);
    }

    #[test]
    fn test_fp_round_int_zero_rounding_ignores_fpscr_round_mode() {
        let mut processor = Processor::new();
        processor
            .fpscr
            .set_rounding_mode(FPSCRRounding::RoundTowardsPlusInfinity);

        let op = (-2.9f32).to_bits();
        let result = processor.fp_round_int::<u32>(op, Some(FPRounding::Zero), false, true);

        assert_eq!(result, (-2.0f32).to_bits());
    }

    #[test]
    fn test_fp_round_int_exact_sets_inexact() {
        let mut processor = Processor::new();
        processor.fpscr = 0;

        let op = 1.25f32.to_bits();
        let _ = processor.fp_round_int::<u32>(op, Some(FPRounding::Zero), true, true);

        assert!(processor.fpscr.get_bit(4));
    }

    #[test]
    fn test_fp_round_int_exact_false_does_not_set_inexact() {
        let mut processor = Processor::new();
        processor.fpscr = 0;

        let op = 1.25f32.to_bits();
        let _ = processor.fp_round_int::<u32>(op, Some(FPRounding::Zero), false, true);

        assert!(!processor.fpscr.get_bit(4));
    }

    #[test]
    fn test_fp_round_int_keeps_infinity() {
        let mut processor = Processor::new();

        let pos = processor.fp_round_int::<u32>(
            f32::INFINITY.to_bits(),
            Some(FPRounding::Zero),
            false,
            true,
        );
        let neg = processor.fp_round_int::<u32>(
            f32::NEG_INFINITY.to_bits(),
            Some(FPRounding::Zero),
            false,
            true,
        );

        assert_eq!(pos, f32::INFINITY.to_bits());
        assert_eq!(neg, f32::NEG_INFINITY.to_bits());
    }

    #[test]
    fn test_fp_round_int_preserves_signed_zero() {
        let mut processor = Processor::new();

        let pos =
            processor.fp_round_int::<u32>(0.0f32.to_bits(), Some(FPRounding::Zero), false, true);
        let neg =
            processor.fp_round_int::<u32>((-0.0f32).to_bits(), Some(FPRounding::Zero), false, true);

        assert_eq!(pos, 0.0f32.to_bits());
        assert_eq!(neg, (-0.0f32).to_bits());
    }

    #[test]
    fn test_fp_round_int_uses_fpscr_mode_when_no_rounding_is_named() {
        let mut processor = Processor::new();
        processor
            .fpscr
            .set_rounding_mode(FPSCRRounding::RoundTowardsPlusInfinity);

        let op = 2.1f32.to_bits();
        let result = processor.fp_round_int::<u32>(op, None, false, true);

        assert_eq!(result, 3.0f32.to_bits());
    }

    #[test]
    fn test_satq() {
        // 1.0 -> 1 (unsigned, 32 bit)
        assert_eq!(satq(1, 32, true), (1, false));

        // 1.0 -> 1 (signed, 32 bit)
        assert_eq!(satq(1, 32, false), (1, false));

        // 1.0 -> 1 (unsigned, 64 bit)
        assert_eq!(satq(1, 64, true), (1, false));

        // 1.0 -> 1 (signed, 64 bit)
        assert_eq!(satq(1, 64, false), (1, false));

        // -1.0 -> 0 (signed, 32 bit)
        assert_eq!(satq(-1, 32, false), (0xffff_ffff, false));

        // value bigger than max value (unsigned, 32 bit)
        assert_eq!(satq(0x1_0000_0000, 32, true), (0xffff_ffff, true));

        // value bigger than max value (signed, 32 bit)
        assert_eq!(satq(0x8000_0000, 32, false), (0x7fff_ffff, true));

        // value smaller than min value (signed, 32 bit)
        assert_eq!(satq(-0x8000_0001, 32, false), (0x8000_0000, true));

        // value bigger than max value (unsigned, 64 bit)
        assert_eq!(
            satq(0x1_0000_0000_0000_0000, 64, true),
            (0xffff_ffff_ffff_ffff, true)
        );

        // value bigger than max value (signed, 64 bit)
        assert_eq!(
            satq(0x8000_0000_0000_0000, 64, false),
            (0x7fff_ffff_ffff_ffff, true)
        );

        // value smaller than min value (signed, 64 bit)
        assert_eq!(
            satq(-0x8000_0000_0000_0001, 64, false),
            (0x8000_0000_0000_0000, true)
        );

        // negative value (unsigned, 64 bit)
        assert_eq!(satq(-1, 64, true), (0, true));
    }

    #[test]
    fn test_fp_to_fixed_f32_s32() {
        let mut processor = Processor::new();

        // 1.0 -> 1 (signed)
        assert_eq!(
            processor.fp_to_fixed::<u32>(0x3F80_0000, 32, 0, false, None, false),
            0x0000_0001
        );

        // 0.00001 -> 0 (signed) (rounding towards zero)
        assert_eq!(
            processor.fp_to_fixed::<u32>(0x3C23_D70A, 32, 0, false, None, true),
            0x0000_0000
        );

        // 42.0 -> 42 (signed)
        assert_eq!(
            processor.fp_to_fixed::<u32>(0x4228_0000, 32, 0, false, None, false),
            0x0000_002A
        );

        // -1.0 -> -1 (signed)
        assert_eq!(
            processor.fp_to_fixed::<u32>(0xBF80_0000, 32, 0, false, None, false),
            0xFFFF_FFFF
        );

        // -42.0 -> -42 (signed)
        assert_eq!(
            processor.fp_to_fixed::<u32>(0xC228_0000, 32, 0, false, None, false),
            0xFFFF_FFD6
        );

        // positive infinity -> max value
        assert_eq!(
            processor.fp_to_fixed::<u32>(0x7F80_0000, 32, 0, false, None, false),
            0x7FFF_FFFF
        );

        // negative infinity -> min value
        assert_eq!(
            processor.fp_to_fixed::<u32>(0xFF80_0000, 32, 0, false, None, false),
            0x8000_0000
        );

        // QNAN -> 0
        assert_eq!(
            processor.fp_to_fixed::<u32>(0x7FC0_0000, 32, 0, false, None, false),
            0x0000_0000
        );

        // SNAN -> 0
        assert_eq!(
            processor.fp_to_fixed::<u32>(0x7F80_0001, 32, 0, false, None, false),
            0x0000_0000
        );
    }

    #[test]
    fn test_fp_to_fixed_f32_u32() {
        let mut processor = Processor::new();

        // 1.0 -> 1 (unsigned)
        assert_eq!(
            processor.fp_to_fixed::<u32>(0x3F80_0000, 32, 0, true, None, false),
            0x0000_0001
        );

        // 42.0 -> 42 (unsigned)
        assert_eq!(
            processor.fp_to_fixed::<u32>(0x4228_0000, 32, 0, true, None, false),
            0x0000_002A
        );

        // positive infinity -> max value
        assert_eq!(
            processor.fp_to_fixed::<u32>(0x7F80_0000, 32, 0, true, None, false),
            0xFFFF_FFFF
        );

        // negative infinity -> 0
        assert_eq!(
            processor.fp_to_fixed::<u32>(0xFF80_0000, 32, 0, true, None, false),
            0x0000_0000
        );

        // -42.0 -> 0 (unsigned)
        assert_eq!(
            processor.fp_to_fixed::<u32>(0xC228_0000, 32, 0, true, None, false),
            0x0000_0000
        );

        // QNAN -> 0
        assert_eq!(
            processor.fp_to_fixed::<u32>(0x7FC0_0000, 32, 0, true, None, false),
            0x0000_0000
        );

        //SNAN -> 0
        assert_eq!(
            processor.fp_to_fixed::<u32>(0x7F80_0001, 32, 0, true, None, false),
            0x0000_0000
        );
    }

    #[test]
    fn test_fp_to_fixed_f64_s32() {
        let mut processor = Processor::new();

        // 1.0 -> 1 (signed)
        assert_eq!(
            processor.fp_to_fixed::<u64>(0x3FF0_0000_0000_0000, 32, 0, false, None, false),
            0x0000_0001
        );

        // 42.0 -> 42 (signed)
        assert_eq!(
            processor.fp_to_fixed::<u64>(0x4045_0000_0000_0000, 32, 0, false, None, false),
            0x0000_002A
        );

        // -1.0 -> -1 (signed)
        assert_eq!(
            processor.fp_to_fixed::<u64>(0xBFF0_0000_0000_0000, 32, 0, false, None, false),
            0xFFFF_FFFF
        );

        // -42.0 -> -42 (signed)
        assert_eq!(
            processor.fp_to_fixed::<u64>(0xC045_0000_0000_0000, 32, 0, false, None, false),
            0xFFFF_FFD6
        );

        // positive infinity -> max value
        assert_eq!(
            processor.fp_to_fixed::<u64>(0x7FF0_0000_0000_0000, 32, 0, false, None, false),
            0x7FFF_FFFF
        );

        // negative infinity -> min value
        assert_eq!(
            processor.fp_to_fixed::<u64>(0xFFF0_0000_0000_0000, 32, 0, false, None, false),
            0x8000_0000
        );

        // QNAN -> 0
        assert_eq!(
            processor.fp_to_fixed::<u64>(0x7FF8_0000_0000_0000, 32, 0, false, None, false),
            0x0000_0000
        );

        // SNAN -> 0
        assert_eq!(
            processor.fp_to_fixed::<u64>(0x7FF0_0000_0000_0001, 32, 0, false, None, false),
            0x0000_0000
        );
    }

    #[test]
    fn test_fp_to_fixed_f64_u32() {
        let mut processor = Processor::new();

        // 1.0 -> 1 (unsigned)
        assert_eq!(
            processor.fp_to_fixed::<u64>(0x3FF0_0000_0000_0000, 32, 0, true, None, false),
            0x0000_0001
        );

        // 42.0 -> 42 (unsigned)
        assert_eq!(
            processor.fp_to_fixed::<u64>(0x4045_0000_0000_0000, 32, 0, true, None, false),
            0x0000_002A
        );

        // positive infinity -> max value
        assert_eq!(
            processor.fp_to_fixed::<u64>(0x7FF0_0000_0000_0000, 32, 0, true, None, false),
            0xFFFF_FFFF
        );

        // negative infinity -> 0
        assert_eq!(
            processor.fp_to_fixed::<u64>(0xFFF0_0000_0000_0000, 32, 0, true, None, false),
            0x0000_0000
        );

        // -42.0 -> 0 (unsigned)
        assert_eq!(
            processor.fp_to_fixed::<u64>(0xC045_0000_0000_0000, 32, 0, true, None, false),
            0x0000_0000
        );

        // QNAN -> 0
        assert_eq!(
            processor.fp_to_fixed::<u64>(0x7FF8_0000_0000_0000, 32, 0, true, None, false),
            0x0000_0000
        );

        // SNAN -> 0
        assert_eq!(
            processor.fp_to_fixed::<u64>(0x7FF0_0000_0000_0001, 32, 0, true, None, false),
            0x0000_0000
        );
    }

    #[test]
    fn test_fixed_to_fp_s32_f32() {
        let mut processor = Processor::new();

        // 1 -> 1.0 (signed)
        assert_eq!(
            processor.fixed_to_fp::<u32, u32>(1, 0, false, false, false),
            0x3F80_0000
        );

        // 42 -> 42.0 (signed)
        assert_eq!(
            processor.fixed_to_fp::<u32, u32>(42, 0, false, false, false),
            0x4228_0000
        );

        // -1 -> -1.0 (signed)
        assert_eq!(
            processor.fixed_to_fp::<u32, u32>(0xffff_ffff, 0, false, false, false),
            0xBF80_0000
        );
    }

    #[test]
    fn test_preserve_fp_state_writes_registers_and_fpscr() {
        let mut processor = Processor::new();
        processor.fpcar = 0x2000_0100;
        processor.fpccr = 1 << FPCCR_LSPACT;
        processor.fpscr = 0xabcd_1234;

        for i in 0..16 {
            processor.set_sr(SingleReg::from(i as u8), 0x4444_0000 + i);
        }

        processor.preserve_fp_state().unwrap();

        for i in 0..16 {
            assert_eq!(
                processor.read32(0x2000_0100 + i * 4).unwrap(),
                0x4444_0000 + i
            );
        }
        assert_eq!(processor.read32(0x2000_0140).unwrap(), 0xabcd_1234);
        assert!(!processor.fpccr.get_bit(FPCCR_LSPACT));
    }

    #[test]
    fn test_preserve_fp_state_uses_unprivileged_access_type_when_user_bit_set() {
        let mut processor = Processor::new();
        processor.fpcar = 0x2000_0180;
        processor.fpccr = (1 << FPCCR_LSPACT) | (1 << FPCCR_USER);
        processor.set_sr(SingleReg::S0, 0xdead_beef);

        processor.preserve_fp_state().unwrap();

        assert_eq!(processor.read32(0x2000_0180).unwrap(), 0xdead_beef);
        assert!(!processor.fpccr.get_bit(FPCCR_LSPACT));
    }

    #[test]
    fn test_check_vfp_enabled_rejects_mismatched_coprocessor_access() {
        let mut processor = Processor::new();
        processor.cpacr = 0x0010_0000;

        assert_eq!(processor.check_vfp_enabled(), Err(Fault::Forced));
    }

    #[test]
    fn test_check_vfp_enabled_sets_nocp_when_vfp_disabled() {
        let mut processor = Processor::new();
        processor.cfsr = 0;

        assert_eq!(processor.check_vfp_enabled(), Err(Fault::Nocp));
        assert!(processor.cfsr.get_bit(19));
    }

    #[test]
    fn test_check_vfp_enabled_requires_privilege_for_privileged_only_access() {
        let mut processor = Processor::new();
        processor.cpacr = 0x0050_0000;
        processor.control.n_priv = true;

        assert_eq!(processor.check_vfp_enabled(), Err(Fault::Nocp));
        assert!(processor.cfsr.get_bit(19));
    }

    #[test]
    fn test_check_vfp_enabled_allows_privileged_only_access_in_privileged_mode() {
        let mut processor = Processor::new();
        processor.cpacr = 0x0050_0000;

        assert_eq!(processor.check_vfp_enabled(), Ok(()));
    }

    #[test]
    fn test_check_vfp_enabled_rejects_reserved_access_encoding() {
        let mut processor = Processor::new();
        processor.cpacr = 0x00a0_0000;

        assert_eq!(processor.check_vfp_enabled(), Err(Fault::Nocp));
        assert!(processor.cfsr.get_bit(19));
    }

    #[test]
    fn test_execute_fp_check_preserves_lazy_state_and_loads_fpdscr_control_bits() {
        let mut processor = Processor::new();
        processor.cpacr = 0x00f0_0000;
        processor.fpcar = 0x2000_0200;
        processor.fpccr = (1 << FPCCR_LSPACT) | (1 << FPCCR_ASPEN);
        processor.fpdscr = 0x07c0_0000;
        processor.fpscr = 0;
        processor.set_sr(SingleReg::S0, 0x1357_9bdf);

        processor.execute_fp_check().unwrap();

        assert_eq!(processor.read32(0x2000_0200).unwrap(), 0x1357_9bdf);
        assert_eq!(
            processor
                .fpscr
                .get_bits(FPSCR_STATUS_CONTROL_START..FPSCR_STATUS_CONTROL_END),
            processor
                .fpdscr
                .get_bits(FPSCR_STATUS_CONTROL_START..FPSCR_STATUS_CONTROL_END)
        );
        assert!(processor.control.fpca);
    }

    #[test]
    fn fp_round_int_f64_matches_the_host_for_every_rounding() {
        for rounding in ALL_ROUNDINGS {
            for x in F64_VALUES {
                let mut processor = Processor::new();
                let result =
                    processor.fp_round_int::<u64>(x.to_bits(), Some(rounding), false, true);
                assert_eq!(
                    result,
                    host_rint64(x, rounding).to_bits(),
                    "{rounding:?} {x:e}"
                );
            }
        }
    }

    #[test]
    fn fp_round_int_f32_matches_the_host_for_every_rounding() {
        for rounding in ALL_ROUNDINGS {
            for x in F32_VALUES {
                let mut processor = Processor::new();
                let result =
                    processor.fp_round_int::<u32>(x.to_bits(), Some(rounding), false, true);
                assert_eq!(
                    result,
                    host_rint32(x, rounding).to_bits(),
                    "{rounding:?} {x:e}"
                );
            }
        }
    }

    #[test]
    fn fp_round_int_takes_the_fpscr_mode_when_none_is_named() {
        let modes = [
            (FPSCRRounding::RoundToNearest, FPRounding::TieEven),
            (FPSCRRounding::RoundTowardsPlusInfinity, FPRounding::PosInf),
            (FPSCRRounding::RoundTowardsMinusInfinity, FPRounding::NegInf),
            (FPSCRRounding::RoundTowardsZero, FPRounding::Zero),
        ];
        for (mode, rounding) in modes {
            for x in [2.5f64, -2.5, 3.5, 0.3, -0.3] {
                let mut processor = Processor::new();
                processor.fpscr.set_rounding_mode(mode);
                let result = processor.fp_round_int::<u64>(x.to_bits(), None, false, true);
                assert_eq!(result, host_rint64(x, rounding).to_bits(), "{mode:?} {x}");
            }
        }
    }

    #[test]
    fn fp_round_int_leaves_the_flags_alone_unless_exact_is_asked_for() {
        for rounding in ALL_ROUNDINGS {
            let mut processor = Processor::new();
            processor.fp_round_int::<u64>(2.5f64.to_bits(), Some(rounding), false, true);
            assert_eq!(processor.fpscr, 0, "{rounding:?}");

            processor.fp_round_int::<u64>(2.5f64.to_bits(), Some(rounding), true, true);
            assert_eq!(processor.fpscr, IXC, "{rounding:?}");
        }
        // A whole number is exact, large ones included.
        for x in [
            2.0f64,
            -7.0,
            0.0,
            4_503_599_627_370_496.0,
            1e300,
            f64::INFINITY,
        ] {
            let mut processor = Processor::new();
            processor.fp_round_int::<u64>(x.to_bits(), Some(FPRounding::Zero), true, true);
            assert_eq!(processor.fpscr, 0, "{x}");
        }
    }

    #[test]
    fn fp_round_int_quiets_a_signalling_nan_and_keeps_a_quiet_one() {
        let mut processor = Processor::new();
        let quiet = 0x7FF8_0000_0000_1234u64;
        assert_eq!(
            processor.fp_round_int::<u64>(quiet, Some(FPRounding::TieAway), false, true),
            quiet
        );
        assert_eq!(processor.fpscr, 0);

        let signalling = 0x7FF0_0000_0000_1234u64;
        assert_eq!(
            processor.fp_round_int::<u64>(signalling, Some(FPRounding::TieAway), false, true),
            quiet
        );
        assert_eq!(processor.fpscr, IOC);

        let mut processor = Processor::new();
        processor.fpscr = DN;
        assert_eq!(
            processor.fp_round_int::<u32>(0x7FC0_1234, Some(FPRounding::TieAway), false, true),
            0x7FC0_0000
        );
    }

    #[test]
    fn fp_round_int_flushes_a_subnormal_input_when_fz_is_set() {
        let mut processor = Processor::new();
        processor.fpscr = FZ;
        // Rounded up it would be 1.0 and rounded down it is -0.0; flushed first, both are zero.
        let tiny = f32::from_bits(0x0000_0001);
        assert_eq!(
            processor.fp_round_int::<u32>(tiny.to_bits(), Some(FPRounding::PosInf), false, true),
            0
        );
        assert_eq!(processor.fpscr, FZ | IDC);
        assert_eq!(
            processor.fp_round_int::<u32>((-tiny).to_bits(), Some(FPRounding::NegInf), false, true),
            (-0.0f32).to_bits()
        );

        let mut processor = Processor::new();
        assert_eq!(
            processor.fp_round_int::<u32>(tiny.to_bits(), Some(FPRounding::PosInf), false, true),
            1.0f32.to_bits()
        );
        assert_eq!(processor.fpscr, 0);
    }

    // The conversion the host does with `as` saturates and sends a NaN to zero, which is what
    // the instruction does to a result that does not fit.
    #[test]
    fn fp_to_fixed_matches_the_host_for_every_rounding() {
        for rounding in ALL_ROUNDINGS {
            for x in F64_VALUES {
                let mut processor = Processor::new();
                let rounded = host_rint64(x, rounding);
                let signed =
                    processor.fp_to_fixed::<u64>(x.to_bits(), 32, 0, false, Some(rounding), true);
                assert_eq!(
                    signed as u32, rounded as i32 as u32,
                    "{rounding:?} {x:e} signed"
                );
                let unsigned =
                    processor.fp_to_fixed::<u64>(x.to_bits(), 32, 0, true, Some(rounding), true);
                assert_eq!(
                    unsigned as u32, rounded as u32,
                    "{rounding:?} {x:e} unsigned"
                );
            }
            for x in F32_VALUES {
                let mut processor = Processor::new();
                let rounded = host_rint32(x, rounding);
                let signed =
                    processor.fp_to_fixed::<u32>(x.to_bits(), 32, 0, false, Some(rounding), true);
                assert_eq!(
                    signed as u32, rounded as i32 as u32,
                    "{rounding:?} {x:e} signed"
                );
                let unsigned =
                    processor.fp_to_fixed::<u32>(x.to_bits(), 32, 0, true, Some(rounding), true);
                assert_eq!(
                    unsigned as u32, rounded as u32,
                    "{rounding:?} {x:e} unsigned"
                );
            }
        }
    }

    #[test]
    fn fp_to_fixed_sends_a_nan_to_zero_and_signals_it() {
        for nan in [0x7FF8_0000_0000_0000u64, 0x7FF0_0000_0000_0001] {
            let mut processor = Processor::new();
            assert_eq!(
                processor.fp_to_fixed::<u64>(nan, 32, 0, false, Some(FPRounding::TieEven), true),
                0
            );
            assert_eq!(processor.fpscr & IOC, IOC);
        }
    }

    #[test]
    fn fp_to_fixed_signals_invalid_operation_when_the_result_saturates() {
        for (x, unsigned) in [(3e9f32, false), (-1.0, true), (f32::INFINITY, false)] {
            let mut processor = Processor::new();
            processor.fp_to_fixed::<u32>(x.to_bits(), 32, 0, unsigned, None, true);
            assert_eq!(processor.fpscr, IOC, "{x} unsigned {unsigned}");
        }
        // 16 bits saturate sooner than 32, and so do 32 with fraction bits.
        let mut processor = Processor::new();
        processor.fp_to_fixed::<u32>(40000.0f32.to_bits(), 16, 0, false, None, true);
        assert_eq!(processor.fpscr, IOC);
        let mut processor = Processor::new();
        processor.fp_to_fixed::<u32>(40000.0f32.to_bits(), 32, 16, false, None, true);
        assert_eq!(processor.fpscr, IOC);
    }

    #[test]
    fn fp_to_fixed_signals_inexact_only_when_it_drops_a_fraction() {
        let mut processor = Processor::new();
        processor.fp_to_fixed::<u32>(2.5f32.to_bits(), 32, 0, false, None, true);
        assert_eq!(processor.fpscr, IXC);

        // 2.5 with one fraction bit is exactly 5.
        let mut processor = Processor::new();
        let result = processor.fp_to_fixed::<u32>(2.5f32.to_bits(), 32, 1, false, None, true);
        assert_eq!((result, processor.fpscr), (5, 0));

        let mut processor = Processor::new();
        processor.fp_to_fixed::<u32>(7.0f32.to_bits(), 32, 0, false, None, true);
        assert_eq!(processor.fpscr, 0);
    }

    #[test]
    fn fp_to_fixed_scales_by_the_fraction_bits() {
        let mut processor = Processor::new();
        // 1.5 * 2^16, and -1.5 * 2^16 of a double, which is 32 bits of the two's complement.
        assert_eq!(
            processor.fp_to_fixed::<u32>(1.5f32.to_bits(), 32, 16, false, None, true),
            0x1_8000
        );
        assert_eq!(
            processor.fp_to_fixed::<u64>((-1.5f64).to_bits(), 32, 16, false, None, true),
            0xFFFE_8000
        );
    }

    #[test]
    fn fp_to_fixed_rounds_a_tie_to_the_even_neighbour_in_the_fpscr_nearest_mode() {
        let mut processor = Processor::new();
        for (x, expected) in [
            (0.5f32, 0u32),
            (1.5, 2),
            (2.5, 2),
            (3.5, 4),
            (-0.5, 0),
            (-1.5, !1),
            (-2.5, !1),
        ] {
            assert_eq!(
                processor.fp_to_fixed::<u32>(x.to_bits(), 32, 0, false, None, true) as u32,
                expected,
                "{x}"
            );
        }
    }

    fn max_num64(processor: &mut Processor, a: f64, b: f64) -> u64 {
        processor.fp_min_max_num::<u64>(a.to_bits(), b.to_bits(), true)
    }

    fn min_num64(processor: &mut Processor, a: f64, b: f64) -> u64 {
        processor.fp_min_max_num::<u64>(a.to_bits(), b.to_bits(), false)
    }

    #[test]
    fn fp_min_max_num_f64_matches_the_host_on_numbers() {
        // The host leaves the order of the two zeros open, so they are checked below.
        let mut processor = Processor::new();
        for a in F64_VALUES {
            for b in F64_VALUES {
                if a == 0.0 && b == 0.0 {
                    continue;
                }
                assert_eq!(
                    max_num64(&mut processor, a, b),
                    a.max(b).to_bits(),
                    "max {a:e} {b:e}"
                );
                assert_eq!(
                    min_num64(&mut processor, a, b),
                    a.min(b).to_bits(),
                    "min {a:e} {b:e}"
                );
            }
        }
        assert_eq!(processor.fpscr, 0);
    }

    #[test]
    fn fp_min_max_num_f32_matches_the_host_on_numbers() {
        let mut processor = Processor::new();
        for a in F32_VALUES {
            for b in F32_VALUES {
                if a == 0.0 && b == 0.0 {
                    continue;
                }
                assert_eq!(
                    processor.fp_min_max_num::<u32>(a.to_bits(), b.to_bits(), true),
                    a.max(b).to_bits(),
                    "max {a:e} {b:e}"
                );
                assert_eq!(
                    processor.fp_min_max_num::<u32>(a.to_bits(), b.to_bits(), false),
                    a.min(b).to_bits(),
                    "min {a:e} {b:e}"
                );
            }
        }
    }

    #[test]
    fn fp_min_max_num_treats_plus_zero_as_larger_than_minus_zero() {
        let mut processor = Processor::new();
        for (a, b) in [(0.0, -0.0), (-0.0, 0.0)] {
            assert_eq!(max_num64(&mut processor, a, b), 0.0f64.to_bits());
            assert_eq!(min_num64(&mut processor, a, b), (-0.0f64).to_bits());
        }
        assert_eq!(max_num64(&mut processor, -0.0, -0.0), (-0.0f64).to_bits());
        assert_eq!(min_num64(&mut processor, 0.0, 0.0), 0.0f64.to_bits());
    }

    #[test]
    fn fp_min_max_num_prefers_a_number_to_a_quiet_nan() {
        let mut processor = Processor::new();
        let nan = f64::NAN.to_bits();
        for number in [1.5f64, -1.5, 0.0, -0.0, f64::INFINITY, f64::NEG_INFINITY] {
            for max in [true, false] {
                assert_eq!(
                    processor.fp_min_max_num::<u64>(nan, number.to_bits(), max),
                    number.to_bits()
                );
                assert_eq!(
                    processor.fp_min_max_num::<u64>(number.to_bits(), nan, max),
                    number.to_bits()
                );
            }
        }
        assert_eq!(processor.fpscr, 0);
        // Two quiet NaNs give the first one.
        let first = 0x7FF8_0000_0000_0001u64;
        let second = 0x7FF8_0000_0000_0002u64;
        assert_eq!(processor.fp_min_max_num::<u64>(first, second, true), first);
        assert_eq!(processor.fpscr, 0);
    }

    #[test]
    fn fp_min_max_num_returns_a_signalling_nan_quieted_and_signals_it() {
        let signalling = 0x7FF0_0000_0000_0001u64;
        let quiet = 0x7FF8_0000_0000_0001u64;
        for (a, b) in [
            (signalling, 1.0f64.to_bits()),
            (1.0f64.to_bits(), signalling),
            (f64::NAN.to_bits(), signalling),
        ] {
            let mut processor = Processor::new();
            assert_eq!(processor.fp_min_max_num::<u64>(a, b, true), quiet);
            assert_eq!(processor.fpscr, IOC);
        }
        let mut processor = Processor::new();
        processor.fpscr = DN;
        assert_eq!(
            processor.fp_min_max_num::<u64>(signalling, 1.0f64.to_bits(), false),
            0x7FF8_0000_0000_0000
        );
    }

    #[test]
    fn fp_min_max_num_flushes_a_subnormal_when_fz_is_set() {
        let mut processor = Processor::new();
        processor.fpscr = FZ;
        let tiny = f32::from_bits(1).to_bits();
        // max(tiny, -1) would be tiny; flushed, it is zero.
        assert_eq!(
            processor.fp_min_max_num::<u32>(tiny, (-1.0f32).to_bits(), true),
            0
        );
        assert_eq!(processor.fpscr, FZ | IDC);
    }

    #[test]
    fn fp_round_int_f64_leaves_a_value_past_the_fraction_bits_alone() {
        // These are whole numbers too large for the 128-bit integer the rounding works in.
        for x in [1e40f64, -1e100, f64::MAX, f64::MIN] {
            let mut processor = Processor::new();
            assert_eq!(
                processor.fp_round_int::<u64>(x.to_bits(), Some(FPRounding::TieEven), true, true),
                x.to_bits()
            );
            assert_eq!(processor.fpscr, 0);
        }
    }
}
