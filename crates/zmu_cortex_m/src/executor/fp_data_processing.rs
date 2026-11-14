use crate::Processor;
use crate::core::fpregister::{FPRounding, Fpscr};
use crate::core::instruction::{
    VAddSubParamsf32, VAddSubParamsf64, VCVTFixedParams, VCVTParams, VCVTParamsF32F64,
    VCVTParamsF64F32, VCmpParamsf32, VCmpParamsf64, VMinMaxNumParamsf32, VMinMaxNumParamsf64,
    VMovRegParamsf32, VMovRegParamsf64, VRintParamsf32, VRintParamsf64, VSelParamsf32,
    VSelParamsf64,
};

use crate::executor::ExecuteSuccess;

use super::ExecuteResult;
use super::fp_generic::{FloatingPointChecks, FloatingPointPublicOperations};
use crate::core::register::{ExtensionReg, ExtensionRegOperations};
use crate::executor::ExecutorHelper;

// The low `size` bits of a value, extended to 64 bits by zeros when they are unsigned and by
// their sign when they are signed.
fn extend_fixed(value: u64, size: usize, unsigned: bool) -> u64 {
    let shift = 64 - size;
    if unsigned {
        (value << shift) >> shift
    } else {
        (((value << shift) as i64) >> shift) as u64
    }
}

// The fixed-point number in the low `size` bits of a register, as the 32-bit value that
// FixedToFP reads.
fn fixed_operand(register: u32, size: usize, unsigned: bool) -> u32 {
    extend_fixed(u64::from(register), size, unsigned) as u32
}

pub trait IsaFloatingPointDataProcessing {
    fn exec_vabs_f32(&mut self, params: VMovRegParamsf32) -> ExecuteResult;
    fn exec_vabs_f64(&mut self, params: VMovRegParamsf64) -> ExecuteResult;
    fn exec_vneg_f32(&mut self, params: VMovRegParamsf32) -> ExecuteResult;
    fn exec_vneg_f64(&mut self, params: VMovRegParamsf64) -> ExecuteResult;

    fn exec_vrint_f32(&mut self, params: VRintParamsf32) -> ExecuteResult;
    fn exec_vrint_f64(&mut self, params: VRintParamsf64) -> ExecuteResult;

    fn exec_vminmaxnm_f32(&mut self, params: &VMinMaxNumParamsf32) -> ExecuteResult;
    fn exec_vminmaxnm_f64(&mut self, params: &VMinMaxNumParamsf64) -> ExecuteResult;

    fn exec_vsqrt_f32(&mut self, params: VMovRegParamsf32) -> ExecuteResult;
    fn exec_vsqrt_f64(&mut self, params: VMovRegParamsf64) -> ExecuteResult;

    fn exec_vcmp_f32(&mut self, params: &VCmpParamsf32) -> ExecuteResult;
    fn exec_vcmp_f64(&mut self, params: &VCmpParamsf64) -> ExecuteResult;

    fn exec_vadd_f32(&mut self, params: &VAddSubParamsf32) -> ExecuteResult;
    fn exec_vadd_f64(&mut self, params: &VAddSubParamsf64) -> ExecuteResult;
    fn exec_vdiv_f32(&mut self, params: &VAddSubParamsf32) -> ExecuteResult;
    fn exec_vdiv_f64(&mut self, params: &VAddSubParamsf64) -> ExecuteResult;
    fn exec_vfma_f32(&mut self, params: &VAddSubParamsf32) -> ExecuteResult;
    fn exec_vfma_f64(&mut self, params: &VAddSubParamsf64) -> ExecuteResult;
    fn exec_vfms_f32(&mut self, params: &VAddSubParamsf32) -> ExecuteResult;
    fn exec_vfms_f64(&mut self, params: &VAddSubParamsf64) -> ExecuteResult;
    fn exec_vfnms_f32(&mut self, params: &VAddSubParamsf32) -> ExecuteResult;
    fn exec_vfnms_f64(&mut self, params: &VAddSubParamsf64) -> ExecuteResult;
    fn exec_vfnma_f32(&mut self, params: &VAddSubParamsf32) -> ExecuteResult;
    fn exec_vfnma_f64(&mut self, params: &VAddSubParamsf64) -> ExecuteResult;
    fn exec_vmul_f32(&mut self, params: &VAddSubParamsf32) -> ExecuteResult;
    fn exec_vmul_f64(&mut self, params: &VAddSubParamsf64) -> ExecuteResult;
    fn exec_vnmul_f32(&mut self, params: &VAddSubParamsf32) -> ExecuteResult;
    fn exec_vnmul_f64(&mut self, params: &VAddSubParamsf64) -> ExecuteResult;

    fn exec_vsub_f32(&mut self, params: &VAddSubParamsf32) -> ExecuteResult;
    fn exec_vsub_f64(&mut self, params: &VAddSubParamsf64) -> ExecuteResult;

    fn exec_vcvt(&mut self, params: &VCVTParams) -> ExecuteResult;
    fn exec_vcvt_fixed(&mut self, params: &VCVTFixedParams) -> ExecuteResult;
    fn exec_vcvt_f64_f32(&mut self, params: VCVTParamsF64F32) -> ExecuteResult;
    fn exec_vcvt_f32_f64(&mut self, params: VCVTParamsF32F64) -> ExecuteResult;
    fn exec_vsel_f32(&mut self, params: VSelParamsf32) -> ExecuteResult;
    fn exec_vsel_f64(&mut self, params: VSelParamsf64) -> ExecuteResult;
}

