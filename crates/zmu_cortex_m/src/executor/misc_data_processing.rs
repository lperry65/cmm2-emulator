use crate::Processor;

use crate::executor::{ExecuteSuccess, ExecutorHelper};

use super::ExecuteResult;

#[cfg(feature = "has-dsp-ext")]
use crate::core::instruction::Reg3NoSetFlagsParams;
use crate::core::{
    bits::Bits,
    instruction::{BfcParams, BfiParams, BfxParams, MovtParams, Reg2RdRmParams},
    operation::sign_extend,
    register::{Apsr, BaseReg},
};

/// Branching operations
pub trait IsaMiscDataProcessing {
    fn exec_bfc(&mut self, params: &BfcParams) -> ExecuteResult;
    fn exec_bfi(&mut self, params: &BfiParams) -> ExecuteResult;

    fn exec_movt(&mut self, params: MovtParams) -> ExecuteResult;

    fn exec_clz(&mut self, params: Reg2RdRmParams) -> ExecuteResult;
    fn exec_rev(&mut self, params: Reg2RdRmParams) -> ExecuteResult;
    fn exec_rbit(&mut self, params: Reg2RdRmParams) -> ExecuteResult;
    fn exec_sat(
        &mut self,
        params: &crate::core::instruction::SatParams,
        signed: bool,
    ) -> ExecuteResult;
    fn exec_rev16(&mut self, params: Reg2RdRmParams) -> ExecuteResult;
    fn exec_revsh(&mut self, params: Reg2RdRmParams) -> ExecuteResult;
    #[cfg(feature = "has-dsp-ext")]
    fn exec_sel(&mut self, params: &Reg3NoSetFlagsParams) -> ExecuteResult;
    fn exec_ubfx(&mut self, params: &BfxParams) -> ExecuteResult;
    fn exec_sbfx(&mut self, params: &BfxParams) -> ExecuteResult;
}

