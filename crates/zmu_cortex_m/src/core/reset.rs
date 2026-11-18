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
        let result = self.take_reset();
        if let Err(fault) = result {
            // A vector read error at reset is a lockup at priority -1 with the lockup address
            // 0xFFFFFFFE (B1-556), with the fault status showing the vector read (B1-555). Here,
            // so that every caller gets it.
            self.record_fault_status(fault, crate::core::fault::FaultStatusContext::default());
            self.execution_priority = -1;
            self.lock_up(-1);
            self.set_pc(0xffff_fffe);
        }
        result
    }
}

impl Processor {
    /// `TakeReset()` (B1.5.5): the state is reset first, then the vectors are read.
    fn take_reset(&mut self) -> Result<(), Fault> {
        // The local exclusive monitor is Open Access (ClearExclusiveLocal in TakeReset)
        self.exclusive_tag = None;

        // All basic registers to zero.
        self.r0_12.fill(0);

        // VTOR is reset to the boot address (ResetSCSRegs, INITVTOR on the Cortex-M7)
        self.vtor = self.boot_address & !0x7f;
        let vtor = self.vtor;

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
        // STKALIGN resets to an IMPLEMENTATION DEFINED value, Arm recommends 1 (B3.2, p. B3-605)
        self.ccr = crate::CCR_RESET;
        self.dwt_ctrl = 0x4000_0000;
        self.exceptions_reset();
        self.reset_scb_fault_state();
        self.reset_fp_system_state();

        //self.event_reg.clear();

        self.itstate = 0;
        // a reset is no exception return: what is taken after it is no tail-chain
        self.exception_returned = false;
        // a reset leaves a lockup (B1-555), and the core runs again
        if self.locked_up {
            self.locked_up = false;
            self.running = true;
        }
        self.execution_priority = self.get_execution_priority();

        // The vectors are read after the state is reset (TakeReset), so a reset that cannot read
        // them has still cleared the active exceptions. Main stack pointer first.
        let sp = self.read32(vtor).map_err(Fault::on_vector_read)? & 0xffff_fffc;
        self.set_msp(sp);

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
    fn reset_with_the_vector_table_at_the_top_of_memory_reads_it_there() {
        let mut processor = booted();
        processor.device(Some(Box::new(crate::memory::ram::RAM::new(
            0xffff_f000,
            0x1000,
        ))));
        // VTOR keeps bits 31:7 (B3.2.5), so the table is at 0xffff_ff80; reset loads it from the
        // boot address
        processor.boot_address = 0xffff_ffff;
        processor.write32(0xffff_ff80, 0x2000_0100).unwrap();
        processor.write32(0xffff_ff84, 0x0800_0001).unwrap();

        assert_eq!(processor.reset(), Ok(()));
        assert_eq!(processor.get_pc(), 0x0800_0000);
        assert_eq!(processor.get_msp(), 0x2000_0100);
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
        assert_eq!(
            processor.read32(0xE000_ED14),
            Ok(if cfg!(feature = "armv7em") { 1 << 9 } else { 0 })
        );
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

    #[test]
    fn a_reset_leaves_a_lockup() {
        let mut processor = booted();
        processor.lock_up(-1);
        assert!(processor.is_locked_up());
        assert_eq!(processor.reset(), Ok(()));
        assert!(!processor.is_locked_up());
    }

    // VTOR is reset to the boot address (ResetSCSRegs), which is zero unless the embedder says.
    #[test]
    fn reset_loads_vtor_from_the_boot_address() {
        let mut processor = booted();
        processor.vtor = 0x1234_5600;
        processor.reset().unwrap();
        assert_eq!(processor.vtor, 0);
        processor.boot_address = 0x1fff_ff80;
        assert!(processor.reset().is_err());
        assert_eq!(processor.vtor, 0x1fff_ff80);
    }

    // A reset that fails reading its vectors has still reset the exceptions: nothing stays active.
    #[test]
    fn a_failed_reset_leaves_no_exception_active() {
        let mut processor = booted();
        processor.test_set_active_exception(crate::core::exception::Exception::NMI);
        processor.test_set_active_exception(crate::core::exception::Exception::HardFault);
        processor.boot_address = 0x1fff_ff80;
        assert!(processor.reset().is_err());
        assert!(!processor.exception_active(crate::core::exception::Exception::NMI));
        assert!(!processor.exception_active(crate::core::exception::Exception::HardFault));
    }

    // A reset that cannot read its vectors is a lockup at priority -1 at 0xFFFFFFFE with VECTTBL
    // (B1-556), whoever calls it: the lockup it left is still there, and so is the stop.
    #[test]
    fn a_failed_reset_is_a_lockup_for_every_caller() {
        for was_locked in [false, true] {
            let mut processor = booted();
            if was_locked {
                processor.lock_up(-1);
            }
            processor.boot_address = 0x1fff_ff80;
            assert_eq!(processor.reset(), Err(Fault::VectorTable));
            assert!(processor.is_locked_up(), "{was_locked}");
            assert!(!processor.running, "{was_locked}");
            assert_eq!(processor.get_pc(), 0xffff_fffe, "{was_locked}");
            assert_eq!(processor.execution_priority, -1, "{was_locked}");
            assert_eq!(processor.hfsr & (1 << 1), 1 << 1, "VECTTBL");
        }
    }
}
