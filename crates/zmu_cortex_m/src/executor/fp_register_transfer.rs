use crate::core::instruction::{
    VMRSTarget, VMovCr2DpParams, VMovCr2Sp2Params, VMovCrScalarParams, VMovCrSpParams,
    VMovImmParams32, VMovImmParams64, VMovRegParamsf32, VMovRegParamsf64,
};

use crate::Processor;
use crate::core::fpregister::Fpscr;
use crate::executor::ExecuteSuccess;
use crate::executor::ExecutorHelper;
use crate::executor::fp_generic::FloatingPointChecks;

use super::ExecuteResult;
use crate::core::register::{Apsr, BaseReg, ExtensionRegOperations, Reg};

pub trait IsaFloatingPointRegisterTransfer {
    fn exec_vmov_cr_sp(&mut self, params: &VMovCrSpParams) -> ExecuteResult;
    fn exec_vmov_cr2_dp(&mut self, params: &VMovCr2DpParams) -> ExecuteResult;
    fn exec_vmov_cr2_sp2(&mut self, params: VMovCr2Sp2Params) -> ExecuteResult;
    fn exec_vmov_cr_scalar(&mut self, params: VMovCrScalarParams) -> ExecuteResult;
    fn exec_vmov_scalar_cr(&mut self, params: VMovCrScalarParams) -> ExecuteResult;

    fn exec_vmov_reg_f32(&mut self, params: VMovRegParamsf32) -> ExecuteResult;
    fn exec_vmov_reg_f64(&mut self, params: VMovRegParamsf64) -> ExecuteResult;

    fn exec_vmov_imm_32(&mut self, params: VMovImmParams32) -> ExecuteResult;
    fn exec_vmov_imm_64(&mut self, params: VMovImmParams64) -> ExecuteResult;

    fn exec_vmrs(&mut self, params: VMRSTarget) -> ExecuteResult;
    fn exec_vmsr(&mut self, rt: Reg) -> ExecuteResult;
}

