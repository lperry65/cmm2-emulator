use crate::Processor;

use crate::executor::{ExecuteSuccess, ExecutorHelper};

use super::ExecuteResult;
use crate::core::instruction::{Reg2UsizeParams, Reg3ShiftParams, Reg3UsizeParams};
use crate::core::{
    bits::Bits,
    operation::{ror, shift, sign_extend},
    register::{Apsr, BaseReg},
};

/// Multiply operations
pub trait IsaPacking {
    fn exec_sxtb(&mut self, params: &Reg2UsizeParams) -> ExecuteResult;
    fn exec_sxth(&mut self, params: &Reg2UsizeParams) -> ExecuteResult;
    fn exec_uxtb(&mut self, params: &Reg2UsizeParams) -> ExecuteResult;
    fn exec_uxth(&mut self, params: &Reg2UsizeParams) -> ExecuteResult;
    fn exec_uxtab(&mut self, params: &Reg3UsizeParams) -> ExecuteResult;
    fn exec_uxtah(&mut self, params: &Reg3UsizeParams) -> ExecuteResult;
    fn exec_sxtab(&mut self, params: &Reg3UsizeParams) -> ExecuteResult;
    fn exec_sxtah(&mut self, params: &Reg3UsizeParams) -> ExecuteResult;
    fn exec_pkh(&mut self, params: &Reg3ShiftParams, tb_form: bool) -> ExecuteResult;
}