impl IsaFloatingPointDataProcessing for Processor {
    fn exec_vabs_f32(&mut self, params: VMovRegParamsf32) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let op = self.get_sr(params.sm);
            let result = self.fp_abs::<u32>(op);
            self.set_sr(params.sd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vabs_f64(&mut self, params: VMovRegParamsf64) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let (lower, upper) = self.get_dr(params.dm);
            let op = u64::from(upper) << 32 | (u64::from(lower));

            let result = self.fp_abs::<u64>(op);

            let result_upper = (result >> 32) as u32;
            let result_lower = result as u32;

            self.set_dr(params.dd, result_lower, result_upper);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vneg_f32(&mut self, params: VMovRegParamsf32) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let op = self.get_sr(params.sm);
            self.set_sr(params.sd, op ^ 0x8000_0000);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vneg_f64(&mut self, params: VMovRegParamsf64) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let (lower, upper) = self.get_dr(params.dm);
            self.set_dr(params.dd, lower, upper ^ 0x8000_0000);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vrint_f32(&mut self, params: VRintParamsf32) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let op = self.get_sr(params.sm);
            let result = self.fp_round_int::<u32>(op, params.rounding, params.exact, true);
            self.set_sr(params.sd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vrint_f64(&mut self, params: VRintParamsf64) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let op = self.get_dr_bits(params.dm);
            let result = self.fp_round_int::<u64>(op, params.rounding, params.exact, true);
            self.set_dr_bits(params.dd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vminmaxnm_f32(&mut self, params: &VMinMaxNumParamsf32) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let op1 = self.get_sr(params.sn);
            let op2 = self.get_sr(params.sm);
            let result = self.fp_min_max_num::<u32>(op1, op2, params.max);
            self.set_sr(params.sd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vminmaxnm_f64(&mut self, params: &VMinMaxNumParamsf64) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let op1 = self.get_dr_bits(params.dn);
            let op2 = self.get_dr_bits(params.dm);
            let result = self.fp_min_max_num::<u64>(op1, op2, params.max);
            self.set_dr_bits(params.dd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vsqrt_f32(&mut self, params: VMovRegParamsf32) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let op = self.get_sr(params.sm);
            let result = self.fp_sqrt::<u32>(op, true);
            self.set_sr(params.sd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vsqrt_f64(&mut self, params: VMovRegParamsf64) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let (lower, upper) = self.get_dr(params.dm);
            let op = (u64::from(upper) << 32) | u64::from(lower);
            let result = self.fp_sqrt::<u64>(op, true);
            self.set_dr(params.dd, result as u32, (result >> 32) as u32);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vcmp_f32(&mut self, params: &VCmpParamsf32) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let op32 = if params.with_zero {
                0
            } else {
                self.get_sr(params.sm)
            };
            let op1 = self.get_sr(params.sd);
            let (n, z, c, v) = self.fp_compare::<u32>(op1, op32, params.quiet_nan_exc, true);
            self.fpscr.set_n(n);
            self.fpscr.set_z(z);
            self.fpscr.set_c(c);
            self.fpscr.set_v(v);
        }
        Ok(ExecuteSuccess::NotTaken)
    }
    fn exec_vcmp_f64(&mut self, params: &VCmpParamsf64) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let op64 = if params.with_zero {
                0u64
            } else {
                let (lower, upper) = self.get_dr(params.dm);
                u64::from(upper) << 32 | u64::from(lower)
            };
            let op1_src = self.get_dr(params.dd);
            let op1 = u64::from(op1_src.1) << 32 | u64::from(op1_src.0);
            let (n, z, c, v) = self.fp_compare::<u64>(op1, op64, params.quiet_nan_exc, true);
            self.fpscr.set_n(n);
            self.fpscr.set_z(z);
            self.fpscr.set_c(c);
            self.fpscr.set_v(v);
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vadd_f32(&mut self, params: &VAddSubParamsf32) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let op1 = self.get_sr(params.sn);
            let op2 = self.get_sr(params.sm);
            let result = self.fp_add::<u32>(op1, op2, true);
            self.set_sr(params.sd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }
    fn exec_vadd_f64(&mut self, params: &VAddSubParamsf64) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let (op1_lower, op1_upper) = self.get_dr(params.dn);
            let (op2_lower, op2_upper) = self.get_dr(params.dm);
            let op1 = u64::from(op1_upper) << 32 | u64::from(op1_lower);
            let op2 = u64::from(op2_upper) << 32 | u64::from(op2_lower);
            let result = self.fp_add::<u64>(op1, op2, true);
            let result_upper = (result >> 32) as u32;
            let result_lower = result as u32;
            self.set_dr(params.dd, result_lower, result_upper);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vdiv_f32(&mut self, params: &VAddSubParamsf32) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let op1 = self.get_sr(params.sn);
            let op2 = self.get_sr(params.sm);
            let result = self.fp_div::<u32>(op1, op2, true);
            self.set_sr(params.sd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vdiv_f64(&mut self, params: &VAddSubParamsf64) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let (op1_lower, op1_upper) = self.get_dr(params.dn);
            let (op2_lower, op2_upper) = self.get_dr(params.dm);
            let op1 = u64::from(op1_upper) << 32 | u64::from(op1_lower);
            let op2 = u64::from(op2_upper) << 32 | u64::from(op2_lower);
            let result = self.fp_div::<u64>(op1, op2, true);
            let result_upper = (result >> 32) as u32;
            let result_lower = result as u32;
            self.set_dr(params.dd, result_lower, result_upper);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vfma_f32(&mut self, params: &VAddSubParamsf32) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let addend = self.get_sr(params.sd);
            let op1 = self.get_sr(params.sn);
            let op2 = self.get_sr(params.sm);
            let result = self.fp_mul_add::<u32>(addend, op1, op2, true);
            self.set_sr(params.sd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vfma_f64(&mut self, params: &VAddSubParamsf64) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let (dest_low, dest_high) = self.get_dr(params.dd);
            let (op1_low, op1_high) = self.get_dr(params.dn);
            let (op2_low, op2_high) = self.get_dr(params.dm);
            let addend = (u64::from(dest_high) << 32) | u64::from(dest_low);
            let op1 = (u64::from(op1_high) << 32) | u64::from(op1_low);
            let op2 = (u64::from(op2_high) << 32) | u64::from(op2_low);
            let result = self.fp_mul_add::<u64>(addend, op1, op2, true);
            self.set_dr(params.dd, result as u32, (result >> 32) as u32);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vfms_f32(&mut self, params: &VAddSubParamsf32) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let addend = self.get_sr(params.sd);
            let op1 = self.get_sr(params.sn) ^ 0x8000_0000;
            let op2 = self.get_sr(params.sm);
            let result = self.fp_mul_add::<u32>(addend, op1, op2, true);
            self.set_sr(params.sd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vfms_f64(&mut self, params: &VAddSubParamsf64) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let addend = self.get_dr_bits(params.dd);
            let op1 = self.get_dr_bits(params.dn) ^ 0x8000_0000_0000_0000;
            let op2 = self.get_dr_bits(params.dm);
            let result = self.fp_mul_add::<u64>(addend, op1, op2, true);
            self.set_dr(params.dd, result as u32, (result >> 32) as u32);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vfnms_f32(&mut self, params: &VAddSubParamsf32) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let addend = self.get_sr(params.sd) ^ 0x8000_0000;
            let op1 = self.get_sr(params.sn);
            let op2 = self.get_sr(params.sm);
            let result = self.fp_mul_add::<u32>(addend, op1, op2, true);
            self.set_sr(params.sd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vfnms_f64(&mut self, params: &VAddSubParamsf64) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let (dest_low, dest_high) = self.get_dr(params.dd);
            let (op1_low, op1_high) = self.get_dr(params.dn);
            let (op2_low, op2_high) = self.get_dr(params.dm);
            let addend =
                ((u64::from(dest_high) << 32) | u64::from(dest_low)) ^ 0x8000_0000_0000_0000;
            let op1 = (u64::from(op1_high) << 32) | u64::from(op1_low);
            let op2 = (u64::from(op2_high) << 32) | u64::from(op2_low);
            let result = self.fp_mul_add::<u64>(addend, op1, op2, true);
            self.set_dr(params.dd, result as u32, (result >> 32) as u32);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vfnma_f32(&mut self, params: &VAddSubParamsf32) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let addend = self.get_sr(params.sd) ^ 0x8000_0000;
            let op1 = self.get_sr(params.sn) ^ 0x8000_0000;
            let op2 = self.get_sr(params.sm);
            let result = self.fp_mul_add::<u32>(addend, op1, op2, true);
            self.set_sr(params.sd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vfnma_f64(&mut self, params: &VAddSubParamsf64) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let addend = self.get_dr_bits(params.dd) ^ 0x8000_0000_0000_0000;
            let op1 = self.get_dr_bits(params.dn) ^ 0x8000_0000_0000_0000;
            let op2 = self.get_dr_bits(params.dm);
            let result = self.fp_mul_add::<u64>(addend, op1, op2, true);
            self.set_dr_bits(params.dd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vmul_f32(&mut self, params: &VAddSubParamsf32) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let op1 = self.get_sr(params.sn);
            let op2 = self.get_sr(params.sm);
            let result = self.fp_mul::<u32>(op1, op2, true);
            self.set_sr(params.sd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vmul_f64(&mut self, params: &VAddSubParamsf64) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let (op1_lower, op1_upper) = self.get_dr(params.dn);
            let (op2_lower, op2_upper) = self.get_dr(params.dm);
            let op1 = u64::from(op1_upper) << 32 | u64::from(op1_lower);
            let op2 = u64::from(op2_upper) << 32 | u64::from(op2_lower);
            let result = self.fp_mul::<u64>(op1, op2, true);
            let result_upper = (result >> 32) as u32;
            let result_lower = result as u32;
            self.set_dr(params.dd, result_lower, result_upper);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vnmul_f32(&mut self, params: &VAddSubParamsf32) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let op1 = self.get_sr(params.sn);
            let op2 = self.get_sr(params.sm);
            let result = self.fp_mul::<u32>(op1, op2, true) ^ 0x8000_0000;
            self.set_sr(params.sd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vnmul_f64(&mut self, params: &VAddSubParamsf64) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let (op1_lower, op1_upper) = self.get_dr(params.dn);
            let (op2_lower, op2_upper) = self.get_dr(params.dm);
            let op1 = u64::from(op1_upper) << 32 | u64::from(op1_lower);
            let op2 = u64::from(op2_upper) << 32 | u64::from(op2_lower);
            let result = self.fp_mul::<u64>(op1, op2, true) ^ 0x8000_0000_0000_0000;
            let result_upper = (result >> 32) as u32;
            let result_lower = result as u32;
            self.set_dr(params.dd, result_lower, result_upper);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vsub_f32(&mut self, params: &VAddSubParamsf32) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let op1 = self.get_sr(params.sn);
            let op2 = self.get_sr(params.sm);
            let result = self.fp_sub::<u32>(op1, op2, true);
            self.set_sr(params.sd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }
    fn exec_vsub_f64(&mut self, params: &VAddSubParamsf64) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let (op1_lower, op1_upper) = self.get_dr(params.dn);
            let (op2_lower, op2_upper) = self.get_dr(params.dm);
            let op1 = u64::from(op1_upper) << 32 | u64::from(op1_lower);
            let op2 = u64::from(op2_upper) << 32 | u64::from(op2_lower);
            let result = self.fp_sub::<u64>(op1, op2, true);
            let result_upper = (result >> 32) as u32;
            let result_lower = result as u32;
            self.set_dr(params.dd, result_lower, result_upper);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vcvt(&mut self, params: &VCVTParams) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;

            if params.to_integer {
                if params.dp_operation {
                    let m_reg = params
                        .m
                        .as_double()
                        .expect("Invalid register for double precision operation");
                    let d_reg = params
                        .d
                        .as_single()
                        .expect("Invalid register for single precision operation");
                    let dm = self.get_dr(*m_reg);
                    let dm_val = u64::from(dm.1) << 32 | u64::from(dm.0);
                    let result = self.fp_to_fixed::<u64>(
                        dm_val,
                        32,
                        0,
                        params.unsigned,
                        params.rounding,
                        true,
                    );
                    self.set_sr(*d_reg, result as u32);
                } else {
                    let m_reg = params
                        .m
                        .as_single()
                        .expect("Invalid register for single precision operation");
                    let d_reg = params
                        .d
                        .as_single()
                        .expect("Invalid register for single precision operation");
                    let dm = self.get_sr(*m_reg);
                    let result =
                        self.fp_to_fixed::<u32>(dm, 32, 0, params.unsigned, params.rounding, true);
                    self.set_sr(*d_reg, result as u32);
                }
            } else if params.dp_operation {
                let m_reg = params
                    .m
                    .as_single()
                    .expect("Invalid register for single precision operation");
                let d_reg = params
                    .d
                    .as_double()
                    .expect("Invalid register for double precision operation");
                let dm = self.get_sr(*m_reg);
                let result = self.fixed_to_fp::<u64, u32>(dm, 0, params.unsigned, false, true);
                let result_upper = (result >> 32) as u32;
                let result_lower = result as u32;
                self.set_dr(*d_reg, result_lower, result_upper);
            } else {
                let m_reg = params
                    .m
                    .as_single()
                    .expect("Invalid register for single precision operation");
                let d_reg = params
                    .d
                    .as_single()
                    .expect("Invalid register for single precision operation");
                let dm = self.get_sr(*m_reg);
                let result = self.fixed_to_fp::<u32, u32>(dm, 0, params.unsigned, false, true);
                self.set_sr(*d_reg, result);
            }

            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vcvt_fixed(&mut self, params: &VCVTFixedParams) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let size = usize::from(params.size);
            let fraction_bits = usize::from(params.fraction_bits);
            let unsigned = params.unsigned;
            match params.reg {
                ExtensionReg::Single { reg } => {
                    let op = self.get_sr(reg);
                    let result = if params.to_fixed {
                        let fixed = self.fp_to_fixed::<u32>(
                            op,
                            size,
                            fraction_bits,
                            unsigned,
                            Some(FPRounding::Zero),
                            true,
                        );
                        extend_fixed(fixed, size, unsigned) as u32
                    } else {
                        let fixed = fixed_operand(op, size, unsigned);
                        self.fixed_to_fp::<u32, u32>(fixed, fraction_bits, unsigned, true, true)
                    };
                    self.set_sr(reg, result);
                }
                ExtensionReg::Double { reg } => {
                    let result = if params.to_fixed {
                        let op = self.get_dr_bits(reg);
                        let fixed = self.fp_to_fixed::<u64>(
                            op,
                            size,
                            fraction_bits,
                            unsigned,
                            Some(FPRounding::Zero),
                            true,
                        );
                        extend_fixed(fixed, size, unsigned)
                    } else {
                        // The operand is the low bits of the register, which are all in the
                        // low word.
                        let (low_word, _) = self.get_dr(reg);
                        let fixed = fixed_operand(low_word, size, unsigned);
                        self.fixed_to_fp::<u64, u32>(fixed, fraction_bits, unsigned, true, true)
                    };
                    self.set_dr_bits(reg, result);
                }
            }
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vcvt_f64_f32(&mut self, params: VCVTParamsF64F32) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let source = self.get_sr(params.sm);
            let result = self.fp_convert::<u32, u64>(source);
            self.set_dr(params.dd, result as u32, (result >> 32) as u32);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vcvt_f32_f64(&mut self, params: VCVTParamsF32F64) -> ExecuteResult {
        if self.condition_passed() {
            self.execute_fp_check()?;
            let source = self.get_dr_bits(params.dm);
            let result = self.fp_convert::<u64, u32>(source);
            self.set_sr(params.sd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_vsel_f32(&mut self, params: VSelParamsf32) -> ExecuteResult {
        self.execute_fp_check()?;
        let result = if self.condition_passed_b(params.cond) {
            self.get_sr(params.sn)
        } else {
            self.get_sr(params.sm)
        };
        self.set_sr(params.sd, result);
        Ok(ExecuteSuccess::Taken { cycles: 1 })
    }

    fn exec_vsel_f64(&mut self, params: VSelParamsf64) -> ExecuteResult {
        self.execute_fp_check()?;
        let result = if self.condition_passed_b(params.cond) {
            self.get_dr(params.dn)
        } else {
            self.get_dr(params.dm)
        };
        self.set_dr(params.dd, result.0, result.1);
        Ok(ExecuteSuccess::Taken { cycles: 1 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::bits::Bits;
    use crate::core::fpregister::FPSCRRounding;
    use crate::core::register::{Apsr, DoubleReg, ExtensionReg, SingleReg};
    use crate::executor::fp_test_support::*;

    fn fp_test_processor() -> Processor {
        let mut processor = Processor::new();
        processor.cpacr = 0x00f0_0000;
        processor
    }

    #[test]
    fn test_vabs_f32() {
        let mut processor = fp_test_processor();

        // -1.0
        processor.set_sr(SingleReg::S0, 0xBF80_0000);
        processor
            .exec_vabs_f32(VMovRegParamsf32 {
                sd: SingleReg::S1,
                sm: SingleReg::S0,
            })
            .unwrap();

        let result = processor.get_sr(SingleReg::S1);
        assert_eq!(result, 0x3F80_0000);
    }

    #[test]
    fn test_vabs_f64() {
        let mut processor = fp_test_processor();

        // -1.0
        processor.set_dr(DoubleReg::D0, 0x0000_0000, 0xBFF0_0000);
        processor
            .exec_vabs_f64(VMovRegParamsf64 {
                dd: DoubleReg::D1,
                dm: DoubleReg::D0,
            })
            .unwrap();

        let result = processor.get_dr(DoubleReg::D1);
        assert_eq!(result, (0x0000_0000, 0x3FF0_0000));
    }

    #[test]
    fn test_vneg_f32() {
        let mut processor = fp_test_processor();

        processor.set_sr(SingleReg::S0, 1.5f32.to_bits());
        processor
            .exec_vneg_f32(VMovRegParamsf32 {
                sd: SingleReg::S1,
                sm: SingleReg::S0,
            })
            .unwrap();

        assert_eq!(processor.get_sr(SingleReg::S1), (-1.5f32).to_bits());
    }

    #[test]
    fn test_vneg_f64() {
        let mut processor = fp_test_processor();

        let bits = 1.5f64.to_bits();
        processor.set_dr(DoubleReg::D0, bits as u32, (bits >> 32) as u32);
        processor
            .exec_vneg_f64(VMovRegParamsf64 {
                dd: DoubleReg::D1,
                dm: DoubleReg::D0,
            })
            .unwrap();

        let result = processor.get_dr(DoubleReg::D1);
        let result_bits = (u64::from(result.1) << 32) | u64::from(result.0);
        assert_eq!(result_bits, (-1.5f64).to_bits());
    }

    #[test]
    fn test_vsqrt_f32() {
        let mut processor = fp_test_processor();

        processor.set_sr(SingleReg::S0, 0x4080_0000);
        processor
            .exec_vsqrt_f32(VMovRegParamsf32 {
                sd: SingleReg::S1,
                sm: SingleReg::S0,
            })
            .unwrap();

        let result = processor.get_sr(SingleReg::S1);
        assert_eq!(result, 0x4000_0000);
    }

    #[test]
    fn test_vsqrt_f64() {
        let mut processor = fp_test_processor();

        processor.set_dr(DoubleReg::D0, 0x0000_0000, 0x4010_0000);
        processor
            .exec_vsqrt_f64(VMovRegParamsf64 {
                dd: DoubleReg::D1,
                dm: DoubleReg::D0,
            })
            .unwrap();

        let result = processor.get_dr(DoubleReg::D1);
        assert_eq!(result, (0x0000_0000, 0x4000_0000));
    }

    // Runs one VRINT on a double and gives back the result and the FPSCR.
    fn vrint_f64(rounding: Option<FPRounding>, exact: bool, fpscr: u32, x: f64) -> (f64, u32) {
        let mut processor = fp_test_processor();
        processor.fpscr = fpscr;
        processor.set_dr_bits(DoubleReg::D7, x.to_bits());
        processor
            .exec_vrint_f64(VRintParamsf64 {
                dd: DoubleReg::D6,
                dm: DoubleReg::D7,
                rounding,
                exact,
            })
            .unwrap();
        (
            f64::from_bits(processor.get_dr_bits(DoubleReg::D6)),
            processor.fpscr,
        )
    }

    fn vrint_f32(rounding: Option<FPRounding>, exact: bool, fpscr: u32, x: f32) -> (f32, u32) {
        let mut processor = fp_test_processor();
        processor.fpscr = fpscr;
        processor.set_sr(SingleReg::S15, x.to_bits());
        processor
            .exec_vrint_f32(VRintParamsf32 {
                sd: SingleReg::S14,
                sm: SingleReg::S15,
                rounding,
                exact,
            })
            .unwrap();
        (
            f32::from_bits(processor.get_sr(SingleReg::S14)),
            processor.fpscr,
        )
    }

    #[test]
    fn test_vrint_f32_rounds_as_named() {
        for rounding in ALL_ROUNDINGS {
            for x in [2.5f32, -2.5, 3.5, 0.4, -0.4, -0.0, 1e30, f32::INFINITY] {
                let (result, fpscr) = vrint_f32(Some(rounding), false, 0, x);
                assert_eq!(
                    result.to_bits(),
                    host_rint32(x, rounding).to_bits(),
                    "{rounding:?} {x}"
                );
                assert_eq!(fpscr, 0);
            }
        }
    }

    #[test]
    fn test_vrint_f64_rounds_as_named() {
        for rounding in ALL_ROUNDINGS {
            for x in [2.5f64, -2.5, 3.5, 0.4, -0.4, -0.0, 1e300, f64::INFINITY] {
                let (result, fpscr) = vrint_f64(Some(rounding), false, 0, x);
                assert_eq!(
                    result.to_bits(),
                    host_rint64(x, rounding).to_bits(),
                    "{rounding:?} {x}"
                );
                assert_eq!(fpscr, 0);
            }
        }
    }

    #[test]
    fn test_vrint_ignores_the_fpscr_rounding_mode_when_it_names_one() {
        let mut fpscr = 0;
        fpscr.set_rounding_mode(FPSCRRounding::RoundTowardsPlusInfinity);
        assert_eq!(
            vrint_f32(Some(FPRounding::Zero), false, fpscr, -2.9).0,
            -2.0
        );
        assert_eq!(
            vrint_f64(Some(FPRounding::Zero), false, fpscr, -2.9).0,
            -2.0
        );
    }

    #[test]
    fn test_vrintr_and_vrintx_follow_the_fpscr_rounding_mode() {
        let modes = [
            (FPSCRRounding::RoundToNearest, FPRounding::TieEven),
            (FPSCRRounding::RoundTowardsPlusInfinity, FPRounding::PosInf),
            (FPSCRRounding::RoundTowardsMinusInfinity, FPRounding::NegInf),
            (FPSCRRounding::RoundTowardsZero, FPRounding::Zero),
        ];
        for (mode, rounding) in modes {
            let mut fpscr = 0;
            fpscr.set_rounding_mode(mode);
            for exact in [false, true] {
                let (result, _) = vrint_f32(None, exact, fpscr, 2.5);
                assert_eq!(result, host_rint32(2.5, rounding), "{mode:?}");
                let (result, _) = vrint_f64(None, exact, fpscr, -2.5);
                assert_eq!(result, host_rint64(-2.5, rounding), "{mode:?}");
            }
        }
    }

    #[test]
    fn test_vrintx_signals_inexact_and_vrintr_does_not() {
        assert_eq!(vrint_f32(None, true, 0, 2.5).1, IXC);
        assert_eq!(vrint_f64(None, true, 0, 2.5).1, IXC);
        assert_eq!(vrint_f32(None, false, 0, 2.5).1, 0);
        assert_eq!(vrint_f64(None, false, 0, 2.5).1, 0);
        // A result equal to the operand is exact.
        assert_eq!(vrint_f32(None, true, 0, 2.0).1, 0);
        assert_eq!(vrint_f64(None, true, 0, 2.0).1, 0);
    }

    #[test]
    fn test_vrint_obeys_the_it_condition() {
        let mut processor = fp_test_processor();
        // In an IT NE block with the Z flag set, the instruction is skipped.
        processor.set_dr_bits(DoubleReg::D7, 2.5f64.to_bits());
        processor.set_dr_bits(DoubleReg::D6, 9.0f64.to_bits());
        processor.psr.set_z_bit(true);
        processor
            .set_itstate(((crate::core::condition::Condition::NE.value() as u8) << 4) | 0b1000);
        let outcome = processor
            .exec_vrint_f64(VRintParamsf64 {
                dd: DoubleReg::D6,
                dm: DoubleReg::D7,
                rounding: Some(FPRounding::Zero),
                exact: false,
            })
            .unwrap();
        assert!(matches!(outcome, ExecuteSuccess::NotTaken));
        assert_eq!(processor.get_dr_bits(DoubleReg::D6), 9.0f64.to_bits());
    }

    #[test]
    fn test_vrint_faults_when_the_fpu_is_off() {
        let mut processor = Processor::new();
        processor.cpacr = 0;
        let result = processor.exec_vrint_f32(VRintParamsf32 {
            sd: SingleReg::S0,
            sm: SingleReg::S1,
            rounding: Some(FPRounding::Zero),
            exact: false,
        });
        assert!(result.is_err());
    }

    fn min_max_f64(max: bool, a: f64, b: f64) -> f64 {
        let mut processor = fp_test_processor();
        processor.set_dr_bits(DoubleReg::D1, a.to_bits());
        processor.set_dr_bits(DoubleReg::D2, b.to_bits());
        processor
            .exec_vminmaxnm_f64(&VMinMaxNumParamsf64 {
                dd: DoubleReg::D0,
                dn: DoubleReg::D1,
                dm: DoubleReg::D2,
                max,
            })
            .unwrap();
        f64::from_bits(processor.get_dr_bits(DoubleReg::D0))
    }

    fn min_max_f32(max: bool, a: f32, b: f32) -> f32 {
        let mut processor = fp_test_processor();
        processor.set_sr(SingleReg::S1, a.to_bits());
        processor.set_sr(SingleReg::S2, b.to_bits());
        processor
            .exec_vminmaxnm_f32(&VMinMaxNumParamsf32 {
                sd: SingleReg::S0,
                sn: SingleReg::S1,
                sm: SingleReg::S2,
                max,
            })
            .unwrap();
        f32::from_bits(processor.get_sr(SingleReg::S0))
    }

    #[test]
    fn test_vmaxnm_and_vminnm_pick_from_the_right_registers() {
        assert_eq!(min_max_f64(true, 1.5, -2.0), 1.5);
        assert_eq!(min_max_f64(true, -2.0, 1.5), 1.5);
        assert_eq!(min_max_f64(false, 1.5, -2.0), -2.0);
        assert_eq!(min_max_f64(false, -2.0, 1.5), -2.0);
        assert_eq!(min_max_f32(true, 1.5, -2.0), 1.5);
        assert_eq!(min_max_f32(true, -2.0, 1.5), 1.5);
        assert_eq!(min_max_f32(false, 1.5, -2.0), -2.0);
        assert_eq!(min_max_f32(false, -2.0, 1.5), -2.0);
    }

    #[test]
    fn test_vmaxnm_and_vminnm_return_the_number_for_a_quiet_nan() {
        assert_eq!(min_max_f64(true, f64::NAN, 4.0), 4.0);
        assert_eq!(min_max_f64(false, 4.0, f64::NAN), 4.0);
        assert_eq!(min_max_f32(true, f32::NAN, -4.0), -4.0);
        assert_eq!(min_max_f32(false, -4.0, f32::NAN), -4.0);
    }

    #[test]
    fn test_vmaxnm_and_vminnm_order_the_zeros() {
        assert_eq!(min_max_f64(true, -0.0, 0.0).to_bits(), 0.0f64.to_bits());
        assert_eq!(min_max_f64(false, 0.0, -0.0).to_bits(), (-0.0f64).to_bits());
        assert_eq!(min_max_f32(true, 0.0, -0.0).to_bits(), 0.0f32.to_bits());
        assert_eq!(min_max_f32(false, -0.0, 0.0).to_bits(), (-0.0f32).to_bits());
    }

    #[test]
    fn test_vfnma_f32() {
        // -(10) + (-(2)) * 3
        let mut processor = fp_test_processor();
        processor.set_sr(SingleReg::S0, 10.0f32.to_bits());
        processor.set_sr(SingleReg::S5, 2.0f32.to_bits());
        processor.set_sr(SingleReg::S14, 3.0f32.to_bits());
        processor
            .exec_vfnma_f32(&VAddSubParamsf32 {
                sd: SingleReg::S0,
                sn: SingleReg::S5,
                sm: SingleReg::S14,
            })
            .unwrap();
        assert_eq!(processor.get_sr(SingleReg::S0), (-16.0f32).to_bits());
    }

    #[test]
    fn test_vfnma_f64() {
        let mut processor = fp_test_processor();
        processor.set_dr_bits(DoubleReg::D0, 10.0f64.to_bits());
        processor.set_dr_bits(DoubleReg::D5, 2.0f64.to_bits());
        processor.set_dr_bits(DoubleReg::D14, 3.0f64.to_bits());
        processor
            .exec_vfnma_f64(&VAddSubParamsf64 {
                dd: DoubleReg::D0,
                dn: DoubleReg::D5,
                dm: DoubleReg::D14,
            })
            .unwrap();
        assert_eq!(processor.get_dr_bits(DoubleReg::D0), (-16.0f64).to_bits());
    }

    #[test]
    fn test_vfnma_is_fused() {
        // (1 + 2^-30)^2 is 1 + 2^-29 + 2^-60, and a double cannot hold the last term. With the
        // product rounded before the add, taking 1 + 2^-29 off it leaves nothing. Fused, it
        // leaves 2^-60.
        let a = 1.0 + 2f64.powi(-30);
        let mut processor = fp_test_processor();
        processor.set_dr_bits(DoubleReg::D0, (1.0 + 2f64.powi(-29)).to_bits());
        processor.set_dr_bits(DoubleReg::D1, (-a).to_bits());
        processor.set_dr_bits(DoubleReg::D2, a.to_bits());
        processor
            .exec_vfnma_f64(&VAddSubParamsf64 {
                dd: DoubleReg::D0,
                dn: DoubleReg::D1,
                dm: DoubleReg::D2,
            })
            .unwrap();
        assert_eq!(
            processor.get_dr_bits(DoubleReg::D0),
            2f64.powi(-60).to_bits()
        );
    }

    #[test]
    fn test_vfnma_f32_matches_the_host_fused_multiply_add() {
        let values = [
            0.0f32,
            -0.0,
            1.0,
            -1.5,
            3.25,
            1e20,
            -1e-20,
            f32::INFINITY,
            f32::NAN,
        ];
        for d in values {
            for n in values {
                for m in values {
                    let mut processor = fp_test_processor();
                    processor.set_sr(SingleReg::S0, d.to_bits());
                    processor.set_sr(SingleReg::S1, n.to_bits());
                    processor.set_sr(SingleReg::S2, m.to_bits());
                    processor
                        .exec_vfnma_f32(&VAddSubParamsf32 {
                            sd: SingleReg::S0,
                            sn: SingleReg::S1,
                            sm: SingleReg::S2,
                        })
                        .unwrap();
                    let expected = (-n).mul_add(m, -d);
                    let result = f32::from_bits(processor.get_sr(SingleReg::S0));
                    if expected.is_nan() {
                        assert!(result.is_nan(), "{d} {n} {m}");
                    } else {
                        assert_eq!(result.to_bits(), expected.to_bits(), "{d} {n} {m}");
                    }
                }
            }
        }
    }

    // Converts a double to an integer with the named rounding, the way VCVTA and its kin do.
    fn vcvt_f64_to_integer(rounding: FPRounding, unsigned: bool, x: f64) -> u32 {
        let mut processor = fp_test_processor();
        processor.set_dr_bits(DoubleReg::D3, x.to_bits());
        processor
            .exec_vcvt(&VCVTParams {
                d: ExtensionReg::Single { reg: SingleReg::S8 },
                m: ExtensionReg::Double { reg: DoubleReg::D3 },
                to_integer: true,
                unsigned,
                dp_operation: true,
                rounding: Some(rounding),
            })
            .unwrap();
        processor.get_sr(SingleReg::S8)
    }

    fn vcvt_f32_to_integer(rounding: FPRounding, unsigned: bool, x: f32) -> u32 {
        let mut processor = fp_test_processor();
        processor.set_sr(SingleReg::S3, x.to_bits());
        processor
            .exec_vcvt(&VCVTParams {
                d: ExtensionReg::Single { reg: SingleReg::S8 },
                m: ExtensionReg::Single { reg: SingleReg::S3 },
                to_integer: true,
                unsigned,
                dp_operation: false,
                rounding: Some(rounding),
            })
            .unwrap();
        processor.get_sr(SingleReg::S8)
    }

    #[test]
    fn test_vcvt_to_integer_rounds_as_named() {
        for rounding in ALL_ROUNDINGS {
            for x in [
                2.5f64,
                -2.5,
                3.5,
                0.4,
                -0.4,
                2.7,
                -2.7,
                1e10,
                -1e10,
                f64::NAN,
            ] {
                let rounded = host_rint64(x, rounding);
                assert_eq!(
                    vcvt_f64_to_integer(rounding, false, x),
                    rounded as i32 as u32,
                    "{rounding:?} {x} signed"
                );
                assert_eq!(
                    vcvt_f64_to_integer(rounding, true, x),
                    rounded as u32,
                    "{rounding:?} {x} unsigned"
                );
                let single = x as f32;
                let rounded = host_rint32(single, rounding);
                assert_eq!(
                    vcvt_f32_to_integer(rounding, false, single),
                    rounded as i32 as u32,
                    "{rounding:?} {single} signed"
                );
                assert_eq!(
                    vcvt_f32_to_integer(rounding, true, single),
                    rounded as u32,
                    "{rounding:?} {single} unsigned"
                );
            }
        }
    }

    #[test]
    fn test_vcvtr_uses_the_fpscr_rounding_mode() {
        let mut processor = fp_test_processor();
        processor
            .fpscr
            .set_rounding_mode(FPSCRRounding::RoundTowardsMinusInfinity);
        processor.set_sr(SingleReg::S3, 2.9f32.to_bits());
        processor
            .exec_vcvt(&VCVTParams {
                d: ExtensionReg::Single { reg: SingleReg::S8 },
                m: ExtensionReg::Single { reg: SingleReg::S3 },
                to_integer: true,
                unsigned: false,
                dp_operation: false,
                rounding: None,
            })
            .unwrap();
        assert_eq!(processor.get_sr(SingleReg::S8), 2);

        processor.set_sr(SingleReg::S3, (-2.1f32).to_bits());
        processor
            .exec_vcvt(&VCVTParams {
                d: ExtensionReg::Single { reg: SingleReg::S8 },
                m: ExtensionReg::Single { reg: SingleReg::S3 },
                to_integer: true,
                unsigned: false,
                dp_operation: false,
                rounding: None,
            })
            .unwrap();
        assert_eq!(processor.get_sr(SingleReg::S8) as i32, -3);
    }

    #[test]
    fn test_vsqrt_f32_zero_and_inf() {
        let mut processor = fp_test_processor();

        processor.set_sr(SingleReg::S0, 0.0f32.to_bits());
        processor
            .exec_vsqrt_f32(VMovRegParamsf32 {
                sd: SingleReg::S1,
                sm: SingleReg::S0,
            })
            .unwrap();
        assert_eq!(processor.get_sr(SingleReg::S1), 0.0f32.to_bits());

        processor.set_sr(SingleReg::S0, f32::INFINITY.to_bits());
        processor
            .exec_vsqrt_f32(VMovRegParamsf32 {
                sd: SingleReg::S1,
                sm: SingleReg::S0,
            })
            .unwrap();
        assert_eq!(processor.get_sr(SingleReg::S1), f32::INFINITY.to_bits());
    }

    #[test]
    fn test_vsqrt_f32_negative_and_nan() {
        let mut processor = fp_test_processor();

        processor.set_sr(SingleReg::S0, (-1.0f32).to_bits());
        processor
            .exec_vsqrt_f32(VMovRegParamsf32 {
                sd: SingleReg::S1,
                sm: SingleReg::S0,
            })
            .unwrap();
        assert!(f32::from_bits(processor.get_sr(SingleReg::S1)).is_nan());

        processor.set_sr(SingleReg::S0, f32::NAN.to_bits());
        processor
            .exec_vsqrt_f32(VMovRegParamsf32 {
                sd: SingleReg::S1,
                sm: SingleReg::S0,
            })
            .unwrap();
        assert!(f32::from_bits(processor.get_sr(SingleReg::S1)).is_nan());
    }

    #[test]
    fn test_vsqrt_f32_subnormal() {
        let mut processor = fp_test_processor();

        let input = f32::from_bits(0x0000_0001);
        processor.set_sr(SingleReg::S0, input.to_bits());
        processor
            .exec_vsqrt_f32(VMovRegParamsf32 {
                sd: SingleReg::S1,
                sm: SingleReg::S0,
            })
            .unwrap();

        let expected = input.sqrt().to_bits();
        assert_eq!(processor.get_sr(SingleReg::S1), expected);
    }

    #[test]
    fn test_vsqrt_f32_negative_sets_invalid_op() {
        let mut processor = fp_test_processor();
        processor.fpscr = 0;

        processor.set_sr(SingleReg::S0, (-4.0f32).to_bits());
        processor
            .exec_vsqrt_f32(VMovRegParamsf32 {
                sd: SingleReg::S1,
                sm: SingleReg::S0,
            })
            .unwrap();

        assert!(processor.fpscr.get_bit(0));
        assert!(f32::from_bits(processor.get_sr(SingleReg::S1)).is_nan());
    }

    #[test]
    fn test_vsqrt_f32_snan_sets_invalid_op_and_quiets_nan() {
        let mut processor = fp_test_processor();
        processor.fpscr = 0;

        let snan = 0x7F80_0001u32;
        processor.set_sr(SingleReg::S0, snan);
        processor
            .exec_vsqrt_f32(VMovRegParamsf32 {
                sd: SingleReg::S1,
                sm: SingleReg::S0,
            })
            .unwrap();

        let result = processor.get_sr(SingleReg::S1);
        assert!(processor.fpscr.get_bit(0));
        assert!(result.get_bit(22));
        assert!(f32::from_bits(result).is_nan());
    }

    #[test]
    fn test_vsqrt_f32_qnan_does_not_set_invalid_op() {
        let mut processor = fp_test_processor();
        processor.fpscr = 0;

        let qnan = 0x7FC0_1234u32;
        processor.set_sr(SingleReg::S0, qnan);
        processor
            .exec_vsqrt_f32(VMovRegParamsf32 {
                sd: SingleReg::S1,
                sm: SingleReg::S0,
            })
            .unwrap();

        let result = processor.get_sr(SingleReg::S1);
        assert!(!processor.fpscr.get_bit(0));
        assert_eq!(result, qnan);
    }

    #[test]
    fn test_vsqrt_f64_zero_and_inf() {
        let mut processor = fp_test_processor();

        processor.set_dr(
            DoubleReg::D0,
            0.0f64.to_bits() as u32,
            (0.0f64.to_bits() >> 32) as u32,
        );
        processor
            .exec_vsqrt_f64(VMovRegParamsf64 {
                dd: DoubleReg::D1,
                dm: DoubleReg::D0,
            })
            .unwrap();
        let result = processor.get_dr(DoubleReg::D1);
        assert_eq!(
            (u64::from(result.1) << 32) | u64::from(result.0),
            0.0f64.to_bits()
        );

        processor.set_dr(
            DoubleReg::D0,
            f64::INFINITY.to_bits() as u32,
            (f64::INFINITY.to_bits() >> 32) as u32,
        );
        processor
            .exec_vsqrt_f64(VMovRegParamsf64 {
                dd: DoubleReg::D1,
                dm: DoubleReg::D0,
            })
            .unwrap();
        let result = processor.get_dr(DoubleReg::D1);
        assert_eq!(
            (u64::from(result.1) << 32) | u64::from(result.0),
            f64::INFINITY.to_bits()
        );
    }

    #[test]
    fn test_vsqrt_f64_negative_and_nan() {
        let mut processor = fp_test_processor();

        processor.set_dr(
            DoubleReg::D0,
            (-1.0f64).to_bits() as u32,
            ((-1.0f64).to_bits() >> 32) as u32,
        );
        processor
            .exec_vsqrt_f64(VMovRegParamsf64 {
                dd: DoubleReg::D1,
                dm: DoubleReg::D0,
            })
            .unwrap();
        let result = processor.get_dr(DoubleReg::D1);
        let result_bits = (u64::from(result.1) << 32) | u64::from(result.0);
        assert!(f64::from_bits(result_bits).is_nan());

        processor.set_dr(
            DoubleReg::D0,
            f64::NAN.to_bits() as u32,
            (f64::NAN.to_bits() >> 32) as u32,
        );
        processor
            .exec_vsqrt_f64(VMovRegParamsf64 {
                dd: DoubleReg::D1,
                dm: DoubleReg::D0,
            })
            .unwrap();
        let result = processor.get_dr(DoubleReg::D1);
        let result_bits = (u64::from(result.1) << 32) | u64::from(result.0);
        assert!(f64::from_bits(result_bits).is_nan());
    }

    #[test]
    fn test_vsqrt_f64_subnormal() {
        let mut processor = fp_test_processor();

        let input = f64::from_bits(0x0000_0000_0000_0001);
        processor.set_dr(
            DoubleReg::D0,
            input.to_bits() as u32,
            (input.to_bits() >> 32) as u32,
        );
        processor
            .exec_vsqrt_f64(VMovRegParamsf64 {
                dd: DoubleReg::D1,
                dm: DoubleReg::D0,
            })
            .unwrap();

        let result = processor.get_dr(DoubleReg::D1);
        let result_bits = (u64::from(result.1) << 32) | u64::from(result.0);
        let expected = input.sqrt().to_bits();
        assert_eq!(result_bits, expected);
    }

    #[test]
    fn test_vfma_f32() {
        let mut processor = fp_test_processor();

        processor.set_sr(SingleReg::S15, 1.0f32.to_bits());
        processor.set_sr(SingleReg::S13, 2.0f32.to_bits());
        processor.set_sr(SingleReg::S14, 3.0f32.to_bits());

        processor
            .exec_vfma_f32(&VAddSubParamsf32 {
                sd: SingleReg::S15,
                sn: SingleReg::S13,
                sm: SingleReg::S14,
            })
            .unwrap();

        assert_eq!(processor.get_sr(SingleReg::S15), 7.0f32.to_bits());
    }

    #[test]
    fn test_vfma_f64() {
        let mut processor = fp_test_processor();

        let addend = 1.0f64.to_bits();
        let op1 = 2.0f64.to_bits();
        let op2 = 3.0f64.to_bits();

        processor.set_dr(DoubleReg::D7, addend as u32, (addend >> 32) as u32);
        processor.set_dr(DoubleReg::D6, op1 as u32, (op1 >> 32) as u32);
        processor.set_dr(DoubleReg::D5, op2 as u32, (op2 >> 32) as u32);

        processor
            .exec_vfma_f64(&VAddSubParamsf64 {
                dd: DoubleReg::D7,
                dn: DoubleReg::D6,
                dm: DoubleReg::D5,
            })
            .unwrap();

        let result = processor.get_dr(DoubleReg::D7);
        let result_bits = (u64::from(result.1) << 32) | u64::from(result.0);
        assert_eq!(result_bits, 7.0f64.to_bits());
    }

    #[test]
    fn test_vmul_f32() {
        let mut processor = fp_test_processor();

        processor.set_sr(SingleReg::S17, 3.0f32.to_bits());

        processor
            .exec_vmul_f32(&VAddSubParamsf32 {
                sd: SingleReg::S24,
                sn: SingleReg::S17,
                sm: SingleReg::S17,
            })
            .unwrap();

        assert_eq!(processor.get_sr(SingleReg::S24), 9.0f32.to_bits());
    }

    #[test]
    fn test_vnmul_f32() {
        let mut processor = fp_test_processor();

        processor.set_sr(SingleReg::S1, 2.0f32.to_bits());
        processor.set_sr(SingleReg::S2, 3.0f32.to_bits());

        processor
            .exec_vnmul_f32(&VAddSubParamsf32 {
                sd: SingleReg::S0,
                sn: SingleReg::S1,
                sm: SingleReg::S2,
            })
            .unwrap();

        assert_eq!(processor.get_sr(SingleReg::S0), (-6.0f32).to_bits());
    }

    #[test]
    fn test_vdiv_f32() {
        let mut processor = fp_test_processor();

        processor.set_sr(SingleReg::S24, 6.0f32.to_bits());
        processor.set_sr(SingleReg::S16, 2.0f32.to_bits());

        processor
            .exec_vdiv_f32(&VAddSubParamsf32 {
                sd: SingleReg::S26,
                sn: SingleReg::S24,
                sm: SingleReg::S16,
            })
            .unwrap();

        assert_eq!(processor.get_sr(SingleReg::S26), 3.0f32.to_bits());
    }

    #[test]
    fn test_vmul_f64() {
        let mut processor = fp_test_processor();

        let op = 1.5f64.to_bits();
        processor.set_dr(DoubleReg::D9, op as u32, (op >> 32) as u32);

        processor
            .exec_vmul_f64(&VAddSubParamsf64 {
                dd: DoubleReg::D12,
                dn: DoubleReg::D9,
                dm: DoubleReg::D9,
            })
            .unwrap();

        let result = processor.get_dr(DoubleReg::D12);
        let result_bits = (u64::from(result.1) << 32) | u64::from(result.0);
        assert_eq!(result_bits, 2.25f64.to_bits());
    }

    #[test]
    fn test_vnmul_f64() {
        let mut processor = fp_test_processor();

        let op1 = 1.5f64.to_bits();
        let op2 = 2.0f64.to_bits();
        processor.set_dr(DoubleReg::D8, op1 as u32, (op1 >> 32) as u32);
        processor.set_dr(DoubleReg::D9, op2 as u32, (op2 >> 32) as u32);

        processor
            .exec_vnmul_f64(&VAddSubParamsf64 {
                dd: DoubleReg::D7,
                dn: DoubleReg::D8,
                dm: DoubleReg::D9,
            })
            .unwrap();

        let result = processor.get_dr(DoubleReg::D7);
        let result_bits = (u64::from(result.1) << 32) | u64::from(result.0);
        assert_eq!(result_bits, (-3.0f64).to_bits());
    }

    #[test]
    fn test_vdiv_f64() {
        let mut processor = fp_test_processor();

        let numerator = 9.0f64.to_bits();
        let denominator = 4.5f64.to_bits();
        processor.set_dr(DoubleReg::D12, numerator as u32, (numerator >> 32) as u32);
        processor.set_dr(
            DoubleReg::D8,
            denominator as u32,
            (denominator >> 32) as u32,
        );

        processor
            .exec_vdiv_f64(&VAddSubParamsf64 {
                dd: DoubleReg::D13,
                dn: DoubleReg::D12,
                dm: DoubleReg::D8,
            })
            .unwrap();

        let result = processor.get_dr(DoubleReg::D13);
        let result_bits = (u64::from(result.1) << 32) | u64::from(result.0);
        assert_eq!(result_bits, 2.0f64.to_bits());
    }

    #[test]
    fn test_vfms_f32() {
        let mut processor = fp_test_processor();

        processor.set_sr(SingleReg::S15, 10.0f32.to_bits());
        processor.set_sr(SingleReg::S5, 2.0f32.to_bits());
        processor.set_sr(SingleReg::S13, 3.0f32.to_bits());

        processor
            .exec_vfms_f32(&VAddSubParamsf32 {
                sd: SingleReg::S15,
                sn: SingleReg::S5,
                sm: SingleReg::S13,
            })
            .unwrap();

        assert_eq!(processor.get_sr(SingleReg::S15), 4.0f32.to_bits());
    }

    #[test]
    fn test_vfnms_f32() {
        let mut processor = fp_test_processor();

        processor.set_sr(SingleReg::S0, 10.0f32.to_bits());
        processor.set_sr(SingleReg::S5, 2.0f32.to_bits());
        processor.set_sr(SingleReg::S14, 3.0f32.to_bits());

        processor
            .exec_vfnms_f32(&VAddSubParamsf32 {
                sd: SingleReg::S0,
                sn: SingleReg::S5,
                sm: SingleReg::S14,
            })
            .unwrap();

        assert_eq!(processor.get_sr(SingleReg::S0), (-4.0f32).to_bits());
    }

    #[test]
    fn test_vcvt_f64_f32() {
        let mut processor = fp_test_processor();

        processor.set_sr(SingleReg::S7, 1.5f32.to_bits());
        processor
            .exec_vcvt_f64_f32(VCVTParamsF64F32 {
                dd: DoubleReg::D3,
                sm: SingleReg::S7,
            })
            .unwrap();

        let result = processor.get_dr(DoubleReg::D3);
        let result_bits = (u64::from(result.1) << 32) | u64::from(result.0);
        assert_eq!(result_bits, 1.5f64.to_bits());
    }

    #[test]
    fn test_vcvt_f32_f64() {
        let mut processor = fp_test_processor();

        let input = 1.25f64.to_bits();
        processor.set_dr(DoubleReg::D7, input as u32, (input >> 32) as u32);
        processor
            .exec_vcvt_f32_f64(VCVTParamsF32F64 {
                sd: SingleReg::S14,
                dm: DoubleReg::D7,
            })
            .unwrap();

        assert_eq!(processor.get_sr(SingleReg::S14), 1.25f32.to_bits());
    }

    #[test]
    fn test_vsel_f32_gt_selects_sn_when_true() {
        let mut processor = fp_test_processor();
        processor.psr.set_n(0);
        processor.psr.set_v(false);
        processor.psr.set_z(1);

        processor.set_sr(SingleReg::S14, 0x3f80_0000);
        processor.set_sr(SingleReg::S15, 0x4000_0000);

        processor
            .exec_vsel_f32(VSelParamsf32 {
                sd: SingleReg::S0,
                sn: SingleReg::S14,
                sm: SingleReg::S15,
                cond: crate::core::condition::Condition::GT,
            })
            .unwrap();

        assert_eq!(processor.get_sr(SingleReg::S0), 0x3f80_0000);
    }

    #[test]
    fn test_vsel_f32_gt_selects_sm_when_false() {
        let mut processor = fp_test_processor();
        processor.psr.set_n(1);
        processor.psr.set_v(false);
        processor.psr.set_z(0);

        processor.set_sr(SingleReg::S14, 0x3f80_0000);
        processor.set_sr(SingleReg::S15, 0x4000_0000);

        processor
            .exec_vsel_f32(VSelParamsf32 {
                sd: SingleReg::S0,
                sn: SingleReg::S14,
                sm: SingleReg::S15,
                cond: crate::core::condition::Condition::GT,
            })
            .unwrap();

        assert_eq!(processor.get_sr(SingleReg::S0), 0x4000_0000);
    }

    #[test]
    fn test_vcmp_f64_with_zero_sets_fpscr_flags() {
        let mut processor = fp_test_processor();

        processor.set_dr(DoubleReg::D0, 0, 0x4008_0000);
        processor
            .exec_vcmp_f64(&VCmpParamsf64 {
                dd: DoubleReg::D0,
                dm: DoubleReg::D1,
                quiet_nan_exc: false,
                with_zero: true,
            })
            .unwrap();

        assert!(!processor.fpscr.get_n());
        assert!(!processor.fpscr.get_z());
        assert!(processor.fpscr.get_c());
        assert!(!processor.fpscr.get_v());
    }

    #[test]
    fn test_vcmp_f32_with_zero_sets_fpscr_flags() {
        let mut processor = fp_test_processor();

        processor.set_sr(SingleReg::S0, (-1.0f32).to_bits());
        processor
            .exec_vcmp_f32(&VCmpParamsf32 {
                sd: SingleReg::S0,
                sm: SingleReg::S1,
                quiet_nan_exc: false,
                with_zero: true,
            })
            .unwrap();

        assert!(processor.fpscr.get_n());
        assert!(!processor.fpscr.get_z());
        assert!(!processor.fpscr.get_c());
        assert!(!processor.fpscr.get_v());
    }

    #[test]
    fn test_vadd_f32() {
        let mut processor = fp_test_processor();

        processor.set_sr(SingleReg::S2, 1.25f32.to_bits());
        processor.set_sr(SingleReg::S3, 2.5f32.to_bits());

        processor
            .exec_vadd_f32(&VAddSubParamsf32 {
                sd: SingleReg::S4,
                sn: SingleReg::S2,
                sm: SingleReg::S3,
            })
            .unwrap();

        assert_eq!(processor.get_sr(SingleReg::S4), 3.75f32.to_bits());
    }

    #[test]
    fn test_vfms_f64() {
        let mut processor = fp_test_processor();

        let addend = 10.0f64.to_bits();
        let op1 = 2.0f64.to_bits();
        let op2 = 3.0f64.to_bits();
        processor.set_dr(DoubleReg::D4, addend as u32, (addend >> 32) as u32);
        processor.set_dr(DoubleReg::D5, op1 as u32, (op1 >> 32) as u32);
        processor.set_dr(DoubleReg::D6, op2 as u32, (op2 >> 32) as u32);

        processor
            .exec_vfms_f64(&VAddSubParamsf64 {
                dd: DoubleReg::D4,
                dn: DoubleReg::D5,
                dm: DoubleReg::D6,
            })
            .unwrap();

        let result = processor.get_dr(DoubleReg::D4);
        let result_bits = (u64::from(result.1) << 32) | u64::from(result.0);
        assert_eq!(result_bits, 4.0f64.to_bits());
    }

    #[test]
    fn test_vadd_f64() {
        let mut processor = fp_test_processor();

        let lhs = 1.25f64.to_bits();
        let rhs = 2.5f64.to_bits();
        processor.set_dr(DoubleReg::D0, lhs as u32, (lhs >> 32) as u32);
        processor.set_dr(DoubleReg::D1, rhs as u32, (rhs >> 32) as u32);

        processor
            .exec_vadd_f64(&VAddSubParamsf64 {
                dd: DoubleReg::D2,
                dn: DoubleReg::D0,
                dm: DoubleReg::D1,
            })
            .unwrap();

        let result = processor.get_dr(DoubleReg::D2);
        let result_bits = (u64::from(result.1) << 32) | u64::from(result.0);
        assert_eq!(result_bits, 3.75f64.to_bits());
    }

    #[test]
    fn test_vfnms_f64() {
        let mut processor = fp_test_processor();

        let addend = 10.0f64.to_bits();
        let op1 = 2.0f64.to_bits();
        let op2 = 3.0f64.to_bits();
        processor.set_dr(DoubleReg::D0, addend as u32, (addend >> 32) as u32);
        processor.set_dr(DoubleReg::D1, op1 as u32, (op1 >> 32) as u32);
        processor.set_dr(DoubleReg::D2, op2 as u32, (op2 >> 32) as u32);

        processor
            .exec_vfnms_f64(&VAddSubParamsf64 {
                dd: DoubleReg::D0,
                dn: DoubleReg::D1,
                dm: DoubleReg::D2,
            })
            .unwrap();

        let result = processor.get_dr(DoubleReg::D0);
        let result_bits = (u64::from(result.1) << 32) | u64::from(result.0);
        assert_eq!(result_bits, (-4.0f64).to_bits());
    }

    #[test]
    fn test_vsub_f64() {
        let mut processor = fp_test_processor();

        let lhs = 7.25f64.to_bits();
        let rhs = 1.5f64.to_bits();
        processor.set_dr(DoubleReg::D10, lhs as u32, (lhs >> 32) as u32);
        processor.set_dr(DoubleReg::D11, rhs as u32, (rhs >> 32) as u32);

        processor
            .exec_vsub_f64(&VAddSubParamsf64 {
                dd: DoubleReg::D12,
                dn: DoubleReg::D10,
                dm: DoubleReg::D11,
            })
            .unwrap();

        let result = processor.get_dr(DoubleReg::D12);
        let result_bits = (u64::from(result.1) << 32) | u64::from(result.0);
        assert_eq!(result_bits, 5.75f64.to_bits());
    }

    #[test]
    fn test_vsub_f32() {
        let mut processor = fp_test_processor();

        processor.set_sr(SingleReg::S8, 7.25f32.to_bits());
        processor.set_sr(SingleReg::S9, 1.5f32.to_bits());

        processor
            .exec_vsub_f32(&VAddSubParamsf32 {
                sd: SingleReg::S10,
                sn: SingleReg::S8,
                sm: SingleReg::S9,
            })
            .unwrap();

        assert_eq!(processor.get_sr(SingleReg::S10), 5.75f32.to_bits());
    }

    #[test]
    fn test_vcvt_f64_to_s32() {
        let mut processor = fp_test_processor();

        let input = 5.0f64.to_bits();
        processor.set_dr(DoubleReg::D3, input as u32, (input >> 32) as u32);

        processor
            .exec_vcvt(&VCVTParams {
                d: ExtensionReg::Single { reg: SingleReg::S8 },
                m: ExtensionReg::Double { reg: DoubleReg::D3 },
                to_integer: true,
                unsigned: false,
                dp_operation: true,
                rounding: Some(FPRounding::Zero),
            })
            .unwrap();

        assert_eq!(processor.get_sr(SingleReg::S8), 5);
    }

    // vcvt of a single or a double register in place, to or from fixed point.
    fn vcvt_fixed(
        reg: ExtensionReg,
        to_fixed: bool,
        unsigned: bool,
        size: u8,
        fraction_bits: u8,
    ) -> VCVTFixedParams {
        VCVTFixedParams {
            reg,
            to_fixed,
            unsigned,
            size,
            fraction_bits,
        }
    }

    const FIXED_FLOATS: [f64; 14] = [
        0.0,
        -0.0,
        0.5,
        1.5,
        -1.5,
        2.7,
        100.3,
        -100.3,
        40000.5,
        -40000.5,
        1e10,
        -1e10,
        f64::INFINITY,
        f64::NAN,
    ];

    // The result the host gives for a float turned into a fixed-point number of `size` bits,
    // truncating: its `as` casts saturate, and send a NaN to zero.
    fn host_to_fixed(x: f64, size: u8, fraction_bits: u8, unsigned: bool) -> u64 {
        let scaled = (x * 2f64.powi(i32::from(fraction_bits))).trunc();
        match (size, unsigned) {
            (16, false) => scaled as i16 as i64 as u64,
            (16, true) => u64::from(scaled as u16),
            (_, false) => scaled as i32 as i64 as u64,
            (_, true) => u64::from(scaled as u32),
        }
    }

    #[test]
    fn test_vcvt_f32_to_fixed_truncates_and_saturates() {
        for size in [16, 32] {
            for unsigned in [false, true] {
                for fraction_bits in [1, 5, size / 2, size] {
                    for x in FIXED_FLOATS {
                        let reg = ExtensionReg::Single { reg: SingleReg::S6 };
                        let mut processor = fp_test_processor();
                        processor.set_sr(SingleReg::S6, (x as f32).to_bits());
                        processor
                            .exec_vcvt_fixed(&vcvt_fixed(reg, true, unsigned, size, fraction_bits))
                            .unwrap();
                        // The 32-bit register holds the result extended to its width.
                        let expected =
                            host_to_fixed(f64::from(x as f32), size, fraction_bits, unsigned);
                        assert_eq!(
                            processor.get_sr(SingleReg::S6),
                            expected as u32,
                            "{x} size {size} fbits {fraction_bits} unsigned {unsigned}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_vcvt_f64_to_fixed_truncates_and_saturates() {
        for size in [16, 32] {
            for unsigned in [false, true] {
                for fraction_bits in [1, 5, size / 2, size] {
                    for x in FIXED_FLOATS {
                        let reg = ExtensionReg::Double { reg: DoubleReg::D6 };
                        let mut processor = fp_test_processor();
                        processor.set_dr_bits(DoubleReg::D6, x.to_bits());
                        processor
                            .exec_vcvt_fixed(&vcvt_fixed(reg, true, unsigned, size, fraction_bits))
                            .unwrap();
                        // The 64-bit register holds the result extended to its width.
                        assert_eq!(
                            processor.get_dr_bits(DoubleReg::D6),
                            host_to_fixed(x, size, fraction_bits, unsigned),
                            "{x} size {size} fbits {fraction_bits} unsigned {unsigned}"
                        );
                    }
                }
            }
        }
    }

    const FIXED_INTEGERS: [u32; 10] = [
        0,
        1,
        0xFFFF_FFFF,
        0x7FFF,
        0x8000,
        0xFFFF,
        0x1_2345,
        0x7FFF_FFFF,
        0x8000_0000,
        0x8765_4321,
    ];

    // The number the register holds when it is read as fixed point: the low `size` bits, as a
    // signed or an unsigned integer.
    fn host_fixed_value(raw: u32, size: u8, unsigned: bool) -> f64 {
        match (size, unsigned) {
            (16, false) => f64::from(raw as u16 as i16),
            (16, true) => f64::from(raw as u16),
            (_, false) => f64::from(raw as i32),
            (_, true) => f64::from(raw),
        }
    }

    #[test]
    fn test_vcvt_f32_from_fixed_rounds_to_nearest() {
        for size in [16, 32] {
            for unsigned in [false, true] {
                for fraction_bits in [1, 5, size / 2, size] {
                    for raw in FIXED_INTEGERS {
                        let reg = ExtensionReg::Single { reg: SingleReg::S6 };
                        let mut processor = fp_test_processor();
                        processor.set_sr(SingleReg::S6, raw);
                        processor
                            .exec_vcvt_fixed(&vcvt_fixed(reg, false, unsigned, size, fraction_bits))
                            .unwrap();
                        // Exact in a double, and rounded once, to a single.
                        let expected = (host_fixed_value(raw, size, unsigned)
                            / 2f64.powi(i32::from(fraction_bits)))
                            as f32;
                        assert_eq!(
                            processor.get_sr(SingleReg::S6),
                            expected.to_bits(),
                            "{raw:#x} size {size} fbits {fraction_bits} unsigned {unsigned}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_vcvt_f64_from_fixed_reads_the_low_word() {
        for size in [16, 32] {
            for unsigned in [false, true] {
                for fraction_bits in [1, 5, size / 2, size] {
                    for raw in FIXED_INTEGERS {
                        let reg = ExtensionReg::Double { reg: DoubleReg::D6 };
                        let mut processor = fp_test_processor();
                        // The high word is not part of the operand.
                        processor.set_dr(DoubleReg::D6, raw, 0xDEAD_BEEF);
                        processor
                            .exec_vcvt_fixed(&vcvt_fixed(reg, false, unsigned, size, fraction_bits))
                            .unwrap();
                        let expected = host_fixed_value(raw, size, unsigned)
                            / 2f64.powi(i32::from(fraction_bits));
                        assert_eq!(
                            processor.get_dr_bits(DoubleReg::D6),
                            expected.to_bits(),
                            "{raw:#x} size {size} fbits {fraction_bits} unsigned {unsigned}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_vcvt_to_fixed_ignores_the_fpscr_rounding_mode() {
        let mut processor = fp_test_processor();
        processor
            .fpscr
            .set_rounding_mode(FPSCRRounding::RoundTowardsPlusInfinity);
        processor.set_sr(SingleReg::S6, 2.1f32.to_bits());
        processor
            .exec_vcvt_fixed(&vcvt_fixed(
                ExtensionReg::Single { reg: SingleReg::S6 },
                true,
                false,
                32,
                0,
            ))
            .unwrap();
        assert_eq!(processor.get_sr(SingleReg::S6), 2);
    }

    // A signed 16-bit fixed-point result is sign extended into the whole register, and a signed
    // operand is read from the low 16 bits only.
    #[test]
    fn test_vcvt_negative_16_bit_fixed_point_is_sign_extended() {
        let run = |processor: &mut Processor, opcode| {
            processor
                .execute_internal(&crate::decoder::decode_32(opcode))
                .unwrap();
        };

        // vcvt.s16.f32 s0, s0, #4: -1.5 is -24
        let mut processor = fp_test_processor();
        processor.set_sr(SingleReg::S0, (-1.5f32).to_bits());
        run(&mut processor, 0xeebe_0a46);
        assert_eq!(processor.get_sr(SingleReg::S0), 0xffff_ffe8);

        // vcvt.s16.f64 d0, d0, #4
        processor.set_dr_bits(DoubleReg::D0, (-1.5f64).to_bits());
        run(&mut processor, 0xeebe_0b46);
        assert_eq!(processor.get_dr_bits(DoubleReg::D0), 0xffff_ffff_ffff_ffe8);

        // vcvt.f32.s16 s0, s0, #4: the upper half of the register is not part of the operand
        processor.set_sr(SingleReg::S0, 0x1234_ffe8);
        run(&mut processor, 0xeeba_0a46);
        assert_eq!(processor.get_sr(SingleReg::S0), (-1.5f32).to_bits());

        // vcvt.f64.s16 d0, d0, #4
        processor.set_dr(DoubleReg::D0, 0x1234_ffe8, 0xdead_beef);
        run(&mut processor, 0xeeba_0b46);
        assert_eq!(processor.get_dr_bits(DoubleReg::D0), (-1.5f64).to_bits());
    }

    #[test]
    fn test_vcvt_from_fixed_ignores_the_fpscr_rounding_mode() {
        // 2^24 + 1 is half way between two singles; to nearest, ties to even, it is 2^24.
        let mut processor = fp_test_processor();
        processor
            .fpscr
            .set_rounding_mode(FPSCRRounding::RoundTowardsPlusInfinity);
        processor.set_sr(SingleReg::S6, (1 << 24) + 1);
        processor
            .exec_vcvt_fixed(&vcvt_fixed(
                ExtensionReg::Single { reg: SingleReg::S6 },
                false,
                false,
                32,
                0,
            ))
            .unwrap();
        assert_eq!(processor.get_sr(SingleReg::S6), 16_777_216.0f32.to_bits());
    }

    #[test]
    fn test_vsel_f64_selects_false_operand() {
        let mut processor = fp_test_processor();
        processor.psr.set_n(1);
        processor.psr.set_v(false);
        processor.psr.set_z(0);

        processor.set_dr(DoubleReg::D0, 0x1111_2222, 0x3333_4444);
        processor.set_dr(DoubleReg::D1, 0xaaaa_bbbb, 0xcccc_dddd);

        processor
            .exec_vsel_f64(VSelParamsf64 {
                dd: DoubleReg::D2,
                dn: DoubleReg::D0,
                dm: DoubleReg::D1,
                cond: crate::core::condition::Condition::GT,
            })
            .unwrap();

        assert_eq!(processor.get_dr(DoubleReg::D2), (0xaaaa_bbbb, 0xcccc_dddd));
    }

    // The manual negates the first operand of VFMS, so a NaN there comes out with its sign turned,
    // and one in the second operand comes out as it was.
    #[test]
    fn test_vfms_negates_the_first_operand_not_the_second() {
        let mut processor = fp_test_processor();
        let quiet_nan = 0x7FF8_0000_0000_0001u64;
        let other_nan = 0x7FF8_0000_0000_0002u64;
        processor.set_dr_bits(DoubleReg::D4, 1.0f64.to_bits());
        processor.set_dr_bits(DoubleReg::D5, quiet_nan);
        processor.set_dr_bits(DoubleReg::D6, 2.0f64.to_bits());
        let params = VAddSubParamsf64 {
            dd: DoubleReg::D4,
            dn: DoubleReg::D5,
            dm: DoubleReg::D6,
        };
        processor.exec_vfms_f64(&params).unwrap();
        assert_eq!(processor.get_dr_bits(DoubleReg::D4), quiet_nan | 1 << 63);

        processor.set_dr_bits(DoubleReg::D4, 1.0f64.to_bits());
        processor.set_dr_bits(DoubleReg::D5, 2.0f64.to_bits());
        processor.set_dr_bits(DoubleReg::D6, other_nan);
        processor.exec_vfms_f64(&params).unwrap();
        assert_eq!(processor.get_dr_bits(DoubleReg::D4), other_nan);

        // The same in a single.
        processor.set_sr(SingleReg::S4, 1.0f32.to_bits());
        processor.set_sr(SingleReg::S5, 0x7FC0_0001);
        processor.set_sr(SingleReg::S6, 2.0f32.to_bits());
        let params = VAddSubParamsf32 {
            sd: SingleReg::S4,
            sn: SingleReg::S5,
            sm: SingleReg::S6,
        };
        processor.exec_vfms_f32(&params).unwrap();
        assert_eq!(processor.get_sr(SingleReg::S4), 0xFFC0_0001);

        processor.set_sr(SingleReg::S4, 1.0f32.to_bits());
        processor.set_sr(SingleReg::S5, 2.0f32.to_bits());
        processor.set_sr(SingleReg::S6, 0x7FC0_0002);
        processor.exec_vfms_f32(&params).unwrap();
        assert_eq!(processor.get_sr(SingleReg::S4), 0x7FC0_0002);
    }

    #[test]
    fn test_vcvt_between_single_and_double_follows_the_fpscr() {
        let mut processor = fp_test_processor();
        // A signalling NaN is quieted and signals, and a double too large for a single overflows.
        processor.set_sr(SingleReg::S1, 0x7F80_0001);
        processor
            .exec_vcvt_f64_f32(VCVTParamsF64F32 {
                dd: DoubleReg::D2,
                sm: SingleReg::S1,
            })
            .unwrap();
        assert_eq!(processor.get_dr_bits(DoubleReg::D2), 0x7FF8_0000_2000_0000);
        assert_eq!(processor.fpscr & CUMULATIVE_FLAGS, IOC);

        processor.fpscr = RZ;
        processor.set_dr_bits(DoubleReg::D3, 1e300f64.to_bits());
        processor
            .exec_vcvt_f32_f64(VCVTParamsF32F64 {
                sd: SingleReg::S4,
                dm: DoubleReg::D3,
            })
            .unwrap();
        assert_eq!(processor.get_sr(SingleReg::S4), f32::MAX.to_bits());
        assert_eq!(processor.fpscr & CUMULATIVE_FLAGS, OFC | IXC);
    }
}
