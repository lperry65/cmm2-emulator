//!
//! Processor Reset logic
//!

use crate::Processor;
use crate::ProcessorMode;
use crate::bus::Bus;
use crate::core::exception::ExceptionHandling;
use crate::core::fault::Fault;
use crate::core::register::{BaseReg, PSR};

/// Trait for processor reset
pub trait Reset {
    ///
    /// Reset Processor
    ///
    fn reset(&mut self) -> Result<(), Fault>;
}

impl Reset for Processor {
    fn reset(&mut self) -> Result<(), Fault> {
        // All basic registers to zero.
        for r in &mut self.r0_12 {
            *r = 0;
        }

        // Main stack pointer is read via vector table
        let vtor = self.vtor;
        let sp = self.read32(vtor).map_err(Fault::on_vector_read)? & 0xffff_fffc;
        self.set_msp(sp);

        // Process stack pointer to zero
        self.set_psp(0);

        // Link Register
        self.lr = 0;

        // Mode
        self.mode = ProcessorMode::ThreadMode;

        // Apsr, ipsr
        self.psr = PSR { value: 0 };
        self.primask = false;

        #[cfg(not(feature = "armv6m"))]
        {
            self.faultmask = false;
            self.basepri = 0;
        }
        self.control.sp_sel = false;
        self.control.n_priv = false;
        #[cfg(feature = "has-fp")]
        {
            self.control.fpca = false;
        }

        // The system control space: the same values `Processor::new` starts with. The cycle
        // counter in the DWT keeps counting.
        self.nvic_interrupt_enabled = [0; 16];
        self.nvic_interrupt_pending = [0; 16];
        self.syst_rvr = 0;
        self.syst_cvr = 0;
        self.syst_csr = 0;
        self.aircr = 0xFA05_0000;
        self.system_reset_requested = false;
        self.scr = 0;
        self.ccr = 0;
        self.dwt_ctrl = 0x4000_0000;
        self.exceptions_reset();
        self.reset_scb_fault_state();
        self.reset_fp_system_state();

        //self.event_reg.clear();

        self.itstate = 0;
        self.execution_priority = self.get_execution_priority();

        let reset_vector = self
            .read32(vtor.wrapping_add(4))
            .map_err(Fault::on_vector_read)?;
        self.blx_write_pc(reset_vector);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peripheral::nvic::NVIC;

    const AIRCR: u32 = 0xE000_ED0C;

    fn booted() -> Processor {
        let mut image = vec![0; 0x100].into_boxed_slice();
        image[0..4].copy_from_slice(&0x2000_0100_u32.to_le_bytes());
        image[4..8].copy_from_slice(&0x0000_0041_u32.to_le_bytes());

        let mut processor = Processor::new();
        processor.flash_memory(image.len(), &image);
        processor.reset().unwrap();
        processor
    }

    #[test]
    fn reset_with_the_vector_table_at_the_top_of_memory_wraps_to_the_bottom() {
        let mut processor = booted();
        processor.device(Some(Box::new(crate::memory::ram::RAM::new(
            0xffff_f000,
            0x1000,
        ))));
        processor.write32(0xE000_ED08, 0xffff_fffc).unwrap();
        processor.write32(0xffff_fffc, 0x2000_0100).unwrap();

        // the stack pointer comes from the last word, and the reset vector is past it, at address 0
        assert_eq!(processor.reset(), Ok(()));
        assert_eq!(processor.get_pc(), 0x2000_0100 & !1);
    }

    #[test]
    fn aircr_with_key_and_sysresetreq_flags_a_reset() {
        let mut processor = booted();
        assert!(!processor.system_reset_requested);

        processor.write32(AIRCR, 0x05FA_0004).unwrap();

        assert!(processor.system_reset_requested);
        // The request itself is not stored.
        assert_eq!(processor.read32(AIRCR), Ok(0xFA05_0000));
    }

    #[test]
    fn aircr_without_the_key_does_not_flag_a_reset() {
        let mut processor = booted();
        processor.write32(AIRCR, 0x1234_0004).unwrap();
        assert!(!processor.system_reset_requested);
    }

    #[test]
    fn aircr_without_sysresetreq_does_not_flag_a_reset() {
        let mut processor = booted();
        processor.write32(AIRCR, 0x05FA_0300).unwrap();
        assert!(!processor.system_reset_requested);
    }

    #[test]
    fn a_write_that_does_not_ask_leaves_the_flag_alone() {
        let mut processor = booted();
        processor.write32(AIRCR, 0x05FA_0004).unwrap();
        processor.write32(AIRCR, 0x05FA_0300).unwrap();
        assert!(processor.system_reset_requested);
    }

    #[test]
    fn reset_clears_the_flag_and_priority_grouping() {
        let mut processor = booted();
        processor.write32(AIRCR, 0x05FA_0704).unwrap();
        assert_eq!(processor.read32(AIRCR), Ok(0xFA05_0700));

        processor.reset().unwrap();

        assert!(!processor.system_reset_requested);
        assert_eq!(processor.read32(AIRCR), Ok(0xFA05_0000));
    }

    #[test]
    fn reset_clears_the_interrupt_controller() {
        let mut processor = booted();
        processor.nvic_write_ipr_u8(5, 0x40);
        processor.nvic_write_iser(0, 1 << 5 | 1 << 7);
        processor.nvic_write_ispr(0, 1 << 5);
        processor.nvic_write_ispr(1, 1 << 3);
        assert!(processor.pending_exception_count > 0);

        processor.reset().unwrap();

        assert_eq!(processor.nvic_read_iser(0), 0);
        assert_eq!(processor.nvic_read_ispr(0), 0);
        assert_eq!(processor.nvic_read_ispr(1), 0);
        assert_eq!(processor.nvic_read_ipr_u8(5), 0);
        assert_eq!(processor.pending_exception_count, 0);
    }

    #[test]
    fn reset_stops_systick() {
        let mut processor = booted();
        processor.write32(0xE000_E014, 1000).unwrap();
        processor.write32(0xE000_E010, 0b111).unwrap();
        processor.syst_cvr = 500;

        processor.reset().unwrap();

        assert_eq!(processor.read32(0xE000_E010), Ok(0));
        assert_eq!(processor.read32(0xE000_E014), Ok(0));
        assert_eq!(processor.read32(0xE000_E018), Ok(0));
    }

    // SHPR1, SHCSR, SCR and CCR are there on ARMv7-M only
    #[cfg(not(feature = "armv6m"))]
    #[test]
    fn reset_clears_system_control_registers() {
        let mut processor = booted();
        processor.write32(0xE000_ED18, 0x4030_2010).unwrap();
        processor.write32(0xE000_ED24, 0x0007_0000).unwrap();
        processor.write32(0xE000_ED10, 0x10).unwrap();
        processor.write32(0xE000_ED14, 0x200).unwrap();
        // CPACR is there only with an FPU
        #[cfg(feature = "has-fp")]
        processor.write32(0xE000_ED88, 0x00F0_0000).unwrap();
        processor.write32(0xE000_1000, 1).unwrap();

        processor.reset().unwrap();

        assert_eq!(processor.read32(0xE000_ED18), Ok(0));
        assert_eq!(processor.read32(0xE000_ED24), Ok(0));
        assert_eq!(processor.read32(0xE000_ED10), Ok(0));
        assert_eq!(processor.read32(0xE000_ED14), Ok(0));
        #[cfg(feature = "has-fp")]
        assert_eq!(processor.read32(0xE000_ED88), Ok(0));
        assert_eq!(processor.read32(0xE000_1000), Ok(0x4000_0000));
    }

    #[test]
    fn reset_keeps_counters_and_the_cycle_counter() {
        let mut processor = booted();
        processor.cycle_count = 1234;
        processor.instruction_count = 567;
        processor.dwt_cyccnt = 89;
        processor.vtor = 0;

        processor.reset().unwrap();

        assert_eq!(processor.cycle_count, 1234);
        assert_eq!(processor.instruction_count, 567);
        assert_eq!(processor.dwt_cyccnt, 89);
        assert_eq!(processor.vtor, 0);
    }
}