impl Processor {
    // Rd = Rn + extend(Rm rotated), where `extend` takes the byte or halfword the instruction
    // names out of the rotated value and extends it to 32 bits.
    fn extend_and_add(
        &mut self,
        params: &Reg3UsizeParams,
        extend: fn(u32) -> u32,
    ) -> ExecuteResult {
        if self.condition_passed() {
            let rotated = ror(self.get_r(params.rm), params.rotation);
            let result = self.get_r(params.rn).wrapping_add(extend(rotated));
            self.set_r(params.rd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }
}

impl IsaPacking for Processor {
    fn exec_sxtb(&mut self, params: &Reg2UsizeParams) -> ExecuteResult {
        if self.condition_passed() {
            let rm = self.get_r(params.rm);
            let rotated = ror(rm, params.rotation);
            let result = sign_extend(rotated.get_bits(0..8), 7, 32) as u32;
            self.set_r(params.rd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_sxth(&mut self, params: &Reg2UsizeParams) -> ExecuteResult {
        if self.condition_passed() {
            let rm = self.get_r(params.rm);
            let rotated = ror(rm, params.rotation);
            let result = sign_extend(rotated.get_bits(0..16), 15, 32) as u32;
            self.set_r(params.rd, result);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_uxtb(&mut self, params: &Reg2UsizeParams) -> ExecuteResult {
        if self.condition_passed() {
            let rm = self.get_r(params.rm);
            let rotated = ror(rm, params.rotation);
            self.set_r(params.rd, rotated.get_bits(0..8));
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_uxth(&mut self, params: &Reg2UsizeParams) -> ExecuteResult {
        if self.condition_passed() {
            let rm = self.get_r(params.rm);
            let rotated = ror(rm, params.rotation);
            self.set_r(params.rd, rotated.get_bits(0..16));
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_uxtab(&mut self, params: &Reg3UsizeParams) -> ExecuteResult {
        self.extend_and_add(params, |rotated| rotated.get_bits(0..8))
    }

    fn exec_uxtah(&mut self, params: &Reg3UsizeParams) -> ExecuteResult {
        self.extend_and_add(params, |rotated| rotated.get_bits(0..16))
    }

    fn exec_sxtab(&mut self, params: &Reg3UsizeParams) -> ExecuteResult {
        self.extend_and_add(params, |rotated| {
            sign_extend(rotated.get_bits(0..8), 7, 32) as u32
        })
    }

    fn exec_sxtah(&mut self, params: &Reg3UsizeParams) -> ExecuteResult {
        self.extend_and_add(params, |rotated| {
            sign_extend(rotated.get_bits(0..16), 15, 32) as u32
        })
    }

    fn exec_pkh(&mut self, params: &Reg3ShiftParams, tb_form: bool) -> ExecuteResult {
        if self.condition_passed() {
            let rn = self.get_r(params.rn);
            let operand2 = shift(
                self.get_r(params.rm),
                params.shift_t,
                usize::from(params.shift_n),
                self.psr.get_c(),
            );
            // PKHTB keeps the top half of Rn and takes the bottom half of the operand, PKHBT
            // the other way round.
            let (top, bottom) = if tb_form {
                (rn, operand2)
            } else {
                (operand2, rn)
            };
            self.set_r(params.rd, top & 0xFFFF_0000 | bottom & 0x0000_FFFF);
            return Ok(ExecuteSuccess::Taken { cycles: 1 });
        }
        Ok(ExecuteSuccess::NotTaken)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::instruction::{Reg3ShiftParams, SRType, SetFlags};
    use crate::core::register::Reg;

    fn extend_params(rotation: usize) -> Reg3UsizeParams {
        Reg3UsizeParams {
            rd: Reg::R0,
            rn: Reg::R1,
            rm: Reg::R2,
            rotation,
        }
    }

    // Runs an extend-and-add with Rn = r1 and Rm = r2 and returns r0.
    fn extend_add(
        exec: fn(&mut Processor, &Reg3UsizeParams) -> ExecuteResult,
        rotation: usize,
        rn: u32,
        rm: u32,
    ) -> u32 {
        let mut processor = Processor::new();
        processor.set_r(Reg::R1, rn);
        processor.set_r(Reg::R2, rm);
        exec(&mut processor, &extend_params(rotation)).unwrap();
        processor.get_r(Reg::R0)
    }

    #[test]
    fn uxtah_adds_the_zero_extended_halfword() {
        let exec = Processor::exec_uxtah;
        assert_eq!(extend_add(exec, 0, 0x1000_0000, 0xDEAD_BEEF), 0x1000_BEEF);
        assert_eq!(extend_add(exec, 8, 5, 0x00AB_CD12), 5 + 0xABCD);
        assert_eq!(extend_add(exec, 16, 5, 0xDEAD_BEEF), 5 + 0xDEAD);
        assert_eq!(extend_add(exec, 24, 5, 0x1234_5678), 5 + 0x7812);
        // The sum wraps.
        assert_eq!(extend_add(exec, 0, 0xFFFF_FFFF, 2), 1);
    }

    #[test]
    fn sxtah_adds_the_sign_extended_halfword() {
        let exec = Processor::exec_sxtah;
        assert_eq!(extend_add(exec, 0, 0x1000_0000, 0xDEAD_BEEF), 0x0FFF_BEEF);
        assert_eq!(extend_add(exec, 0, 100, 0x0000_7FFF), 100 + 0x7FFF);
        assert_eq!(
            extend_add(exec, 0, 100, 0x0000_8000),
            100u32.wrapping_sub(0x8000)
        );
        assert_eq!(
            extend_add(exec, 8, 100, 0x0080_0000),
            100u32.wrapping_sub(0x8000)
        );
        assert_eq!(extend_add(exec, 16, 0, 0xFFFE_0000), 0xFFFF_FFFE);
        assert_eq!(extend_add(exec, 24, 7, 0x0000_FF12), 7 + 0x1200);
    }

    #[test]
    fn sxtab_adds_the_sign_extended_byte() {
        let exec = Processor::exec_sxtab;
        assert_eq!(extend_add(exec, 0, 100, 0xFFFF_FF7F), 100 + 0x7F);
        assert_eq!(
            extend_add(exec, 0, 100, 0xFFFF_FF80),
            100u32.wrapping_sub(0x80)
        );
        assert_eq!(
            extend_add(exec, 8, 100, 0x0000_8000),
            100u32.wrapping_sub(0x80)
        );
        assert_eq!(extend_add(exec, 16, 100, 0x0001_0000), 101);
        assert_eq!(extend_add(exec, 24, 0, 0xFF00_0000), 0xFFFF_FFFF);
    }

    #[test]
    fn uxtab_adds_the_zero_extended_byte() {
        let exec = Processor::exec_uxtab;
        assert_eq!(extend_add(exec, 0, 100, 0xFFFF_FF80), 100 + 0x80);
        assert_eq!(extend_add(exec, 8, 100, 0x0000_8000), 100 + 0x80);
        assert_eq!(extend_add(exec, 16, 100, 0x00AB_0000), 100 + 0xAB);
        assert_eq!(extend_add(exec, 24, 100, 0xCD00_0000), 100 + 0xCD);
    }

    #[test]
    fn the_extend_and_adds_obey_the_it_condition() {
        // In an IT NE block with Z set the instruction is skipped and r0 stays as it was.
        let execs: [fn(&mut Processor, &Reg3UsizeParams) -> ExecuteResult; 4] = [
            Processor::exec_uxtah,
            Processor::exec_sxtah,
            Processor::exec_uxtab,
            Processor::exec_sxtab,
        ];
        for exec in execs {
            let mut processor = Processor::new();
            processor.set_r(Reg::R0, 0x55);
            processor.psr.set_z_bit(true);
            processor
                .set_itstate(((crate::core::condition::Condition::NE.value() as u8) << 4) | 0b1000);
            let outcome = exec(&mut processor, &extend_params(0)).unwrap();
            assert!(matches!(outcome, ExecuteSuccess::NotTaken));
            assert_eq!(processor.get_r(Reg::R0), 0x55);
        }
    }

    // Runs a pack halfword with Rn = r1 and Rm = r2 and returns r0.
    fn pack(tb_form: bool, shift_t: SRType, shift_n: u8, rn: u32, rm: u32) -> u32 {
        let mut processor = Processor::new();
        processor.set_r(Reg::R1, rn);
        processor.set_r(Reg::R2, rm);
        let params = Reg3ShiftParams {
            rd: Reg::R0,
            rn: Reg::R1,
            rm: Reg::R2,
            shift_t,
            shift_n,
            setflags: SetFlags::False,
        };
        processor.exec_pkh(&params, tb_form).unwrap();
        processor.get_r(Reg::R0)
    }

    #[test]
    fn pkhbt_takes_the_bottom_of_rn_and_the_top_of_the_shifted_rm() {
        assert_eq!(
            pack(false, SRType::LSL, 0, 0x1111_2222, 0x3333_4444),
            0x3333_2222
        );
        // Rm shifted left by 8 is 0x3344_4400, whose top half is 0x3344.
        assert_eq!(
            pack(false, SRType::LSL, 8, 0x1111_2222, 0x3333_4444),
            0x3344_2222
        );
        assert_eq!(
            pack(false, SRType::LSL, 31, 0xFFFF_FFFF, 0x0000_0001),
            0x8000_FFFF
        );
    }

    #[test]
    fn pkhtb_takes_the_top_of_rn_and_the_bottom_of_the_arithmetically_shifted_rm() {
        assert_eq!(
            pack(true, SRType::ASR, 0, 0x1111_2222, 0x3333_4444),
            0x1111_4444
        );
        // Rm shifted right by 8 is 0x0033_3344.
        assert_eq!(
            pack(true, SRType::ASR, 8, 0x1111_2222, 0x3333_4444),
            0x1111_3344
        );
        // A shift of 32 is what the encoding's 0 means, and gives the sign: all ones or zeros.
        assert_eq!(
            pack(true, SRType::ASR, 32, 0x1111_2222, 0x8000_0000),
            0x1111_FFFF
        );
        assert_eq!(
            pack(true, SRType::ASR, 32, 0x1111_2222, 0x7FFF_FFFF),
            0x1111_0000
        );
    }
}
