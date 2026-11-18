use super::ExecuteResult;
use super::load_and_store::require_aligned;
use crate::Processor;
use crate::executor::{ExecuteSuccess, ExecutorHelper};

use crate::core::{
    instruction::LoadAndStoreMultipleParams,
    register::{BaseReg, Reg},
};
use enum_set::EnumSet;

/// Load and Store operations
pub trait IsaLoadAndStoreMultiple {
    fn exec_stm(&mut self, params: &LoadAndStoreMultipleParams) -> ExecuteResult;
    fn exec_stmdb(&mut self, params: &LoadAndStoreMultipleParams) -> ExecuteResult;
    fn exec_ldm(&mut self, params: &LoadAndStoreMultipleParams) -> ExecuteResult;
    fn exec_ldmdb(&mut self, params: &LoadAndStoreMultipleParams) -> ExecuteResult;
    /// `unaligned_allowed` is the single-register 32-bit form (STR/LDR with write back,
    /// A7.7.99 T3, A7.7.101 T3); the others need a word-aligned stack (A3.2.1).
    fn exec_push(&mut self, registers: EnumSet<Reg>, unaligned_allowed: bool) -> ExecuteResult;
    fn exec_pop(&mut self, registers: EnumSet<Reg>, unaligned_allowed: bool) -> ExecuteResult;
}

