use crate::Processor;

use crate::executor::{ExecuteSuccess, ExecutorHelper};

use super::ExecuteResult;
use crate::core::instruction::Reg3NoSetFlagsParams;
use crate::core::{fault::Fault, register::BaseReg};

/// Divide operations
pub trait IsaDivide {
    fn exec_udiv(&mut self, params: &Reg3NoSetFlagsParams) -> ExecuteResult;
    fn exec_sdiv(&mut self, params: &Reg3NoSetFlagsParams) -> ExecuteResult;
}

impl IsaDivide for Processor {
    fn exec_sdiv(&mut self, params: &Reg3NoSetFlagsParams) -> ExecuteResult {
        if self.condition_passed() {
            let rm = self.get_r(params.rm);
            let result = if rm == 0 {
                if self.integer_zero_divide_trapping_enabled() {
                    return Err(Fault::DivByZero);
                }
                0
            } else {
                let rn = self.get_r(params.rn);
                (rn as i32).wrapping_div(rm as i32)
            };
            self.set_r(params.rd, result as u32);
            return Ok(ExecuteSuccess::Taken { cycles: 2 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_udiv(&mut self, params: &Reg3NoSetFlagsParams) -> ExecuteResult {
        if self.condition_passed() {
            let rm = self.get_r(params.rm);
            if rm == 0 && self.integer_zero_divide_trapping_enabled() {
                return Err(Fault::DivByZero);
            }
            let result = self.get_r(params.rn).checked_div(rm).unwrap_or(0);
            self.set_r(params.rd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 2 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{instruction::Instruction, register::Reg};

    #[test]
    fn test_udiv() {
        // arrange
        let mut core = Processor::new();
        core.set_r(Reg::R0, 0x7d0);
        core.set_r(Reg::R1, 0x3);
        core.psr.value = 0;

        let instruction = Instruction::UDIV {
            params: Reg3NoSetFlagsParams {
                rd: Reg::R0,
                rn: Reg::R0,
                rm: Reg::R1,
            },
        };

        // act
        let result = core.execute_internal(&instruction);

        assert_eq!(result, Ok(ExecuteSuccess::Taken { cycles: 2 }));

        assert_eq!(core.get_r(Reg::R0), 0x29a);
        assert_eq!(core.get_r(Reg::R1), 0x3);
    }

    #[test]
    fn test_sdiv_min_by_minus_one_does_not_panic() {
        // i32::MIN / -1 overflows in Rust and would panic; the architecture says the result is
        // i32::MIN.
        let mut core = Processor::new();
        core.psr.value = 0;
        core.set_r(Reg::R0, 0x8000_0000);
        core.set_r(Reg::R1, 0xFFFF_FFFF);
        let instruction = Instruction::SDIV {
            params: Reg3NoSetFlagsParams {
                rd: Reg::R2,
                rn: Reg::R0,
                rm: Reg::R1,
            },
        };
        core.execute_internal(&instruction).unwrap();
        assert_eq!(core.get_r(Reg::R2), 0x8000_0000);
    }

    #[test]
    fn test_sdiv_signed() {
        let mut core = Processor::new();
        core.psr.value = 0;
        core.set_r(Reg::R0, (-7i32) as u32);
        core.set_r(Reg::R1, 2);
        let instruction = Instruction::SDIV {
            params: Reg3NoSetFlagsParams {
                rd: Reg::R2,
                rn: Reg::R0,
                rm: Reg::R1,
            },
        };
        core.execute_internal(&instruction).unwrap();
        assert_eq!(core.get_r(Reg::R2) as i32, -3);
    }

    #[test]
    fn test_div_by_zero_follows_ccr_div_0_trp() {
        let mut core = Processor::new();
        core.set_r(Reg::R0, 10);
        core.set_r(Reg::R1, 0);
        let params = Reg3NoSetFlagsParams {
            rd: Reg::R2,
            rn: Reg::R0,
            rm: Reg::R1,
        };

        // CCR.DIV_0_TRP clear: the result is 0
        core.set_r(Reg::R2, 0xff);
        assert_eq!(
            core.exec_udiv(&params),
            Ok(ExecuteSuccess::Taken { cycles: 2 })
        );
        assert_eq!(core.get_r(Reg::R2), 0);
        core.set_r(Reg::R2, 0xff);
        assert_eq!(
            core.exec_sdiv(&params),
            Ok(ExecuteSuccess::Taken { cycles: 2 })
        );
        assert_eq!(core.get_r(Reg::R2), 0);

        // CCR.DIV_0_TRP set: usage fault
        core.ccr = 1 << 4;
        assert_eq!(core.exec_udiv(&params), Err(Fault::DivByZero));
        assert_eq!(core.exec_sdiv(&params), Err(Fault::DivByZero));
    }
}
