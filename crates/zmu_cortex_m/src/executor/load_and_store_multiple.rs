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

/// The cycles a load of PC adds to the 1 + N of a load multiple: the refill of the pipeline (P).
/// ARM publishes no timings for the Cortex-M7; the M4 TRM gives 1 + N and adds P, 1 to 3, for a PC
/// load. LDM, LDMDB and POP all count P = 3.
const PC_LOAD_REFILL_CYCLES: u32 = 3;

impl Processor {
    /// LDM and LDMDB: loads the words from `start` up, one per register, and writes `new_base`
    /// back if the instruction writes back. Every word is read before anything is written, so a
    /// fault, which is precise, leaves the base and every other register as they were. The write
    /// back comes before the load of PC: that can be an exception return, which unstacks the
    /// frame from the SP the instruction leaves and puts back registers (the base among them) that
    /// the write back must not touch afterwards, as POP's pseudocode has it (A7.7.99) and real
    /// cores do. A7.7.41 and A7.7.42 literally write back after `LoadWritePC`.
    fn load_multiple(
        &mut self,
        params: &LoadAndStoreMultipleParams,
        start: u32,
        new_base: u32,
    ) -> ExecuteResult {
        let mut address = start;
        require_aligned(address, 3)?;

        // at most 15 registers: no SP in a 32-bit list
        let mut loaded = [(Reg::R0, 0u32); 16];
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
        // The decoders refuse write back with the base in the list (UNPREDICTABLE), so the base
        // is not in `loaded` when this runs.
        if params.wback {
            self.set_r(params.rn, new_base);
        }
        let cycles = 1 + count as u32;
        if params.registers.contains(&Reg::PC) {
            self.load_write_pc(loaded[count - 1].1)?;
            return Ok(ExecuteSuccess::Branched {
                cycles: cycles + PC_LOAD_REFILL_CYCLES,
            });
        }
        Ok(ExecuteSuccess::Taken { cycles })
    }
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
        if !self.condition_passed() {
            return Ok(ExecuteSuccess::NotTaken);
        }
        let base = self.get_r(params.rn);
        let new_base = base.wrapping_add(4 * params.registers.len() as u32);
        self.load_multiple(params, base, new_base)
    }

    /// LDMDB (A7.7.42): the words just below the base, lowest register from the lowest address.
    fn exec_ldmdb(&mut self, params: &LoadAndStoreMultipleParams) -> ExecuteResult {
        if !self.condition_passed() {
            return Ok(ExecuteSuccess::NotTaken);
        }
        let base = self.get_r(params.rn);
        let new_base = base.wrapping_sub(4 * params.registers.len() as u32);
        self.load_multiple(params, new_base, new_base)
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
                    cycles: 1 + registers.len() as u32 + PC_LOAD_REFILL_CYCLES,
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
