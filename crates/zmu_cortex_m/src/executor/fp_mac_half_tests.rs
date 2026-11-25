//! VMLA, VMLS, VNMLA and VNMLS (A7.7.238 and A7.7.250), VCVTB and VCVTT (A7.7.231), and
//! FLDMX and FSTMX (D2.3.1). The multiply-accumulate family rounds the product and the sum
//! separately (it is not a fused multiply-add), so one case below is chosen where the fused
//! and the two-roundings answers differ. The half-precision conversions are checked against
//! the binary16 values worked out by hand from A2.5 (`FPHalfToSingle`, `FPSingleToHalf`, ...).

use super::Executor;
use super::ExecutorHelper;
use super::fp_data_processing::IsaFloatingPointDataProcessing;
use super::fp_load_and_store::IsaFloatingPointLoadAndStore;
use super::fp_test_support::*;
use crate::core::condition::Condition;
use crate::core::instruction::{
    AddressingMode, VAddSubParamsf32, VAddSubParamsf64, VCVTParamsF16F32, VCVTParamsF16F64,
    VCVTParamsF32F16, VCVTParamsF64F16, VStoreMultipleParams64,
};
use crate::core::register::{Apsr, BaseReg, DoubleReg, ExtensionRegOperations, Reg, SingleReg};
use crate::decoder::decode_32;
use crate::executor::{ExecuteResult, ExecuteSuccess};
use crate::{Processor, bus::Bus};
use enum_set::EnumSet;

// llvm-mc for thumbv7em-unknown-eabi with +fp-armv8. The accumulator is s0 (or d0) and the
// factors are s2 and s3 (or d2 and d3), so reads and write go to different registers.
const VMLA_F32: u32 = 0xee01_0a21;
const VMLS_F32: u32 = 0xee01_0a61;
const VNMLA_F32: u32 = 0xee11_0a61;
const VNMLS_F32: u32 = 0xee11_0a21;
const VMLA_F64: u32 = 0xee02_0b03;
const VMLS_F64: u32 = 0xee02_0b43;
const VNMLA_F64: u32 = 0xee12_0b43;
const VNMLS_F64: u32 = 0xee12_0b03;

// llvm-mc: vcvtb.f32.f16 s0, s2 and the rest of the seven, always with source and destination
// in different registers.
const VCVTB_F32_F16: u32 = 0xeeb2_0a41;
const VCVTT_F32_F16: u32 = 0xeeb2_0ac1;
const VCVTB_F16_F32: u32 = 0xeeb3_0a41;
const VCVTT_F16_F32: u32 = 0xeeb3_0ac1;
const VCVTB_F64_F16: u32 = 0xeeb2_0b41;
const VCVTT_F64_F16: u32 = 0xeeb2_0bc1;
const VCVTB_F16_F64: u32 = 0xeeb3_0b42;
const VCVTT_F16_F64: u32 = 0xeeb3_0bc2;

fn fpu() -> Processor {
    let mut processor = Processor::new();
    processor.cpacr = 0x00f0_0000;
    processor
}

/// A multiply-accumulate with the accumulator in s0 and the factors in s2 and s3, giving the
/// new accumulator and the cumulative exception flags it left behind.
fn mac_f32(opcode: u32, d: u32, n: u32, m: u32) -> (u32, u32) {
    mac_f32_fpscr(opcode, d, n, m, 0)
}

/// The same with FPSCR seeded, for the tests that run in a named rounding mode.
fn mac_f32_fpscr(opcode: u32, d: u32, n: u32, m: u32, fpscr: u32) -> (u32, u32) {
    let mut processor = fpu();
    processor.fpscr = fpscr;
    processor.set_sr(SingleReg::S0, d);
    processor.set_sr(SingleReg::S2, n);
    processor.set_sr(SingleReg::S3, m);
    processor.execute(&decode_32(opcode), 4);
    (
        processor.get_sr(SingleReg::S0),
        processor.fpscr & CUMULATIVE_FLAGS,
    )
}

fn mac_f64(opcode: u32, d: f64, n: f64, m: f64) -> (u64, u32) {
    let mut processor = fpu();
    processor.set_dr_bits(DoubleReg::D0, d.to_bits());
    processor.set_dr_bits(DoubleReg::D2, n.to_bits());
    processor.set_dr_bits(DoubleReg::D3, m.to_bits());
    processor.execute(&decode_32(opcode), 4);
    (
        processor.get_dr_bits(DoubleReg::D0),
        processor.fpscr & CUMULATIVE_FLAGS,
    )
}

/// A conversion that reads the single s2 (whole register, bottom half or top half) and
/// writes s0, which starts at `dest_before`.
fn convert(opcode: u32, source: u32, dest_before: u32, fpscr: u32) -> (u32, u32) {
    let mut processor = fpu();
    processor.fpscr = fpscr;
    processor.set_sr(SingleReg::S2, source);
    processor.set_sr(SingleReg::S0, dest_before);
    processor.execute(&decode_32(opcode), 4);
    (
        processor.get_sr(SingleReg::S0),
        processor.fpscr & CUMULATIVE_FLAGS,
    )
}

/// A conversion that reads the double d2 and writes the single s0, with FPSCR seeded.
fn convert_double(opcode: u32, source: u64, dest_before: u32, fpscr: u32) -> (u32, u32) {
    let mut processor = fpu();
    processor.fpscr = fpscr;
    processor.set_dr_bits(DoubleReg::D2, source);
    processor.set_sr(SingleReg::S0, dest_before);
    processor.execute(&decode_32(opcode), 4);
    (
        processor.get_sr(SingleReg::S0),
        processor.fpscr & CUMULATIVE_FLAGS,
    )
}

fn double_register(number: u8) -> DoubleReg {
    match number {
        0 => DoubleReg::D0,
        1 => DoubleReg::D1,
        2 => DoubleReg::D2,
        3 => DoubleReg::D3,
        4 => DoubleReg::D4,
        5 => DoubleReg::D5,
        6 => DoubleReg::D6,
        7 => DoubleReg::D7,
        8 => DoubleReg::D8,
        9 => DoubleReg::D9,
        10 => DoubleReg::D10,
        11 => DoubleReg::D11,
        12 => DoubleReg::D12,
        13 => DoubleReg::D13,
        14 => DoubleReg::D14,
        15 => DoubleReg::D15,
        _ => panic!("the test has no register d{number}"),
    }
}

// The multiply-accumulate family.

#[test]
fn test_vmla_f32_adds_the_product_to_the_accumulator() {
    // 1.0 + 1.5 * 2.5 = 4.75, all exact.
    let (result, flags) = mac_f32(
        VMLA_F32,
        1.0f32.to_bits(),
        1.5f32.to_bits(),
        2.5f32.to_bits(),
    );
    assert_eq!(f32::from_bits(result), 4.75);
    assert_eq!(flags, 0);

    // 0.1 + 3.0 * 7.0 in two roundings, which is what the host's separate operations do.
    let (result, flags) = mac_f32(
        VMLA_F32,
        0.1f32.to_bits(),
        3.0f32.to_bits(),
        7.0f32.to_bits(),
    );
    assert_eq!(f32::from_bits(result), two_step(0.1, 3.0, 7.0));
    assert_eq!(flags, IXC);
}

// The host's own multiply-accumulate: the product rounds, and the sum rounds again.
#[inline(never)]
fn two_step(d: f32, n: f32, m: f32) -> f32 {
    d + (n * m)
}