impl IsaLoadAndStoreMultiple for Processor {
    fn exec_stm(&mut self, params: &LoadAndStoreMultipleParams) -> ExecuteResult {
        if self.condition_passed() {
            let regs_size = 4 * (params.registers.len() as u32);

            let mut address = self.get_r(params.rn);

            require_aligned(address, 3)?;

            for reg in &params.registers {
                let r = self.get_r(reg);
                self.write32_data(address, r)?;
                address = address.wrapping_add(4);
            }

            if params.wback {
                self.add_r(params.rn, regs_size);
            }
            return Ok(ExecuteSuccess::Taken {
                cycles: 1 + params.registers.len() as u32,
            });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_stmdb(&mut self, params: &LoadAndStoreMultipleParams) -> ExecuteResult {
        if self.condition_passed() {
            let regs_size = 4 * (params.registers.len() as u32);

            let mut address = self.get_r(params.rn).wrapping_sub(regs_size);

            require_aligned(address, 3)?;

            for reg in &params.registers {
                let r = self.get_r(reg);
                self.write32_data(address, r)?;
                address = address.wrapping_add(4);
            }

            if params.wback {
                self.sub_r(params.rn, regs_size);
            }
            return Ok(ExecuteSuccess::Taken {
                cycles: 1 + params.registers.len() as u32,
            });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_ldm(&mut self, params: &LoadAndStoreMultipleParams) -> ExecuteResult {
        if self.condition_passed() {
            let regs_size = 4 * (params.registers.len() as u32);

            let mut address = self.get_r(params.rn);

            require_aligned(address, 3)?;

            let mut branched = false;
            for reg in &params.registers {
                let value = self.read32_data(address)?;
                if reg == Reg::PC {
                    self.load_write_pc(value)?;
                    branched = true;
                } else {
                    self.set_r(reg, value);
                }
                address = address.wrapping_add(4);
            }

            if params.wback && !params.registers.contains(&params.rn) {
                self.add_r(params.rn, regs_size);
            }
            let cc = 1 + params.registers.len() as u32;
            if branched {
                return Ok(ExecuteSuccess::Branched { cycles: cc });
            }
            return Ok(ExecuteSuccess::Taken { cycles: cc });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    /// LDMDB (A7.7.42): the words just below the base, lowest register from the lowest address.
    /// Every word is read before anything is written, so a fault, which is precise, leaves the
    /// base and every other register as they were. Write back comes before the load of PC, so an
    /// exception return with SP as the base sees the new SP, as a POP does.
    fn exec_ldmdb(&mut self, params: &LoadAndStoreMultipleParams) -> ExecuteResult {
        if !self.condition_passed() {
            return Ok(ExecuteSuccess::NotTaken);
        }
        let regs_size = 4 * (params.registers.len() as u32);
        let base = self.get_r(params.rn);
        let mut address = base.wrapping_sub(regs_size);
        require_aligned(address, 3)?;

        // at most 14 registers: no SP, and PC and LR are not both there
        let mut loaded = [(Reg::R0, 0u32); 15];
        let mut count = 0;
        for reg in &params.registers {
            loaded[count] = (reg, self.read32_data(address)?);
            count += 1;
            address = address.wrapping_add(4);
        }

        for &(reg, value) in &loaded[..count] {
            if reg != Reg::PC {
                self.set_r(reg, value);
            }
        }
        if params.wback {
            self.set_r(params.rn, base.wrapping_sub(regs_size));
        }
        let cycles = 1 + params.registers.len() as u32;
        if params.registers.contains(&Reg::PC) {
            self.load_write_pc(loaded[count - 1].1)?;
            // the refill of a PC load, as for POP
            return Ok(ExecuteSuccess::Branched { cycles: cycles + 3 });
        }
        Ok(ExecuteSuccess::Taken { cycles })
    }

    fn exec_push(&mut self, registers: EnumSet<Reg>, unaligned_allowed: bool) -> ExecuteResult {
        if self.condition_passed() {
            let regs_size = 4 * (registers.len() as u32);
            let sp = self.get_r(Reg::SP);
            let mut address = sp.wrapping_sub(regs_size);
            if !unaligned_allowed {
                require_aligned(address, 3)?;
            }

            for reg in &registers {
                let value = self.get_r(reg);
                self.write32_data(address, value)?;
                address = address.wrapping_add(4);
            }

            self.set_r(Reg::SP, sp.wrapping_sub(regs_size));
            return Ok(ExecuteSuccess::Taken {
                cycles: 1 + registers.len() as u32,
            });
        }
        Ok(ExecuteSuccess::NotTaken)
    }

    fn exec_pop(&mut self, registers: EnumSet<Reg>, unaligned_allowed: bool) -> ExecuteResult {
        if self.condition_passed() {
            let regs_size = 4 * (registers.len() as u32);
            let sp = self.get_r(Reg::SP);
            let mut address = sp;
            if unaligned_allowed {
                // LDR with write back: only the UNALIGN_TRP rule, and before SP moves
                self.trap_unaligned(address, 3)?;
            } else {
                require_aligned(address, 3)?;
            }

            self.set_r(Reg::SP, sp.wrapping_add(regs_size));

            for reg in &registers {
                let val = match self.read32_data(address) {
                    Ok(val) => val,
                    Err(fault) => {
                        // the fault is precise: SP is where it was
                        self.set_r(Reg::SP, sp);
                        return Err(fault);
                    }
                };
                if reg == Reg::PC {
                    self.bx_write_pc(val)?;
                } else {
                    self.set_r(reg, val);
                }
                address = address.wrapping_add(4);
            }

            if registers.contains(&Reg::PC) {
                return Ok(ExecuteSuccess::Branched {
                    cycles: 4 + registers.len() as u32,
                });
            } else {
                return Ok(ExecuteSuccess::Taken {
                    cycles: 1 + registers.len() as u32,
                });
            }
        }
        Ok(ExecuteSuccess::NotTaken)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::Bus;
    use crate::core::{instruction::LoadAndStoreMultipleParams, operation::get_reglist};

    #[test]
    fn test_exec_ldm_with_writeback_updates_base_register() {
        let mut core = Processor::new();
        let base = 0x2000_0000;

        core.write32(base, 0x1111_1111).unwrap();
        core.write32(base + 4, 0x2222_2222).unwrap();
        core.set_r(Reg::R2, base);

        core.exec_ldm(&LoadAndStoreMultipleParams {
            registers: get_reglist(0b0000_0011),
            rn: Reg::R2,
            wback: true,
        })
        .unwrap();

        assert_eq!(core.get_r(Reg::R0), 0x1111_1111);
        assert_eq!(core.get_r(Reg::R1), 0x2222_2222);
        assert_eq!(core.get_r(Reg::R2), base + 8);
    }

    #[test]
    fn test_exec_ldm_without_writeback_preserves_base_register() {
        let mut core = Processor::new();
        let base = 0x2000_0000;

        core.write32(base, 0x3333_3333).unwrap();
        core.write32(base + 4, 0xCCCC_CCCC).unwrap();
        core.set_r(Reg::R1, base);

        core.exec_ldm(&LoadAndStoreMultipleParams {
            registers: get_reglist(0x1008),
            rn: Reg::R1,
            wback: false,
        })
        .unwrap();

        assert_eq!(core.get_r(Reg::R3), 0x3333_3333);
        assert_eq!(core.get_r(Reg::R12), 0xCCCC_CCCC);
        assert_eq!(core.get_r(Reg::R1), base);
    }
}