impl IsaFloatingPointRegisterTransfer for Processor {
    fn exec_vmov_cr_sp(&mut self, params: &VMovCrSpParams) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            if params.to_arm_register {
                let value = self.get_sr(params.sn);
                self.set_r(params.rt, value);
            } else {
                self.set_sr(params.sn, self.get_r(params.rt));
            }
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vmov_cr2_dp(&mut self, params: &VMovCr2DpParams) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            if params.to_arm_registers {
                let (low, high) = self.get_dr(params.dm);
                self.set_r(params.rt, low);
                self.set_r(params.rt2, high);
            } else {
                let low = self.get_r(params.rt);
                let high = self.get_r(params.rt2);
                self.set_dr(params.dm, low, high);
            }
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vmov_cr2_sp2(&mut self, params: VMovCr2Sp2Params) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            if params.to_arm_registers {
                let (low, high) = (self.get_sr(params.sm), self.get_sr(params.sm1));
                self.set_r(params.rt, low);
                self.set_r(params.rt2, high);
            } else {
                self.set_sr(params.sm, self.get_r(params.rt));
                self.set_sr(params.sm1, self.get_r(params.rt2));
            }
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    // The 32-bit scalar forms: bit x picks the low or the high word of the double register.
    fn exec_vmov_cr_scalar(&mut self, params: VMovCrScalarParams) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let (mut low, mut high) = self.get_dr(params.dd);
            let value = self.get_r(params.rt);
            if params.x {
                high = value;
            } else {
                low = value;
            }
            self.set_dr(params.dd, low, high);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vmov_scalar_cr(&mut self, params: VMovCrScalarParams) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let (low, high) = self.get_dr(params.dd);
            self.set_r(params.rt, if params.x { high } else { low });
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vmov_reg_f32(&mut self, params: VMovRegParamsf32) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let value = self.get_sr(params.sm);
            self.set_sr(params.sd, value);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vmov_reg_f64(&mut self, params: VMovRegParamsf64) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let (low, high) = self.get_dr(params.dm);
            self.set_dr(params.dd, low, high);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vmov_imm_32(&mut self, params: VMovImmParams32) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            self.set_sr(params.sd, params.imm32);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vmov_imm_64(&mut self, params: VMovImmParams64) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let (low, high) = (params.imm64 as u32, (params.imm64 >> 32) as u32);
            self.set_dr(params.dd, low, high);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vmrs(&mut self, params: VMRSTarget) -> ExecuteResult {
        if self.condition_passed() {
            //EncodingSpecificOperations();
            self.execute_fp_check()?;
            //SerializeVFP();
            //VFPExcBarrier();

            match params {
                VMRSTarget::APSRNZCV => {
                    let n = self.fpscr.get_n();
                    let z = self.fpscr.get_z();
                    let c = self.fpscr.get_c();
                    let v = self.fpscr.get_v();

                    self.psr.set_n_bit(n);
                    self.psr.set_z_bit(z);
                    self.psr.set_c(c);
                    self.psr.set_v(v);
                }
                VMRSTarget::Register(reg) => {
                    self.set_r(reg, self.fpscr);
                }
            }
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vmsr(&mut self, rt: Reg) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            self.write_fpscr(self.get_r(rt));
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::fpregister::{FPSCRRounding, Fpscr};
    use crate::core::instruction::{
        VAddSubParamsf32, VCVTParams, VMRSTarget, VMovCr2DpParams, VMovCr2Sp2Params,
        VMovCrSpParams, VMovImmParams32, VMovImmParams64, VMovRegParamsf32, VMovRegParamsf64,
    };
    use crate::core::register::{Apsr, DoubleReg, ExtensionReg, Reg, SingleReg};
    use crate::decoder::decode_32;
    use crate::executor::fp_data_processing::IsaFloatingPointDataProcessing;
    use crate::peripheral::scb::FPSCR_WRITE_MASK;

    fn fp_test_processor() -> Processor {
        let mut processor = Processor::new();
        processor.cpacr = 0x00f0_0000;
        processor
    }

    #[test]
    fn test_vmov_cr_sp_moves_between_core_and_single_registers() {
        let mut processor = fp_test_processor();
        processor.set_r(Reg::R2, 0x1234_5678);

        processor
            .exec_vmov_cr_sp(&VMovCrSpParams {
                to_arm_register: false,
                rt: Reg::R2,
                sn: SingleReg::S4,
            })
            .unwrap();
        assert_eq!(processor.get_sr(SingleReg::S4), 0x1234_5678);

        processor.set_sr(SingleReg::S5, 0x89ab_cdef);
        processor
            .exec_vmov_cr_sp(&VMovCrSpParams {
                to_arm_register: true,
                rt: Reg::R3,
                sn: SingleReg::S5,
            })
            .unwrap();
        assert_eq!(processor.get_r(Reg::R3), 0x89ab_cdef);
    }

    #[test]
    fn test_vmov_cr2_dp_moves_between_core_and_double_registers() {
        let mut processor = fp_test_processor();
        processor.set_r(Reg::R0, 0x1111_2222);
        processor.set_r(Reg::R1, 0x3333_4444);

        processor
            .exec_vmov_cr2_dp(&VMovCr2DpParams {
                to_arm_registers: false,
                rt: Reg::R0,
                rt2: Reg::R1,
                dm: DoubleReg::D6,
            })
            .unwrap();
        assert_eq!(processor.get_dr(DoubleReg::D6), (0x1111_2222, 0x3333_4444));

        processor.set_dr(DoubleReg::D7, 0xaaaa_bbbb, 0xcccc_dddd);
        processor
            .exec_vmov_cr2_dp(&VMovCr2DpParams {
                to_arm_registers: true,
                rt: Reg::R4,
                rt2: Reg::R5,
                dm: DoubleReg::D7,
            })
            .unwrap();
        assert_eq!(processor.get_r(Reg::R4), 0xaaaa_bbbb);
        assert_eq!(processor.get_r(Reg::R5), 0xcccc_dddd);
    }

    #[test]
    fn test_vmov_reg_and_immediate_variants() {
        let mut processor = fp_test_processor();

        processor.set_sr(SingleReg::S0, 0x3f80_0000);
        processor
            .exec_vmov_reg_f32(VMovRegParamsf32 {
                sd: SingleReg::S1,
                sm: SingleReg::S0,
            })
            .unwrap();
        assert_eq!(processor.get_sr(SingleReg::S1), 0x3f80_0000);

        processor.set_dr(DoubleReg::D0, 0x5555_aaaa, 0x1234_5678);
        processor
            .exec_vmov_reg_f64(VMovRegParamsf64 {
                dd: DoubleReg::D1,
                dm: DoubleReg::D0,
            })
            .unwrap();
        assert_eq!(processor.get_dr(DoubleReg::D1), (0x5555_aaaa, 0x1234_5678));

        processor
            .exec_vmov_imm_32(VMovImmParams32 {
                sd: SingleReg::S2,
                imm32: 0x4000_0000,
            })
            .unwrap();
        assert_eq!(processor.get_sr(SingleReg::S2), 0x4000_0000);

        processor
            .exec_vmov_imm_64(VMovImmParams64 {
                dd: DoubleReg::D2,
                imm64: 0x1122_3344_5566_7788,
            })
            .unwrap();
        assert_eq!(processor.get_dr(DoubleReg::D2), (0x5566_7788, 0x1122_3344));
    }

    #[test]
    fn test_vmrs_writes_apsr_flags_and_general_register() {
        let mut processor = fp_test_processor();
        processor.fpscr.set_n(true);
        processor.fpscr.set_z(false);
        processor.fpscr.set_c(true);
        processor.fpscr.set_v(false);

        processor.exec_vmrs(VMRSTarget::APSRNZCV).unwrap();
        assert!(processor.psr.get_n());
        assert!(!processor.psr.get_z());
        assert!(processor.psr.get_c());
        assert!(!processor.psr.get_v());

        processor.fpscr = 0xabcd_1234;
        processor.exec_vmrs(VMRSTarget::Register(Reg::R7)).unwrap();
        assert_eq!(processor.get_r(Reg::R7), 0xabcd_1234);
    }

    // A move inside an IT block whose condition fails does nothing. The firmware's strtod relies on
    // it: a VMOV in an ITEE block overwrote the divisor of a quotient.
    #[test]
    fn test_vmov_obeys_the_it_condition() {
        let mut processor = fp_test_processor();
        processor.set_r(Reg::R0, 0x1111_1111);
        processor.set_r(Reg::R1, 0x2222_2222);
        processor.set_sr(SingleReg::S1, 0x3333_3333);
        processor.set_dr(DoubleReg::D2, 0x4444_4444, 0x5555_5555);
        processor.set_dr(DoubleReg::D3, 0x6666_6666, 0x7777_7777);
        // In an IT NE block with the Z flag set, every instruction is skipped.
        processor.psr.set_z_bit(true);
        let skipped = |processor: &mut Processor| {
            processor
                .set_itstate(((crate::core::condition::Condition::NE.value() as u8) << 4) | 0b1000);
        };

        skipped(&mut processor);
        let to_single = VMovCrSpParams {
            to_arm_register: false,
            rt: Reg::R0,
            sn: SingleReg::S1,
        };
        let outcome = processor.exec_vmov_cr_sp(&to_single).unwrap();
        assert!(matches!(outcome, ExecuteSuccess::NotTaken));
        skipped(&mut processor);
        let to_core = VMovCrSpParams {
            to_arm_register: true,
            ..to_single
        };
        processor.exec_vmov_cr_sp(&to_core).unwrap();
        skipped(&mut processor);
        let to_double = VMovCr2DpParams {
            to_arm_registers: false,
            rt: Reg::R0,
            rt2: Reg::R1,
            dm: DoubleReg::D2,
        };
        processor.exec_vmov_cr2_dp(&to_double).unwrap();
        skipped(&mut processor);
        let to_cores = VMovCr2DpParams {
            to_arm_registers: true,
            ..to_double
        };
        processor.exec_vmov_cr2_dp(&to_cores).unwrap();
        skipped(&mut processor);
        processor
            .exec_vmov_reg_f32(VMovRegParamsf32 {
                sd: SingleReg::S1,
                sm: SingleReg::S0,
            })
            .unwrap();
        skipped(&mut processor);
        processor
            .exec_vmov_reg_f64(VMovRegParamsf64 {
                dd: DoubleReg::D2,
                dm: DoubleReg::D3,
            })
            .unwrap();
        skipped(&mut processor);
        processor
            .exec_vmov_imm_32(VMovImmParams32 {
                sd: SingleReg::S1,
                imm32: 0x4000_0000,
            })
            .unwrap();
        skipped(&mut processor);
        processor
            .exec_vmov_imm_64(VMovImmParams64 {
                dd: DoubleReg::D2,
                imm64: 0x4000_0000_0000_0000,
            })
            .unwrap();

        assert_eq!(processor.get_r(Reg::R0), 0x1111_1111);
        assert_eq!(processor.get_r(Reg::R1), 0x2222_2222);
        assert_eq!(processor.get_sr(SingleReg::S1), 0x3333_3333);
        assert_eq!(processor.get_dr(DoubleReg::D2), (0x4444_4444, 0x5555_5555));
    }

    // Runs a decoded 32-bit instruction on the processor.
    fn run(processor: &mut Processor, opcode: u32) -> ExecuteResult {
        processor.execute_internal(&decode_32(opcode))
    }

    #[test]
    fn test_vmov_cr2_sp2_moves_between_core_and_two_singles() {
        let mut processor = fp_test_processor();
        processor.set_r(Reg::R0, 0x1111_2222);
        processor.set_r(Reg::R1, 0x3333_4444);

        // vmov s1, s2, r0, r1
        run(&mut processor, 0xec41_0a30).unwrap();
        assert_eq!(processor.get_sr(SingleReg::S0), 0);
        assert_eq!(processor.get_sr(SingleReg::S1), 0x1111_2222);
        assert_eq!(processor.get_sr(SingleReg::S2), 0x3333_4444);
        assert_eq!(processor.get_sr(SingleReg::S3), 0);

        // vmov r2, r3, s1, s2
        run(&mut processor, 0xec53_2a30).unwrap();
        assert_eq!(processor.get_r(Reg::R2), 0x1111_2222);
        assert_eq!(processor.get_r(Reg::R3), 0x3333_4444);

        // vmov s0, s1, r0, r1, straight from the parameters
        processor
            .exec_vmov_cr2_sp2(VMovCr2Sp2Params {
                to_arm_registers: false,
                rt: Reg::R1,
                rt2: Reg::R0,
                sm: SingleReg::S10,
                sm1: SingleReg::S11,
            })
            .unwrap();
        assert_eq!(processor.get_sr(SingleReg::S10), 0x3333_4444);
        assert_eq!(processor.get_sr(SingleReg::S11), 0x1111_2222);
    }

    #[test]
    fn test_vmov_cr_scalar_writes_one_word_of_a_double_register() {
        let mut processor = fp_test_processor();
        processor.set_dr(DoubleReg::D3, 0xaaaa_aaaa, 0xbbbb_bbbb);
        processor.set_r(Reg::R2, 0x1234_5678);

        // vmov.32 d3[0], r2
        run(&mut processor, 0xee03_2b10).unwrap();
        assert_eq!(processor.get_dr(DoubleReg::D3), (0x1234_5678, 0xbbbb_bbbb));

        processor.set_r(Reg::R2, 0x9abc_def0);
        // vmov.32 d3[1], r2
        run(&mut processor, 0xee23_2b10).unwrap();
        assert_eq!(processor.get_dr(DoubleReg::D3), (0x1234_5678, 0x9abc_def0));
    }

    #[test]
    fn test_vmov_scalar_cr_reads_one_word_of_a_double_register() {
        let mut processor = fp_test_processor();
        processor.set_dr(DoubleReg::D5, 0x1111_1111, 0x2222_2222);

        // vmov.32 r4, d5[0]
        run(&mut processor, 0xee15_4b10).unwrap();
        assert_eq!(processor.get_r(Reg::R4), 0x1111_1111);
        // vmov.32 r4, d5[1]
        run(&mut processor, 0xee35_4b10).unwrap();
        assert_eq!(processor.get_r(Reg::R4), 0x2222_2222);
    }

    #[test]
    fn test_vmrs_does_nothing_when_the_it_condition_fails() {
        let mut processor = fp_test_processor();
        processor.fpscr = 0x0000_0001;
        processor.set_r(Reg::R3, 0x5555_5555);
        processor.psr.set_z_bit(true);
        processor
            .set_itstate(((crate::core::condition::Condition::NE.value() as u8) << 4) | 0b1000);
        // vmrs r3, fpscr
        let outcome = run(&mut processor, 0xeef1_3a10).unwrap();
        assert!(matches!(outcome, ExecuteSuccess::NotTaken));
        assert_eq!(processor.get_r(Reg::R3), 0x5555_5555);
    }

    #[test]
    fn test_vmrs_to_the_apsr_moves_only_the_flags() {
        let mut processor = fp_test_processor();
        processor.fpscr = 0x9000_0000 | 1;
        // vmrs APSR_nzcv, fpscr
        run(&mut processor, 0xeef1_fa10).unwrap();
        assert!(processor.psr.get_n());
        assert!(!processor.psr.get_z());
        assert!(!processor.psr.get_c());
        assert!(processor.psr.get_v());
    }

    #[test]
    fn test_vmsr_writes_only_the_implemented_fpscr_bits() {
        let mut processor = fp_test_processor();
        processor.set_r(Reg::R0, 0xffff_ffff);
        // vmsr fpscr, r0
        run(&mut processor, 0xeee1_0a10).unwrap();
        assert_eq!(processor.fpscr, FPSCR_WRITE_MASK);
        assert_eq!(processor.fpscr, 0xf7c0_009f);
        // The trap enables read back as zero.
        assert_eq!(processor.fpscr & 0x9f00, 0);

        // vmrs r1, fpscr
        run(&mut processor, 0xeef1_1a10).unwrap();
        assert_eq!(processor.get_r(Reg::R1), 0xf7c0_009f);
    }

    #[test]
    fn test_vmsr_of_zero_clears_the_flags() {
        let mut processor = fp_test_processor();
        processor.fpscr = FPSCR_WRITE_MASK;
        processor.set_r(Reg::R5, 0);
        // vmsr fpscr, r5
        run(&mut processor, 0xeee1_5a10).unwrap();
        assert_eq!(processor.fpscr, 0);
    }

    #[test]
    fn test_vmsr_obeys_the_it_condition() {
        let mut processor = fp_test_processor();
        processor.fpscr = 0x0000_0010;
        processor.set_r(Reg::R0, 0);
        processor.psr.set_z_bit(true);
        processor
            .set_itstate(((crate::core::condition::Condition::NE.value() as u8) << 4) | 0b1000);
        let outcome = run(&mut processor, 0xeee1_0a10).unwrap();
        assert!(matches!(outcome, ExecuteSuccess::NotTaken));
        assert_eq!(processor.fpscr, 0x0000_0010);
    }

    #[test]
    fn test_vmsr_rounding_mode_shows_in_a_later_vadd() {
        // 1 + 2^-30 is not a single: to nearest it is 1, towards plus infinity the next one up.
        let add = VAddSubParamsf32 {
            sd: SingleReg::S2,
            sn: SingleReg::S0,
            sm: SingleReg::S1,
        };
        for (mode, expected) in [
            (FPSCRRounding::RoundToNearest, 0x3f80_0000),
            (FPSCRRounding::RoundTowardsPlusInfinity, 0x3f80_0001),
            (FPSCRRounding::RoundTowardsMinusInfinity, 0x3f80_0000),
            (FPSCRRounding::RoundTowardsZero, 0x3f80_0000),
        ] {
            let mut processor = fp_test_processor();
            processor.set_sr(SingleReg::S0, 1.0f32.to_bits());
            processor.set_sr(SingleReg::S1, 2f32.powi(-30).to_bits());
            let mut fpscr = 0;
            fpscr.set_rounding_mode(mode);
            processor.set_r(Reg::R0, fpscr);
            run(&mut processor, 0xeee1_0a10).unwrap();
            processor.exec_vadd_f32(&add).unwrap();
            assert_eq!(processor.get_sr(SingleReg::S2), expected, "{mode:?}");
        }
    }

    #[test]
    fn test_vmsr_rounding_mode_shows_in_a_later_vcvtr() {
        // vcvtr takes the FPSCR mode: 2.5 is 2 to nearest, ties to even, and 3 towards plus infinity.
        let vcvtr = VCVTParams {
            d: ExtensionReg::Single { reg: SingleReg::S1 },
            m: ExtensionReg::Single { reg: SingleReg::S0 },
            to_integer: true,
            unsigned: false,
            dp_operation: false,
            rounding: None,
        };
        for (mode, expected) in [
            (FPSCRRounding::RoundToNearest, 2),
            (FPSCRRounding::RoundTowardsPlusInfinity, 3),
        ] {
            let mut processor = fp_test_processor();
            processor.set_sr(SingleReg::S0, 2.5f32.to_bits());
            let mut fpscr = 0;
            fpscr.set_rounding_mode(mode);
            processor.set_r(Reg::R0, fpscr);
            run(&mut processor, 0xeee1_0a10).unwrap();
            processor.exec_vcvt(&vcvtr).unwrap();
            assert_eq!(processor.get_sr(SingleReg::S1), expected, "{mode:?}");
        }
    }

    #[test]
    fn test_vmsr_flush_to_zero_shows_in_a_later_vadd() {
        let add = VAddSubParamsf32 {
            sd: SingleReg::S2,
            sn: SingleReg::S0,
            sm: SingleReg::S1,
        };
        let mut processor = fp_test_processor();
        // The smallest subnormal plus zero is itself, until FZ turns the input into a zero.
        processor.set_sr(SingleReg::S0, 1);
        processor.set_sr(SingleReg::S1, 0);
        processor.exec_vadd_f32(&add).unwrap();
        assert_eq!(processor.get_sr(SingleReg::S2), 1);

        processor.set_r(Reg::R0, 1 << 24);
        run(&mut processor, 0xeee1_0a10).unwrap();
        processor.exec_vadd_f32(&add).unwrap();
        assert_eq!(processor.get_sr(SingleReg::S2), 0);
        assert_eq!(processor.fpscr & 0x80, 0x80);
    }
}