#[test]
fn test_vmla_f32_rounds_the_product_before_the_sum() {
    // A search over values near 1.0 found this triple where two roundings give 0x40000694 and
    // a fused multiply-add would give 0x40000695 (exact arithmetic on the host's floats).
    let d = f32::from_bits(0x3f80_08cd);
    let n = f32::from_bits(0x3f80_0390);
    let m = f32::from_bits(0x3f80_00cc);
    let (result, _) = mac_f32(VMLA_F32, d.to_bits(), n.to_bits(), m.to_bits());
    assert_eq!(result, 0x4000_0694);
    assert_eq!(n.mul_add(m, d).to_bits(), 0x4000_0695); // the fused answer, for the record
}

#[test]
fn test_vmls_f32_subtracts_the_product() {
    // 1.0 - 1.5 * 2.5 = -2.75, all exact.
    let (result, flags) = mac_f32(
        VMLS_F32,
        1.0f32.to_bits(),
        1.5f32.to_bits(),
        2.5f32.to_bits(),
    );
    assert_eq!(f32::from_bits(result), -2.75);
    assert_eq!(flags, 0);
}

#[test]
fn test_vnmla_f32_negates_the_product_and_the_accumulator() {
    // -1.0 - 1.5 * 2.5 = -4.75, all exact.
    let (result, flags) = mac_f32(
        VNMLA_F32,
        1.0f32.to_bits(),
        1.5f32.to_bits(),
        2.5f32.to_bits(),
    );
    assert_eq!(f32::from_bits(result), -4.75);
    assert_eq!(flags, 0);
}

#[test]
fn test_vnmls_f32_negates_the_accumulator_only() {
    // -1.0 + 1.5 * 2.5 = 2.75, all exact.
    let (result, flags) = mac_f32(
        VNMLS_F32,
        1.0f32.to_bits(),
        1.5f32.to_bits(),
        2.5f32.to_bits(),
    );
    assert_eq!(f32::from_bits(result), 2.75);
    assert_eq!(flags, 0);
}

#[test]
fn test_the_sign_of_a_nan_result_follows_the_negations() {
    // FPNeg turns the sign bit over without any other treatment (A2.5), so a negated NaN has
    // the other sign. A qNaN factor: VMLA leaves it, VMLS negates it. A qNaN accumulator:
    // VMLA leaves it, VNMLA and VNMLS negate it.
    let qnan = 0x7fc0_0000_u32;
    let one = 1.0f32.to_bits();
    let two = 2.0f32.to_bits();
    assert_eq!(mac_f32(VMLA_F32, one, qnan, two).0, qnan);
    assert_eq!(mac_f32(VMLS_F32, one, qnan, two).0, 0xffc0_0000);
    assert_eq!(mac_f32(VMLA_F32, qnan, two, two).0, qnan);
    assert_eq!(mac_f32(VNMLA_F32, qnan, two, two).0, 0xffc0_0000);
    assert_eq!(mac_f32(VNMLS_F32, qnan, two, two).0, 0xffc0_0000);
}

#[test]
fn test_the_accumulator_nan_payload_wins_over_the_factor_payload() {
    // The pseudocode adds with the accumulator first: FPAdd(operand1, operand2) with
    // operand1 = accumulator (A7.7.238), and a quiet NaN operand passes its payload through,
    // so with two different payloads and DN clear the accumulator's payload is the result
    // whatever the factor brings. A swapped order would show the factor's payload instead.
    let acc = 0x7fc0_1234_u32;
    let factor = 0x7fc0_5678_u32;
    let one = 1.0f32.to_bits();
    assert_eq!(mac_f32(VMLA_F32, acc, factor, one), (acc, 0));
    assert_eq!(mac_f32(VMLS_F32, acc, factor, one), (acc, 0));
    assert_eq!(mac_f32(VNMLA_F32, acc, factor, one), (0xffc0_1234, 0));
    assert_eq!(mac_f32(VNMLS_F32, acc, factor, one), (0xffc0_1234, 0));
    let acc64 = f64::from_bits(0x7ff8_0000_0000_1234);
    let factor64 = f64::from_bits(0x7ff8_0000_0000_5678);
    assert_eq!(
        mac_f64(VMLA_F64, acc64, factor64, 1.0),
        (0x7ff8_0000_0000_1234, 0)
    );
    assert_eq!(
        mac_f64(VMLS_F64, acc64, factor64, 1.0),
        (0x7ff8_0000_0000_1234, 0)
    );
    assert_eq!(
        mac_f64(VNMLA_F64, acc64, factor64, 1.0),
        (0xfff8_0000_0000_1234, 0)
    );
    assert_eq!(
        mac_f64(VNMLS_F64, acc64, factor64, 1.0),
        (0xfff8_0000_0000_1234, 0)
    );
}

#[test]
fn test_the_product_takes_the_payload_of_the_first_factor() {
    // FPProcessNaNs works through its operands in order, first one first (A2.5), so the
    // product of two quiet NaNs carries the payload of Sn, the first factor. The accumulator
    // here is a number, so the payload that reaches the result is the product's: a swapped
    // fp_mul(op2, op1) would show 0x7fc0_5678.
    let (result, flags) = mac_f32(VMLA_F32, 1.0f32.to_bits(), 0x7fc0_1234, 0x7fc0_5678);
    assert_eq!(result, 0x7fc0_1234);
    assert_eq!(flags, 0);
}

#[test]
fn test_the_negation_of_a_multiply_accumulate_turns_the_operand_not_the_sum() {
    // A7.7.250 negates the operand, never the result: VNMLS adds the product to a negated
    // accumulator, not a negated sum. With zero and a zero product the two differ only in
    // the sign of the zero: negated-accumulator + product is (-0) + (+0), which is +0 in
    // round-to-nearest-even; negating the sum would give -0 here.
    let (result, flags) = mac_f32(
        VNMLS_F32,
        0.0f32.to_bits(),
        1.0f32.to_bits(),
        0.0f32.to_bits(),
    );
    assert_eq!(result, 0x0000_0000);
    assert_eq!(flags, 0);
    // VNMLA turns both operands: (-0) + neg(1.0 * -0.0) is also (-0) + (+0). The sign of an
    // exact zero sum follows the rounding mode (round-towards-minus-infinity gives -0), so
    // the mode shows the add really happened operand-wise with a negated accumulator.
    let (result, _) = mac_f32(
        VNMLA_F32,
        0.0f32.to_bits(),
        1.0f32.to_bits(),
        (-0.0f32).to_bits(),
    );
    assert_eq!(result, 0x0000_0000);
    let (result, _) = mac_f32_fpscr(
        VNMLA_F32,
        0.0f32.to_bits(),
        1.0f32.to_bits(),
        (-0.0f32).to_bits(),
        RM,
    );
    assert_eq!(result, 0x8000_0000);
}

#[test]
fn test_a_signalling_nan_factor_is_quieted_once() {
    // The product sees the sNaN first: it is quieted and signals Invalid Operation, and the
    // sign of the quieted product then takes the instruction's negation.
    let snan = 0x7f80_0001_u32;
    let one = 1.0f32.to_bits();
    let two = 2.0f32.to_bits();
    let (result, flags) = mac_f32(VMLA_F32, one, snan, two);
    assert_eq!(result, 0x7fc0_0001);
    assert_eq!(flags, IOC);
    let (result, flags) = mac_f32(VMLS_F32, one, snan, two);
    assert_eq!(result, 0xffc0_0001);
    assert_eq!(flags, IOC);
}

#[test]
fn test_infinity_times_zero_gives_the_default_nan() {
    let inf = f32::INFINITY.to_bits();
    let zero = 0.0f32.to_bits();
    let five = 5.0f32.to_bits();
    let (result, flags) = mac_f32(VMLA_F32, five, inf, zero);
    assert_eq!(result, 0x7fc0_0000);
    assert_eq!(flags, IOC);
}