impl IsaMiscDataProcessing for Processor {
    fn exec_bfc(&mut self, params: &BfcParams) -> ExecuteResult {
        if self.condition_passed() {
            if params.msbit >= params.lsbit {
                let destination_upper_range = params.msbit + 1;
                let mut result: u32 = self.get_r(params.rd);
                result.set_bits(params.lsbit..destination_upper_range, 0);
                self.set_r(params.rd, result);
            }
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_bfi(&mut self, params: &BfiParams) -> ExecuteResult {
        if self.condition_passed() {
            let rn: u32 = self.get_r(params.rn);
            let rd = self.get_r(params.rd);

            let msbit = (params.lsbit + params.width) - 1;

            let source_upper_range = (msbit - params.lsbit) + 1;
            let destination_upper_range = msbit + 1;
            let mut result: u32 = rd;
            let value: u32 = rn.get_bits(0..source_upper_range);
            result.set_bits(params.lsbit..destination_upper_range, value);

            self.set_r(params.rd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_movt(&mut self, params: MovtParams) -> ExecuteResult {
        if self.condition_passed() {
            let mut result: u32 = self.get_r(params.rd);
            result.set_bits(16..32, (params.imm16).into());
            self.set_r(params.rd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }

        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_clz(&mut self, params: Reg2RdRmParams) -> ExecuteResult {
        if self.condition_passed() {
            let rm = self.get_r(params.rm);

            self.set_r(params.rd, rm.leading_zeros());

            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_rbit(&mut self, params: Reg2RdRmParams) -> ExecuteResult {
        if self.condition_passed() {
            let rm = self.get_r(params.rm);
            self.set_r(params.rd, rm.reverse_bits());
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_sat(
        &mut self,
        params: &crate::core::instruction::SatParams,
        signed: bool,
    ) -> ExecuteResult {
        if self.condition_passed() {
            let c = self.psr.get_c();
            let operand = crate::core::operation::shift(
                self.get_r(params.rn),
                params.shift_t,
                params.shift_n as usize,
                c,
            ) as i32 as i64;
            let n = u32::from(params.saturate_to);
            let (lo, hi) = if signed {
                (-(1i64 << (n - 1)), (1i64 << (n - 1)) - 1)
            } else {
                (0, (1i64 << n) - 1)
            };
            let result = operand.clamp(lo, hi);
            self.set_r(params.rd, result as u32);
            if result != operand {
                self.psr.set_q(true);
            }
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_rev(&mut self, params: Reg2RdRmParams) -> ExecuteResult {
        if self.condition_passed() {
            let rm = self.get_r(params.rm);
            self.set_r(
                params.rd,
                ((rm & 0xff) << 24)
                    + ((rm & 0xff00) << 8)
                    + ((rm & 0xff_0000) >> 8)
                    + ((rm & 0xff00_0000) >> 24),
            );
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_rev16(&mut self, params: Reg2RdRmParams) -> ExecuteResult {
        if self.condition_passed() {
            let rm = self.get_r(params.rm);
            self.set_r(
                params.rd,
                ((rm & 0xff) << 8)
                    + ((rm & 0xff00) >> 8)
                    + ((rm & 0xff_0000) << 8)
                    + ((rm & 0xff00_0000) >> 8),
            );
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_revsh(&mut self, params: Reg2RdRmParams) -> ExecuteResult {
        if self.condition_passed() {
            let rm = self.get_r(params.rm);
            self.set_r(
                params.rd,
                ((sign_extend(rm & 0xff, 7, 24) as u32) << 8) + ((rm & 0xff00) >> 8),
            );
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    #[cfg(feature = "has-dsp-ext")]
    fn exec_sel(&mut self, params: &Reg3NoSetFlagsParams) -> ExecuteResult {
        if self.condition_passed() {
            let rm = self.get_r(params.rm);
            let rn = self.get_r(params.rn);

            let mut result = 0;
            result.set_bits(
                0..8,
                if self.psr.get_ge0() {
                    rn.get_bits(0..8)
                } else {
                    rm.get_bits(0..8)
                },
            );
            result.set_bits(
                8..16,
                if self.psr.get_ge1() {
                    rn.get_bits(8..16)
                } else {
                    rm.get_bits(8..16)
                },
            );
            result.set_bits(
                16..24,
                if self.psr.get_ge2() {
                    rn.get_bits(16..24)
                } else {
                    rm.get_bits(16..24)
                },
            );
            result.set_bits(
                24..32,
                if self.psr.get_ge3() {
                    rn.get_bits(24..32)
                } else {
                    rm.get_bits(24..32)
                },
            );
            self.set_r(params.rd, result);

            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_ubfx(&mut self, params: &BfxParams) -> ExecuteResult {
        if self.condition_passed() {
            let msbit = params.lsb + params.widthminus1;
            if msbit <= 31 {
                let upper = msbit + 1;
                let data = self.get_r(params.rn).get_bits(params.lsb..upper);
                self.set_r(params.rd, data);
            } else {
                todo!();
            }

            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_sbfx(&mut self, params: &BfxParams) -> ExecuteResult {
        if self.condition_passed() {
            let msbit = params.lsb + params.widthminus1;
            if msbit <= 31 {
                let upper = msbit + 1;
                let data = self.get_r(params.rn).get_bits(params.lsb..upper);
                let data = sign_extend(data, msbit, 32) as i32;
                self.set_r(params.rd, data as u32);
            } else {
                todo!();
            }

            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::instruction::{Instruction, SRType, SatParams};
    use crate::core::register::Reg;
    #[test]
    fn test_bfi() {
        // arrange
        let mut core = Processor::new();
        core.psr.value = 0;

        core.set_r(Reg::R2, 0x1122_3344);
        core.set_r(Reg::R3, 0xaabb_ccdd);
        core.psr.value = 0;

        let instruction = Instruction::BFI {
            params: BfiParams {
                rd: Reg::R2,
                rn: Reg::R3,
                lsbit: 0,
                width: 8,
            },
        };

        core.execute_internal(&instruction).unwrap();

        assert_eq!(core.get_r(Reg::R3), 0xaabb_ccdd);
        assert_eq!(core.get_r(Reg::R2), 0x1122_33dd);
    }

    #[test]
    fn test_bfi_with_shift_8() {
        // arrange
        let mut core = Processor::new();
        core.psr.value = 0;

        core.set_r(Reg::R0, 0);
        core.set_r(Reg::R1, 0x00e0_00e4);

        let instruction = Instruction::BFI {
            params: BfiParams {
                rd: Reg::R0,
                rn: Reg::R1,
                lsbit: 8,
                width: 24,
            },
        };

        core.execute_internal(&instruction).unwrap();

        assert_eq!(core.get_r(Reg::R0), 0xe000_e400);
        assert_eq!(core.get_r(Reg::R1), 0x00e0_00e4);
    }

    #[test]
    fn test_rbit() {
        let mut core = Processor::new();
        core.set_r(Reg::R1, 0x0000_0001);

        core.execute_internal(&Instruction::RBIT {
            params: Reg2RdRmParams {
                rd: Reg::R2,
                rm: Reg::R1,
            },
        })
        .unwrap();

        assert_eq!(core.get_r(Reg::R2), 0x8000_0000);
    }

    fn sat(signed: bool, saturate_to: u8, value: i32) -> (u32, bool) {
        let mut core = Processor::new();
        core.set_r(Reg::R1, value as u32);
        let params = SatParams {
            rd: Reg::R2,
            rn: Reg::R1,
            saturate_to,
            shift_t: SRType::LSL,
            shift_n: 0,
        };
        let instruction = if signed {
            Instruction::SSAT { params }
        } else {
            Instruction::USAT { params }
        };
        core.execute_internal(&instruction).unwrap();
        (core.get_r(Reg::R2), core.psr.get_q())
    }

    #[test]
    fn test_ssat() {
        assert_eq!(sat(true, 8, 100), (100, false));
        assert_eq!(sat(true, 8, 200), (127, true));
        assert_eq!(sat(true, 8, -200), (-128i32 as u32, true));
    }

    #[test]
    fn test_usat() {
        assert_eq!(sat(false, 8, 100), (100, false));
        assert_eq!(sat(false, 8, 300), (255, true));
        assert_eq!(sat(false, 8, -5), (0, true));
    }

    #[test]
    fn test_rev_w_variants() {
        let mut core = Processor::new();
        core.set_r(Reg::R1, 0x1122_33f4);
        let params = Reg2RdRmParams {
            rd: Reg::R2,
            rm: Reg::R1,
        };

        core.execute_internal(&Instruction::REV {
            params,
            thumb32: true,
        })
        .unwrap();
        assert_eq!(core.get_r(Reg::R2), 0xf433_2211);
        core.execute_internal(&Instruction::REV16 {
            params,
            thumb32: true,
        })
        .unwrap();
        assert_eq!(core.get_r(Reg::R2), 0x2211_f433);
        core.execute_internal(&Instruction::REVSH {
            params,
            thumb32: true,
        })
        .unwrap();
        assert_eq!(core.get_r(Reg::R2), 0xffff_f433);
    }

    #[cfg(not(feature = "has-dsp-ext"))]
    #[test]
    fn test_sel_without_dsp_is_undef() {
        use crate::core::fault::Fault;
        use crate::core::instruction::Reg3NoSetFlagsParams;
        let mut core = Processor::new();
        let instruction = Instruction::SEL {
            params: Reg3NoSetFlagsParams {
                rd: Reg::R0,
                rn: Reg::R1,
                rm: Reg::R2,
            },
        };
        let result = core.execute_internal(&instruction);
        assert_eq!(result, Err(Fault::UndefInstr));
    }
}
