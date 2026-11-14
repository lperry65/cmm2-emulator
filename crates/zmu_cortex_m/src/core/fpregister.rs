//!
//! Cortex Floating Point extension register operations
//!

use crate::core::bits::Bits;

#[derive(PartialEq, Debug, Copy, Clone)]
/// Enumeration of supported Floating Point rounding modes
pub enum FPSCRRounding {
    /// Rounding to nearest representable value
    RoundToNearest,
    /// Rounding towards plus infinity
    RoundTowardsPlusInfinity,
    /// Rounding towards minus infinity
    RoundTowardsMinusInfinity,
    /// Rounding towards zero
    RoundTowardsZero,
}

/// The rounding modes of the pseudocode's FPRounding. The FPSCR names four of them; VRINTA and
/// VCVTA name the fifth, ties away from zero.
#[derive(PartialEq, Debug, Copy, Clone)]
pub enum FPRounding {
    /// Nearest, a tie going to the even neighbour
    TieEven,
    /// Towards plus infinity
    PosInf,
    /// Towards minus infinity
    NegInf,
    /// Towards zero
    Zero,
    /// Nearest, a tie going to the neighbour away from zero
    TieAway,
}

impl FPRounding {
    /// The rounding the two RM bits of VRINTA/N/P/M and VCVTA/N/P/M name, in that order.
    pub fn from_rm(rm: u32) -> Self {
        match rm {
            0 => Self::TieAway,
            1 => Self::TieEven,
            2 => Self::PosInf,
            _ => Self::NegInf,
        }
    }
}

impl From<FPSCRRounding> for FPRounding {
    fn from(mode: FPSCRRounding) -> Self {
        match mode {
            FPSCRRounding::RoundToNearest => Self::TieEven,
            FPSCRRounding::RoundTowardsPlusInfinity => Self::PosInf,
            FPSCRRounding::RoundTowardsMinusInfinity => Self::NegInf,
            FPSCRRounding::RoundTowardsZero => Self::Zero,
        }
    }
}

/// Trait for accessing Floating Point registers
pub trait Fpscr {
    ///
    /// Get "N"egative flag value
    ///
    fn get_n(&self) -> bool;

    ///
    /// Set "N"egative flag value
    ///
    fn set_n(&mut self, n: bool);

    ///
    /// Get "Z"ero flag value
    ///
    fn get_z(&self) -> bool;
    ///
    /// Set "Z"ero flag value
    ///
    fn set_z(&mut self, z: bool);

    ///
    /// Get "C"arry flag value
    ///
    fn get_c(&self) -> bool;
    ///
    /// Set "C"arry flag value
    ///
    fn set_c(&mut self, c: bool);

    ///
    /// Get Overflow flag value
    ///
    fn get_v(&self) -> bool;
    ///
    /// Set Overflow flag value
    ///
    fn set_v(&mut self, v: bool);

    ///
    /// Set UFC flag: Underflow cumulative exception.
    ///
    fn set_ufc(&mut self, ufc: bool);

    ///
    /// Get default NaN mode
    ///
    /// false: NaNs operands propagate
    /// true: any operation involving one or more NaNs returns Default NaN
    fn get_dn(&self) -> bool;

    ///
    /// Get FZ bit (Flush-to-zero mode)
    ///
    /// false: Flush-to-zero mode disabled: fully compliant with IEEE 754 standard
    /// true: Flush-to-zero mode enabled
    fn get_fz(&self) -> bool;

    ///
    /// Get the current rounding mode
    ///
    fn get_rounding_mode(&self) -> FPSCRRounding;

    ///
    /// Set the current rounding mode
    ///
    fn set_rounding_mode(&mut self, mode: FPSCRRounding);
}

impl Fpscr for u32 {
    fn get_n(&self) -> bool {
        self.get_bit(31)
    }

    fn set_n(&mut self, n: bool) {
        self.set_bit(31, n);
    }

    fn get_z(&self) -> bool {
        self.get_bit(30)
    }

    fn set_z(&mut self, z: bool) {
        self.set_bit(30, z);
    }

    fn get_c(&self) -> bool {
        self.get_bit(29)
    }

    fn set_c(&mut self, c: bool) {
        self.set_bit(29, c);
    }

    fn get_v(&self) -> bool {
        self.get_bit(28)
    }

    fn set_v(&mut self, v: bool) {
        self.set_bit(28, v);
    }

    fn set_ufc(&mut self, ufc: bool) {
        self.set_bit(3, ufc);
    }

    fn get_fz(&self) -> bool {
        self.get_bit(24)
    }

    fn get_dn(&self) -> bool {
        self.get_bit(25)
    }

    fn get_rounding_mode(&self) -> FPSCRRounding {
        match self.get_bits(22..24) {
            0 => FPSCRRounding::RoundToNearest,
            1 => FPSCRRounding::RoundTowardsPlusInfinity,
            2 => FPSCRRounding::RoundTowardsMinusInfinity,
            // a two bit field, so 3 is the value left
            _ => FPSCRRounding::RoundTowardsZero,
        }
    }

    fn set_rounding_mode(&mut self, mode: FPSCRRounding) {
        let mode = match mode {
            FPSCRRounding::RoundToNearest => 0,
            FPSCRRounding::RoundTowardsPlusInfinity => 1,
            FPSCRRounding::RoundTowardsMinusInfinity => 2,
            FPSCRRounding::RoundTowardsZero => 3,
        };
        self.set_bits(22..24, mode);
    }
}