#[test]
fn test_the_signed_zeros_of_a_multiply_accumulate() {
    // -0.0 + (-0.0 * -0.0) = -0.0 + 0.0 = 0.0 with the round-to-nearest rounding mode.
    let minus_zero = (-0.0f32).to_bits();
    let zero = 0.0f32.to_bits();
    assert_eq!(
        mac_f32(VMLA_F32, minus_zero, minus_zero, minus_zero).0,
        zero
    );
    // The negated product flips it back: -0.0 - 0.0 = -0.0.
    assert_eq!(mac_f32(VMLS_F32, minus_zero, zero, zero).0, minus_zero);
    // The negated accumulator: -(-0.0) + 1.0 * 1.0 = 0.0 + 1.0 = 1.0.
    let one = 1.0f32.to_bits();
    assert_eq!(mac_f32(VNMLS_F32, minus_zero, one, one).0, one);
}

#[test]
fn test_a_denormal_factor_signals_input_denormal() {
    // 2^-139 * 1.0 + 0.0 with flush-to-zero off: the product is the denormal itself. Input
    // Denormal belongs to flushing only (A2.5), and rounding an exact denormal signals nothing.
    let tiny = 0x0000_0400_u32;
    let (result, flags) = mac_f32(VMLA_F32, 0.0f32.to_bits(), tiny, 1.0f32.to_bits());
    assert_eq!(result, tiny);
    assert_eq!(flags, 0);
    // With flush-to-zero the factor is read as zero: the answer is zero and only Input
    // Denormal is left.
    let mut processor = fpu();
    processor.fpscr = FZ;
    processor.set_sr(SingleReg::S0, 0.0f32.to_bits());
    processor.set_sr(SingleReg::S2, tiny);
    processor.set_sr(SingleReg::S3, 1.0f32.to_bits());
    processor.execute(&decode_32(VMLA_F32), 4);
    assert_eq!(processor.get_sr(SingleReg::S0), 0.0f32.to_bits());
    assert_eq!(processor.fpscr & CUMULATIVE_FLAGS, IDC);
}

#[test]
fn test_the_flags_accumulate_and_never_clear() {
    let mut processor = fpu();
    processor.fpscr = IOC | OFC;
    processor.set_sr(SingleReg::S0, 1.0f32.to_bits());
    processor.set_sr(SingleReg::S2, 1.5f32.to_bits());
    processor.set_sr(SingleReg::S3, 2.5f32.to_bits());
    processor.execute(&decode_32(VMLA_F32), 4);
    assert_eq!(f32::from_bits(processor.get_sr(SingleReg::S0)), 4.75);
    assert_eq!(processor.fpscr & CUMULATIVE_FLAGS, IOC | OFC);
}

#[test]
fn test_vmla_and_vnmls_f64_compute_in_double() {
    // 1.0 + 1.5 * 2.5 = 4.75 and -1.0 + 1.5 * 2.5 = 2.75.
    let (result, flags) = mac_f64(VMLA_F64, 1.0, 1.5, 2.5);
    assert_eq!(result, 4.75f64.to_bits());
    assert_eq!(flags, 0);
    let (result, _) = mac_f64(VNMLS_F64, 1.0, 1.5, 2.5);
    assert_eq!(result, 2.75f64.to_bits());
}

#[test]
fn test_vmls_and_vnmla_f64_compute_in_double() {
    // The same numbers through the other two operations: VMLS takes the product off the
    // accumulator (1.0 - 3.75 = -2.75), VNMLA negates accumulator and product and adds
    // (-1.0 - 3.75 = -4.75).
    let (result, flags) = mac_f64(VMLS_F64, 1.0, 1.5, 2.5);
    assert_eq!(result, (-2.75f64).to_bits());
    assert_eq!(flags, 0);
    let (result, _) = mac_f64(VNMLA_F64, 1.0, 1.5, 2.5);
    assert_eq!(result, (-4.75f64).to_bits());
}

#[test]
fn test_the_accumulator_may_be_a_factor() {
    // vmla.f32 s1, s1, s1 (llvm-mc: ee40 0aa0): s1 = 1.5 + 1.5 * 1.5 = 3.75.
    let mut processor = fpu();
    processor.set_sr(SingleReg::S1, 1.5f32.to_bits());
    processor.execute(&decode_32(0xee40_0aa0), 4);
    assert_eq!(f32::from_bits(processor.get_sr(SingleReg::S1)), 3.75);

    // vmla.f64 d7, d7, d5 (llvm-mc: ee07 7b05): d7 = 2.5 + 2.5 * 4.0 = 12.5.
    let mut processor = fpu();
    processor.set_dr_bits(DoubleReg::D7, 2.5f64.to_bits());
    processor.set_dr_bits(DoubleReg::D5, 4.0f64.to_bits());
    processor.execute(&decode_32(0xee07_7b05), 4);
    assert_eq!(processor.get_dr_bits(DoubleReg::D7), 12.5f64.to_bits());
}

#[test]
fn test_the_high_single_registers_work() {
    // vmla.f32 s31, s30, s29 (llvm-mc: ee4f fa2e): 1.0 + 2.0 * 3.0 = 7.0.
    let mut processor = fpu();
    processor.set_sr(SingleReg::S31, 1.0f32.to_bits());
    processor.set_sr(SingleReg::S30, 2.0f32.to_bits());
    processor.set_sr(SingleReg::S29, 3.0f32.to_bits());
    processor.execute(&decode_32(0xee4f_fa2e), 4);
    assert_eq!(processor.get_sr(SingleReg::S31), 7.0f32.to_bits());
}

#[test]
fn test_a_skipped_multiply_accumulate_changes_nothing() {
    // In an IT NE block with the Z flag set the instruction is skipped: no rounding and no
    // flags.
    let mut processor = fpu();
    processor.set_sr(SingleReg::S0, 1.0f32.to_bits());
    processor.psr.set_z_bit(true);
    processor.set_itstate(((Condition::NE.value() as u8) << 4) | 0b1000);
    let outcome = processor
        .exec_vmla_f32(&VAddSubParamsf32 {
            sd: SingleReg::S0,
            sn: SingleReg::S2,
            sm: SingleReg::S3,
        })
        .unwrap();
    assert!(matches!(outcome, ExecuteSuccess::NotTaken));
    assert_eq!(processor.get_sr(SingleReg::S0), 1.0f32.to_bits());
    assert_eq!(processor.fpscr & CUMULATIVE_FLAGS, 0);
}

#[test]
fn test_a_new_fp_instruction_faults_when_the_fpu_is_off() {
    let mut processor = Processor::new();
    processor.cpacr = 0;
    assert!(
        processor
            .exec_vmla_f32(&VAddSubParamsf32 {
                sd: SingleReg::S0,
                sn: SingleReg::S2,
                sm: SingleReg::S3,
            })
            .is_err()
    );
    assert!(
        processor
            .exec_vcvtb_f16_f32(VCVTParamsF16F32 {
                sd: SingleReg::S0,
                sm: SingleReg::S2,
            })
            .is_err()
    );
}

#[test]
fn test_a_skipped_new_fp_instruction_takes_no_coprocessor_fault() {
    // The pseudocode asks for the condition *first* and the coprocessor check inside it
    // (A7.7.250 Operation: if ConditionPassed() then EncodingSpecificOperations();
    // ExecuteFPCheck()), and the model copies that shape: every arm reads
    // "if self.condition_passed() { self.execute_fp_check()?; ... }". So with CPACR off
    // and an IT NE block whose Z is set, all sixteen arms must skip quietly; with the
    // same CPACR and a passing condition, all sixteen must fault.
    type Arm = fn(&mut Processor) -> ExecuteResult;
    let arms: [(&str, Arm); 16] = [
        ("vmla.f32", |p| {
            p.exec_vmla_f32(&VAddSubParamsf32 {
                sd: SingleReg::S0,
                sn: SingleReg::S2,
                sm: SingleReg::S3,
            })
        }),
        ("vmls.f32", |p| {
            p.exec_vmls_f32(&VAddSubParamsf32 {
                sd: SingleReg::S0,
                sn: SingleReg::S2,
                sm: SingleReg::S3,
            })
        }),
        ("vnmla.f32", |p| {
            p.exec_vnmla_f32(&VAddSubParamsf32 {
                sd: SingleReg::S0,
                sn: SingleReg::S2,
                sm: SingleReg::S3,
            })
        }),
        ("vnmls.f32", |p| {
            p.exec_vnmls_f32(&VAddSubParamsf32 {
                sd: SingleReg::S0,
                sn: SingleReg::S2,
                sm: SingleReg::S3,
            })
        }),
        ("vmla.f64", |p| {
            p.exec_vmla_f64(&VAddSubParamsf64 {
                dd: DoubleReg::D0,
                dn: DoubleReg::D2,
                dm: DoubleReg::D3,
            })
        }),
        ("vmls.f64", |p| {
            p.exec_vmls_f64(&VAddSubParamsf64 {
                dd: DoubleReg::D0,
                dn: DoubleReg::D2,
                dm: DoubleReg::D3,
            })
        }),
        ("vnmla.f64", |p| {
            p.exec_vnmla_f64(&VAddSubParamsf64 {
                dd: DoubleReg::D0,
                dn: DoubleReg::D2,
                dm: DoubleReg::D3,
            })
        }),
        ("vnmls.f64", |p| {
            p.exec_vnmls_f64(&VAddSubParamsf64 {
                dd: DoubleReg::D0,
                dn: DoubleReg::D2,
                dm: DoubleReg::D3,
            })
        }),
        ("vcvtb.f32.f16", |p| {
            p.exec_vcvtb_f32_f16(VCVTParamsF32F16 {
                sd: SingleReg::S0,
                sm: SingleReg::S2,
            })
        }),
        ("vcvtt.f32.f16", |p| {
            p.exec_vcvtt_f32_f16(VCVTParamsF32F16 {
                sd: SingleReg::S0,
                sm: SingleReg::S2,
            })
        }),
        ("vcvtb.f16.f32", |p| {
            p.exec_vcvtb_f16_f32(VCVTParamsF16F32 {
                sd: SingleReg::S0,
                sm: SingleReg::S2,
            })
        }),
        ("vcvtt.f16.f32", |p| {
            p.exec_vcvtt_f16_f32(VCVTParamsF16F32 {
                sd: SingleReg::S0,
                sm: SingleReg::S2,
            })
        }),
        ("vcvtb.f64.f16", |p| {
            p.exec_vcvtb_f64_f16(VCVTParamsF64F16 {
                dd: DoubleReg::D0,
                sm: SingleReg::S2,
            })
        }),
        ("vcvtt.f64.f16", |p| {
            p.exec_vcvtt_f64_f16(VCVTParamsF64F16 {
                dd: DoubleReg::D0,
                sm: SingleReg::S2,
            })
        }),
        ("vcvtb.f16.f64", |p| {
            p.exec_vcvtb_f16_f64(VCVTParamsF16F64 {
                sd: SingleReg::S0,
                dm: DoubleReg::D2,
            })
        }),
        ("vcvtt.f16.f64", |p| {
            p.exec_vcvtt_f16_f64(VCVTParamsF16F64 {
                sd: SingleReg::S0,
                dm: DoubleReg::D2,
            })
        }),
    ];
    for (name, arm) in arms {
        // The condition fails and CPACR is off: skip, no fault.
        let mut processor = Processor::new();
        processor.cpacr = 0;
        processor.psr.set_z_bit(true);
        processor.set_itstate(((Condition::NE.value() as u8) << 4) | 0b1000);
        let outcome = arm(&mut processor).unwrap();
        assert!(matches!(outcome, ExecuteSuccess::NotTaken), "{name}");
        // The condition passes and CPACR is still off: the coprocessor check runs.
        let mut processor = Processor::new();
        processor.cpacr = 0;
        assert!(arm(&mut processor).is_err(), "{name}");
    }
}

#[test]
fn test_a_skipped_new_fp_instruction_raises_no_flags_from_nan_inputs() {
    // The two earlier skip tests use operands that raise nothing; this one puts a
    // signalling NaN in every register the sixteen arms read, so a skipped arm that ran
    // its arithmetic anyway would leave Invalid Operation behind. The seed for d1 depends
    // on how the group reads it: 0x7f80_0001 is a signalling NaN as a 32-bit float and
    // 0x7c01 is one as a 16-bit float, and no single word is both.
    let singles = [
        VMLA_F32,
        VMLS_F32,
        VNMLA_F32,
        VNMLS_F32,
        VCVTB_F16_F32,
        VCVTT_F16_F32,
    ];
    let doubles = [
        VMLA_F64,
        VMLS_F64,
        VNMLA_F64,
        VNMLS_F64,
        VCVTB_F16_F64,
        VCVTT_F16_F64,
    ];
    let halves = [VCVTB_F32_F16, VCVTT_F32_F16, VCVTB_F64_F16, VCVTT_F64_F16];
    let d1_seeds = [0x7f80_0001_7f80_0001_u64, 0, 0x7c01_7c01_7c01_7c01];
    for (words, d1_seed) in [
        (singles.as_slice(), d1_seeds[0]),
        (doubles.as_slice(), d1_seeds[1]),
        (halves.as_slice(), d1_seeds[2]),
    ] {
        for &opcode in words {
            // A fresh processor per word: a conditional instruction advances the IT state
            // whether it is taken or not, so on a one-slot IT block only the first word
            // executed would be skipped.
            let mut processor = fpu();
            processor.set_dr_bits(DoubleReg::D0, 1.0f64.to_bits());
            processor.set_dr_bits(DoubleReg::D1, d1_seed);
            processor.set_dr_bits(DoubleReg::D2, 0x7ff0_0000_0000_0001);
            processor.set_dr_bits(DoubleReg::D3, 0x7ff0_0000_0000_0001);
            processor.psr.set_z_bit(true);
            processor.set_itstate(((Condition::NE.value() as u8) << 4) | 0b1000);
            processor.execute(&decode_32(opcode), 4);
            assert_eq!(processor.fpscr & CUMULATIVE_FLAGS, 0, "{opcode:#x}");
            assert_eq!(processor.get_dr_bits(DoubleReg::D0), 1.0f64.to_bits());
            assert_eq!(processor.get_dr_bits(DoubleReg::D1), d1_seed);
        }
    }
}

// The half-precision conversions. The binary16 values used below: 1.0 = 0x3c00, 1.5 = 0x3e00,
// 2.0 = 0x4000, 65504 (the largest number) = 0x7bff, infinity = 0x7c00, a quiet NaN = 0x7e00.

#[test]
fn test_vcvtb_and_vcvtt_read_each_half_of_the_source() {
    // s2 holds 2.0 in its top half (0x4000 = binary16 2.0) and 1.0 in its bottom half
    // (0x3c00).
    assert_eq!(
        convert(VCVTB_F32_F16, 0x4000_3c00, 0, 0).0,
        1.0f32.to_bits()
    );
    assert_eq!(
        convert(VCVTT_F32_F16, 0x4000_3c00, 0, 0).0,
        2.0f32.to_bits()
    );
    // binary16 65504 (0x7bff) is 65504.0 in every wider format.
    assert_eq!(
        convert(VCVTB_F32_F16, 0x0000_7bff, 0, 0).0,
        65504.0f32.to_bits()
    );
    // The double that vcvtt.f64.f16 (0xeeb2 0bc1) writes to d0 from the top half of s2.
    let mut processor = fpu();
    processor.set_sr(SingleReg::S2, 0x7bff_0000);
    processor.execute(&decode_32(VCVTT_F64_F16), 4);
    assert_eq!(
        processor.get_dr_bits(double_register(0)),
        65504.0f64.to_bits()
    );
}

#[test]
fn test_a_half_conversion_from_f16_carries_the_right_flags() {
    // binary16's smallest denormal, 2^-24, is the exact single 0x33800000. FPUnpack (A2.5)
    // reads a half-precision denormal as it stands even with flush-to-zero, so nothing signals.
    let (result, flags) = convert(VCVTB_F32_F16, 0x0000_0001, 0, 0);
    assert_eq!(result, 0x3380_0000);
    assert_eq!(flags, 0);
    let (result, flags) = convert(VCVTB_F32_F16, 0x0000_0001, 0, FZ);
    assert_eq!(result, 0x3380_0000);
    assert_eq!(flags, 0);
    // A quiet NaN propagates and signals nothing. Its ten fraction bits ride along in the
    // top of the single's fraction (A2.5 FPHalfToSingle: sign : 11111111 1 : operand<8:0> :
    // zeros).
    let (result, flags) = convert(VCVTB_F32_F16, 0x0000_7e12, 0, 0);
    assert_eq!(result, 0x7fc2_4000);
    assert_eq!(flags, 0);
    // A signalling NaN is quieted and signals Invalid Operation.
    let (result, flags) = convert(VCVTB_F32_F16, 0x0000_7c01, 0, 0);
    assert_eq!(result, 0x7fc0_2000);
    assert_eq!(flags, IOC);
    // With the default-NaN mode, any NaN gives the default NaN.
    let (result, _) = convert(VCVTB_F32_F16, 0x0000_7e12, 0, DN);
    assert_eq!(result, 0x7fc0_0000);
    // An infinity of the half is an infinity of the single, with no flag (A2.5
    // FPUnpack/FPRound pass FPType_Infinity through).
    let (result, flags) = convert(VCVTB_F32_F16, 0x0000_7c00, 0, 0);
    assert_eq!(result, 0x7f80_0000);
    assert_eq!(flags, 0);
    // The negative of the smallest binary16 denormal, -2^-24, with flush-to-zero set: FZ
    // looks only at the result (A2.5 FPUnpack, the flush line for N = 32 and 64), and the
    // result is an ordinary single, so the sign and the value both survive.
    let (result, flags) = convert(VCVTB_F32_F16, 0x0000_8001, 0, FZ);
    assert_eq!(result, 0xb380_0000);
    assert_eq!(flags, 0);
}

#[test]
fn test_a_half_conversion_from_f16_to_double_carries_the_right_payload() {
    // The f64 twins of the f32 payload vectors: A2.5 FPHalfToDouble puts the nine payload
    // bits operand<8:0> at the top of the double fraction under the forced quiet bit
    // (sign : 11111111111 : 1 : operand<8:0> : zeros(42)), and the single source test at
    // the same payload (0x7e12) pins the f32 shape.
    let mut processor = fpu();
    processor.set_sr(SingleReg::S2, 0x0000_7e12);
    processor.execute(&decode_32(VCVTB_F64_F16), 4);
    assert_eq!(processor.get_dr_bits(DoubleReg::D0), 0x7ff8_4800_0000_0000);
    assert_eq!(processor.fpscr & CUMULATIVE_FLAGS, 0);
    // A signalling half is quieted and signals: same bits, plus Invalid Operation.
    let mut processor = fpu();
    processor.set_sr(SingleReg::S2, 0x0000_7c01);
    processor.execute(&decode_32(VCVTB_F64_F16), 4);
    assert_eq!(processor.get_dr_bits(DoubleReg::D0), 0x7ff8_0400_0000_0000);
    assert_eq!(processor.fpscr & CUMULATIVE_FLAGS, IOC);
    // vcvtt.f64.f16 reads the top half of the source: -Infinity goes to -Infinity, no flag.
    let mut processor = fpu();
    processor.set_sr(SingleReg::S2, 0xfc00_0000);
    processor.execute(&decode_32(VCVTT_F64_F16), 4);
    assert_eq!(
        processor.get_dr_bits(DoubleReg::D0),
        f64::NEG_INFINITY.to_bits()
    );
    assert_eq!(processor.fpscr & CUMULATIVE_FLAGS, 0);
}

#[test]
fn test_vcvtb_and_vcvtt_write_one_half_and_keep_the_other() {
    // The half of the destination that was not converted into stays as it was.
    let (result, flags) = convert(VCVTB_F16_F32, 1.0f32.to_bits(), 0xbeef_0000, 0);
    assert_eq!(result, 0xbeef_3c00);
    assert_eq!(flags, 0);
    let (result, _) = convert(VCVTT_F16_F32, 1.0f32.to_bits(), 0x0000_5678, 0);
    assert_eq!(result, 0x3c00_5678);
    // A destination that is also the source must be read before it is written:
    // vcvtb.f16.f32 s1, s1 (llvm-mc: eef3 0a60) with s1 = 2.0 = 0x40000000 keeps its top half
    // and gains the converted half 0x4000 at the bottom.
    let mut processor = fpu();
    processor.set_sr(SingleReg::S1, 2.0f32.to_bits());
    processor.execute(&decode_32(0xeef3_0a60), 4);
    assert_eq!(processor.get_sr(SingleReg::S1), 0x4000_4000);
}

#[test]
fn test_the_rounding_to_half_precision() {
    // 1.5 + 2^-11 (0x3fc01000) is a quarter of a binary16 ulp (2^-9) above 1.5: nearest takes
    // 1.5 = 0x3e00, and the rounding is inexact.
    let (result, flags) = convert(VCVTB_F16_F32, 0x3fc0_1000, 0, 0);
    assert_eq!(result & 0xffff, 0x3e00);
    assert_eq!(flags, IXC);
    // 1 + 2^-11 (0x3f801000) is exactly halfway between 1.0 (0x3c00, even fraction) and
    // 0x3c01: the nearest-even rule takes 0x3c00.
    let (result, flags) = convert(VCVTB_F16_F32, 0x3f80_1000, 0, 0);
    assert_eq!(result & 0xffff, 0x3c00);
    assert_eq!(flags, IXC);
    // 1.5029296875 (0x3fc06000) is 1.5 + 3 * 2^-10, and 2^-10 is a binary16 unit in the last
    // place near 1.5: the value is the exact half 0x3e03, with no rounding at all.
    let (result, flags) = convert(VCVTB_F16_F32, 0x3fc0_6000, 0, 0);
    assert_eq!(result & 0xffff, 0x3e03);
    assert_eq!(flags, 0);
    // An exact binary16 value rounds with no flag at all.
    let (result, flags) = convert(VCVTB_F16_F32, 1.5f32.to_bits(), 0, 0);
    assert_eq!(result & 0xffff, 0x3e00);
    assert_eq!(flags, 0);
    // The directed rounding modes follow their direction: towards plus infinity takes the
    // upper neighbour of 1.5 + 2^-11.
    let (result, _) = convert(VCVTB_F16_F32, 0x3fc0_1000, 0, RP);
    assert_eq!(result & 0xffff, 0x3e01);
}

#[test]
fn test_the_ties_to_half_precision_in_the_two_remaining_rounding_modes() {
    // -1 - 2^-11 (0xbf80_1000) is exactly halfway between -1.0 (0xbc00, even fraction) and
    // 0xbc01, so the two remaining modes part: towards zero keeps 0xbc00, towards minus
    // infinity crosses to 0xbc01. Both roundings are inexact.
    let (result, flags) = convert(VCVTB_F16_F32, 0xbf80_1000, 0, RZ);
    assert_eq!(result & 0xffff, 0xbc00);
    assert_eq!(flags, IXC);
    let (result, flags) = convert(VCVTB_F16_F32, 0xbf80_1000, 0, RM);
    assert_eq!(result & 0xffff, 0xbc01);
    assert_eq!(flags, IXC);
    // A positive tie goes to the lower magnitude in both modes, and the inexact non-tie
    // 1.5 + 2^-11 keeps 1.5 towards zero, where plus infinity took 0x3e01.
    for fpscr in [RZ, RM] {
        let (result, flags) = convert(VCVTB_F16_F32, 0x3f80_1000, 0, fpscr);
        assert_eq!(result & 0xffff, 0x3c00);
        assert_eq!(flags, IXC);
    }
    let (result, flags) = convert(VCVTB_F16_F32, 0x3fc0_1000, 0, RZ);
    assert_eq!(result & 0xffff, 0x3e00);
    assert_eq!(flags, IXC);
}

#[test]
fn test_overflow_to_half_precision() {
    // 70000.0 is past 65504, binary16's largest number: infinity with Overflow and Inexact.
    let (result, flags) = convert(VCVTB_F16_F32, 70000.0f32.to_bits(), 0, 0);
    assert_eq!(result & 0xffff, 0x7c00);
    assert_eq!(flags, OFC | IXC);
    // 65520.0 is exactly halfway between 65504 and the next value (2^16, which binary16
    // cannot hold): the rounding overflows, so infinity with Overflow and Inexact.
    let (result, flags) = convert(VCVTB_F16_F32, 65520.0f32.to_bits(), 0, 0);
    assert_eq!(result & 0xffff, 0x7c00);
    assert_eq!(flags, OFC | IXC);
    // 65504.0 is representable exactly, from double as well as from single.
    let (result, flags) = convert(VCVTB_F16_F32, 65504.0f32.to_bits(), 0, 0);
    assert_eq!(result & 0xffff, 0x7bff);
    assert_eq!(flags, 0);
    let (result, flags) = convert_double(VCVTB_F16_F64, 65504.0f64.to_bits(), 0, 0);
    assert_eq!(result & 0xffff, 0x7bff);
    assert_eq!(flags, 0);
    // The infinity of the source is the infinity of the half, with no flag.
    let (result, flags) = convert(VCVTB_F16_F32, f32::INFINITY.to_bits(), 0, 0);
    assert_eq!(result & 0xffff, 0x7c00);
    assert_eq!(flags, 0);
}

#[test]
fn test_underflow_to_half_precision() {
    // 2^-15 (0x38000000) is the exact binary16 denormal 0x0200: a tininess that is exact
    // signals nothing.
    let (result, flags) = convert(VCVTB_F16_F32, 0x3800_0000, 0, 0);
    assert_eq!(result & 0xffff, 0x0200);
    assert_eq!(flags, 0);
    // 3 * 2^-25 (0x33c00000) is halfway between the denormals 0x0001 and 0x0002: even takes
    // 0x0002, and an inexact tininess signals Underflow and Inexact.
    let (result, flags) = convert(VCVTB_F16_F32, 0x33c0_0000, 0, 0);
    assert_eq!(result & 0xffff, 0x0002);
    assert_eq!(flags, UFC | IXC);
    // 1e-5 as a double is 167.77 binary16 denormal units, so 168 = 0x00a8, inexact and tiny.
    let (result, flags) = convert_double(VCVTB_F16_F64, 1e-5f64.to_bits(), 0, 0);
    assert_eq!(result & 0xffff, 0x00a8);
    assert_eq!(flags, UFC | IXC);
    // Flush-to-zero does not touch a half-precision result (A2.5 FPRound): the same denormal
    // and the same flags as without it.
    let mut processor = fpu();
    processor.fpscr = FZ;
    processor.set_sr(SingleReg::S2, 0x33c0_0000);
    processor.execute(&decode_32(VCVTB_F16_F32), 4);
    assert_eq!(processor.get_sr(SingleReg::S0) & 0xffff, 0x0002);
    assert_eq!(processor.fpscr & CUMULATIVE_FLAGS, UFC | IXC);
}

#[test]
fn test_overflow_to_half_precision_in_the_directed_modes() {
    // A2.5 FPRound: the overflow branch depends on the direction (overflow_to_inf) and
    // always signals Overflow and Inexact. 70000.0 overflows a half in every mode:
    // towards plus infinity takes +Infinity and towards zero the largest normal 0x7bff;
    // for the negative number it is the other way round.
    let (result, flags) = convert(VCVTB_F16_F32, 70000.0f32.to_bits(), 0, RP);
    assert_eq!(result & 0xffff, 0x7c00);
    assert_eq!(flags, OFC | IXC);
    let (result, flags) = convert(VCVTB_F16_F32, 70000.0f32.to_bits(), 0, RZ);
    assert_eq!(result & 0xffff, 0x7bff);
    assert_eq!(flags, OFC | IXC);
    let neg = (-70000.0f32).to_bits();
    let (result, flags) = convert(VCVTB_F16_F32, neg, 0, RZ);
    assert_eq!(result & 0xffff, 0xfbff);
    assert_eq!(flags, OFC | IXC);
    let (result, flags) = convert(VCVTB_F16_F32, neg, 0, RM);
    assert_eq!(result & 0xffff, 0xfc00);
    assert_eq!(flags, OFC | IXC);
    // The same from a double source, where the value overflows the half by even more:
    // the mode decides infinity or the largest normal, and the flags do not change.
    let (result, flags) = convert_double(VCVTB_F16_F64, 70000.0f64.to_bits(), 0, RZ);
    assert_eq!(result & 0xffff, 0x7bff);
    assert_eq!(flags, OFC | IXC);
    let (result, flags) = convert_double(VCVTB_F16_F64, (-70000.0f64).to_bits(), 0, RM);
    assert_eq!(result & 0xffff, 0xfc00);
    assert_eq!(flags, OFC | IXC);
}

#[test]
fn test_nan_and_sign_in_the_conversions_to_half() {
    // A quiet NaN keeps its top fraction bits: 0x7fc01234 has fraction 0x401234, and
    // 0x401234 >> 13 = 0x200, which is already the quiet bit of a binary16 NaN: 0x7e00.
    let (result, flags) = convert(VCVTB_F16_F32, 0x7fc0_1234, 0, 0);
    assert_eq!(result & 0xffff, 0x7e00);
    assert_eq!(flags, 0);
    // A signalling NaN is quieted and signals Invalid Operation.
    let (result, flags) = convert(VCVTB_F16_F32, 0x7f80_0001, 0, 0);
    assert_eq!(result & 0xffff, 0x7e00);
    assert_eq!(flags, IOC);
    // A negative sign is kept in both directions, zero included.
    let (result, _) = convert(VCVTB_F16_F32, (-0.0f32).to_bits(), 0, 0);
    assert_eq!(result & 0xffff, 0x8000);
    let (result, _) = convert(VCVTB_F32_F16, 0x0000_8000, 0, 0);
    assert_eq!(result, (-0.0f32).to_bits());
    // With the default-NaN mode, a NaN in gives the default binary16 NaN.
    let (result, _) = convert(VCVTB_F16_F32, 0x7fc0_1234, 0, DN);
    assert_eq!(result & 0xffff, 0x7e00);
    // From a double the same rules reach further: A2.5 FPSingleToHalf and its double twin
    // keep sign, the forced quiet bit and the top nine payload bits (operand<50:42>).
    // 0x7ff8_4800_0000_0000 has fraction bits 50..42 = 0x012, so the half is 0x7e12.
    let (result, flags) = convert_double(VCVTB_F16_F64, 0x7ff8_4800_0000_0000, 0, 0);
    assert_eq!(result & 0xffff, 0x7e12);
    assert_eq!(flags, 0);
    // The same value with the source's quiet bit clear is a signalling NaN: quieted and
    // signalling, the bits otherwise unchanged; with DN the default binary16 NaN.
    let (result, flags) = convert_double(VCVTB_F16_F64, 0x7ff0_4800_0000_0000, 0, 0);
    assert_eq!(result & 0xffff, 0x7e12);
    assert_eq!(flags, IOC);
    let (result, flags) = convert_double(VCVTB_F16_F64, 0x7ff0_4800_0000_0000, 0, DN);
    assert_eq!(result & 0xffff, 0x7e00);
    assert_eq!(flags, IOC);
}

#[test]
fn test_ahp_seeded_conversions_stay_on_the_ieee_path() {
    // FPSCR bit 26 (AHP in the FPSCR description, "AH" in the A2.5 pseudocode) asks for the
    // alternative half-precision interpretation: exponent 0x1F halves are ordinary numbers,
    // an overflowed half is sign:Ones(15) with Invalid Operation, and NaN or Infinity going
    // into a half become zero or sign:Ones(15) with Invalid Operation. The model reads the
    // bit nowhere: these pins record that documented limitation (IEEE only whatever bit 26
    // says), not what the architecture gives with AHP = 1. Each expectation is what the
    // same vector gives with the bit clear (pinned by the neighbouring tests).
    // Overflow of 70000.0: the model gives Infinity with Overflow and Inexact; the A2.5
    // FPRound branch for N = 16 and AHP = 1 would give 0x7fff with Invalid Operation only.
    let (result, flags) = convert(VCVTB_F16_F32, 70000.0f32.to_bits(), 0, AHP);
    assert_eq!(result & 0xffff, 0x7c00);
    assert_eq!(flags, OFC | IXC);
    // 0x7c00 read as a half source: the model sees +Infinity; with AHP = 1, A2.5 FPUnpack
    // would read the normal number 65536.0 = 0x4780_0000.
    let (result, flags) = convert(VCVTB_F32_F16, 0x0000_7c00, 0, AHP);
    assert_eq!(result, 0x7f80_0000);
    assert_eq!(flags, 0);
    // 0x7c01: the model sees a signalling NaN, quieted with its payload, and signals; with
    // AHP = 1 the same half is the normal number 65600.0 = 0x4780_2000 with no flags.
    let (result, flags) = convert(VCVTB_F32_F16, 0x0000_7c01, 0, AHP);
    assert_eq!(result, 0x7fc0_2000);
    assert_eq!(flags, IOC);
    // A double signalling NaN going into a half with AHP and DN set: the model takes the DN
    // branch, the default binary16 NaN, signalling; the AHP branch of A2.5 FPDoubleToHalf
    // would take it before DN and give positive zero, also signalling.
    let (result, flags) = convert_double(VCVTB_F16_F64, 0x7ff0_0000_0000_0001, 0, AHP | DN);
    assert_eq!(result & 0xffff, 0x7e00);
    assert_eq!(flags, IOC);
}

#[test]
fn test_f16_conversions_with_double_read_and_write_the_right_halves() {
    // vcvtb.f64.f16 d0, s2 (0xeeb2 0b41): the bottom half of s2 = 0x1234_7bff is 0x7bff,
    // which is 65504.0.
    let mut processor = fpu();
    processor.set_sr(SingleReg::S2, 0x1234_7bff);
    processor.execute(&decode_32(VCVTB_F64_F16), 4);
    assert_eq!(processor.get_dr_bits(DoubleReg::D0), 65504.0f64.to_bits());

    // vcvtt.f64.f16 d0, s2 (0xeeb2 0bc1): the top half 0x1234: sign 0, exponent field 4,
    // fraction 0x234, so (0x400 + 0x234) * 2^(4-15-10) = 1.55078125 * 2^-11.
    let mut processor = fpu();
    processor.set_sr(SingleReg::S2, 0x1234_0000);
    processor.execute(&decode_32(VCVTT_F64_F16), 4);
    let value = f64::from(0x400u32 + 0x234) * 2f64.powi(-21);
    assert_eq!(processor.get_dr_bits(DoubleReg::D0), value.to_bits());

    // vcvtt.f16.f64 s31, d14 (0xeef3 fbce): -2.0 goes into the top half of s31 as 0xc000 and
    // the bottom half stays. The source is d14, not d15: S31 is the top word of D15, and
    // setting it would overwrite the very register being converted.
    let mut processor = fpu();
    processor.set_dr_bits(DoubleReg::D14, (-2.0f64).to_bits());
    processor.set_sr(SingleReg::S31, 0x0001_abcd);
    processor.execute(&decode_32(0xeef3_fbce), 4);
    assert_eq!(processor.get_sr(SingleReg::S31), 0xc000_abcd);
}

#[test]
fn test_a_half_conversion_reads_a_wide_source_that_shares_a_register_with_the_destination() {
    // The pseudocode unpacks the source before the destination write (A7.7.231:
    // D[d] = FPHalfToDouble(S<m>..., TRUE) and S<d><half> = FPDoubleToHalf(D<m>, TRUE)).
    // Where the two registers overlap, the original source bits must be what converts: an
    // implementation that clears the destination first would convert the wrong number.
    // vcvtb.f64.f16 d0, s0 (llvm-mc: eeb2 0b40): the source half lives in the bottom word
    // of the destination double. s0 = 0x1234_7bff carries 65504 (0x7bff) in its bottom half.
    let mut processor = fpu();
    processor.set_sr(SingleReg::S0, 0x1234_7bff);
    processor.execute(&decode_32(0xeeb2_0b40), 4);
    assert_eq!(
        processor.get_dr_bits(DoubleReg::D0),
        65504.0f64.to_bits(),
        "clearing the destination before unpacking s0 would convert 0x0000 into +0"
    );
    // The write does land: d0 covers s0 and s1, and they hold the double's two words.
    assert_eq!(processor.get_sr(SingleReg::S0), 0x0000_0000);
    assert_eq!(processor.get_sr(SingleReg::S1), 0x40ef_fc00);
    // vcvtt.f16.f64 s31, d15 (llvm-mc: eef3 fbcf): the destination half is the top word of
    // the source double. d15 = 0xbff0_0100_0000_0000 is -(1 + 2^-12): the nearest half is
    // -1.0 (0xbc00, inexact) written into the top half of s31, so the low half of s31
    // (0x0100, the tail of the exponent field) and the s30 word of d15 must survive.
    let mut processor = fpu();
    processor.set_dr_bits(DoubleReg::D15, 0xbff0_0100_0000_0000);
    processor.execute(&decode_32(0xeef3_fbcf), 4);
    assert_eq!(processor.get_sr(SingleReg::S31), 0xbc00_0100);
    assert_eq!(processor.get_sr(SingleReg::S30), 0);
    assert_eq!(processor.fpscr & CUMULATIVE_FLAGS, IXC);
    // vcvtb.f16.f64 s0, d0 (llvm-mc: eeb3 0b40): -2.0 into the bottom half of s0, the top
    // half of s0 and the untouched word s1 of d0 hold the double that was read first.
    let mut processor = fpu();
    processor.set_dr_bits(DoubleReg::D0, (-2.0f64).to_bits());
    processor.execute(&decode_32(0xeeb3_0b40), 4);
    assert_eq!(processor.get_sr(SingleReg::S0), 0x0000_c000);
    assert_eq!(processor.get_sr(SingleReg::S1), 0xc000_0000);
    assert_eq!(processor.fpscr & CUMULATIVE_FLAGS, 0);
}

#[test]
fn test_a_skipped_half_conversion_changes_nothing() {
    let mut processor = fpu();
    processor.set_sr(SingleReg::S0, 0xbeef_0000);
    processor.psr.set_z_bit(true);
    processor.set_itstate(((Condition::NE.value() as u8) << 4) | 0b1000);
    let outcome = processor
        .exec_vcvtb_f16_f32(VCVTParamsF16F32 {
            sd: SingleReg::S0,
            sm: SingleReg::S2,
        })
        .unwrap();
    assert!(matches!(outcome, ExecuteSuccess::NotTaken));
    assert_eq!(processor.get_sr(SingleReg::S0), 0xbeef_0000);
}

// FLDMX and FSTMX: the VLDM and VSTM of doubles with an odd word count. The address moves by
// the whole word count while one fewer double is moved (D2.3.1: the register count drops the
// low bit and the offset keeps it).

#[test]
fn test_fldmx_loads_eight_doubles_and_moves_by_seventeen_words() {
    let mut processor = fpu();
    let base = 0x2000_0100;
    for word in 0..17u32 {
        processor
            .write32(base + word * 4, 0xf000_0000 + word)
            .unwrap();
    }
    processor.set_r(Reg::R0, base);
    // fldmx r0!, {d0-d7} = vldmia r0!, {d0-d7} with imm8 = 0x11 (llvm-mc: ecb0 0b10 with the
    // low bit of imm8 set).
    processor.execute(&decode_32(0xecb0_0b11), 4);
    // R0 advanced by 17 words while D7, the last double, came from words 14 and 15.
    assert_eq!(processor.get_r(Reg::R0), base + 68);
    assert_eq!(
        processor.get_dr_bits(double_register(7)),
        (u64::from(0xf000_000fu32) << 32) | u64::from(0xf000_000eu32)
    );
    assert_eq!(
        processor.get_dr_bits(double_register(0)),
        (u64::from(0xf000_0001u32) << 32) | u64::from(0xf000_0000u32)
    );
    // The odd word at +64 was not read, and no double was made from it: d8 is untouched.
    assert_eq!(processor.get_dr_bits(double_register(8)), 0);
}

#[test]
fn test_fstmx_stores_eight_doubles_and_moves_by_seventeen_words() {
    let mut processor = fpu();
    let base = 0x2000_0100;
    let sentinel = 0xdead_beef_u32;
    for word in 0..17u32 {
        processor.write32(base + word * 4, 0).unwrap();
    }
    processor.write32(base + 64, sentinel).unwrap();
    let values: [f64; 8] = [1.5, -2.5, 3.5, 4.5, 5.5, -6.5, 7.5, 8.5];
    for (number, value) in values.into_iter().enumerate() {
        processor.set_dr_bits(double_register(number as u8), value.to_bits());
    }
    processor.set_r(Reg::R0, base);
    processor.execute(&decode_32(0xeca0_0b11), 4);
    assert_eq!(processor.get_r(Reg::R0), base + 68);
    assert_eq!(processor.read32(base).unwrap(), values[0].to_bits() as u32);
    assert_eq!(
        processor.read32(base + 60).unwrap(),
        (values[7].to_bits() >> 32) as u32
    );
    // The word that the store count does not cover is untouched.
    assert_eq!(processor.read32(base + 64).unwrap(), sentinel);
}

#[test]
fn test_fldmx_decrement_before() {
    let mut processor = fpu();
    let base = 0x2000_0200;
    processor.set_r(Reg::R0, base);
    // vldmdb r0!, {d0-d7} with imm8 = 0x11 (ed30 0b11): the range starts at base - 68.
    processor.write32(base - 68, 0xa5a5_0000).unwrap();
    processor.write32(base - 64, 0xa5a5_0001).unwrap();
    // The walk stops after 16 words, so d7 is the pair at base - 12 and the highest word
    // of the range, at base - 4, is not read: a sentinel there must survive the load.
    processor.write32(base - 12, 0xa5a5_000e).unwrap();
    processor.write32(base - 8, 0xa5a5_000f).unwrap();
    processor.write32(base - 4, 0xdead_beef).unwrap();
    processor.execute(&decode_32(0xed30_0b11), 4);
    assert_eq!(processor.get_r(Reg::R0), base - 68);
    assert_eq!(
        processor.get_dr_bits(double_register(0)),
        (u64::from(0xa5a5_0001u32) << 32) | u64::from(0xa5a5_0000u32)
    );
    assert_eq!(
        processor.get_dr_bits(double_register(7)),
        (u64::from(0xa5a5_000fu32) << 32) | u64::from(0xa5a5_000eu32)
    );
    assert_eq!(processor.read32(base - 4).unwrap(), 0xdead_beef);
}

#[test]
fn test_fstmx_decrement_before() {
    // The store twin: vstmdb r0!, {d0-d7} with imm8 = 0x11 (ed20 0b11, llvm-mc: the fstmdbx
    // of the same word). The range is the same 17 words below the base, d0 to d7 fill the
    // low 16 of them, and the highest word of the range is not written.
    let mut processor = fpu();
    let base = 0x2000_0200;
    for word in 0..17u32 {
        processor.write32(base - 68 + word * 4, 0).unwrap();
    }
    processor.write32(base - 4, 0xdead_beef).unwrap();
    let values: [f64; 8] = [1.5, -2.5, 3.5, 4.5, 5.5, -6.5, 7.5, 8.5];
    for (number, value) in values.into_iter().enumerate() {
        processor.set_dr_bits(double_register(number as u8), value.to_bits());
    }
    processor.set_r(Reg::R0, base);
    processor.execute(&decode_32(0xed20_0b11), 4);
    assert_eq!(processor.get_r(Reg::R0), base - 68);
    assert_eq!(
        processor.read32(base - 68).unwrap(),
        values[0].to_bits() as u32
    );
    assert_eq!(
        processor.read32(base - 8).unwrap(),
        (values[7].to_bits() >> 32) as u32
    );
    assert_eq!(processor.read32(base - 4).unwrap(), 0xdead_beef);
}

#[test]
fn test_a_skipped_fstmx_changes_nothing() {
    let mut processor = fpu();
    let base = 0x2000_0100;
    processor.write32(base, 0).unwrap();
    processor.set_r(Reg::R0, base);
    processor.set_dr_bits(double_register(0), 1.5f64.to_bits());
    processor.psr.set_z_bit(true);
    processor.set_itstate(((Condition::NE.value() as u8) << 4) | 0b1000);
    let mut list = EnumSet::new();
    for number in 0..8u8 {
        list.insert(double_register(number));
    }
    let outcome = processor
        .exec_vstm_t1(&VStoreMultipleParams64 {
            mode: AddressingMode::IncrementAfter,
            rn: Reg::R0,
            write_back: true,
            list,
            imm32: 68,
        })
        .unwrap();
    assert!(matches!(outcome, ExecuteSuccess::NotTaken));
    assert_eq!(processor.get_r(Reg::R0), base);
    assert_eq!(processor.read32(base).unwrap(), 0);
}
