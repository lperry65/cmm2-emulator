//!
//! Functionality for running instructions on a Processor.
//!

use crate::core::bits::Bits;
use crate::core::condition::Condition;
use crate::core::exception::{Exception, ExceptionHandling};
use crate::core::fault::{Fault, FaultStatusContext, FaultTrapReason};
use crate::core::fetch::Fetch;
use crate::core::instruction::{Imm32Carry, Instruction, SetFlags, instruction_size};

use crate::core::operation::condition_test;
use crate::core::register::{Apsr, BaseReg, Ipsr};
use crate::decoder::Decoder;
use crate::memory::map::MapMemory;
use crate::peripheral::{dwt::Dwt, systick::SysTick};

use crate::{CachedInstruction, Processor};

#[cfg(test)]
mod address_wrap_tests;
mod branch;
mod coproc;
mod divide;
mod exception;
#[cfg(test)]
mod guest_never_panics_tests;
mod load_and_store;
mod load_and_store_multiple;
mod misc;
mod misc_data_processing;
mod multiply;
mod packing;
mod parallel_add;
mod shift;
mod signed_multiply;
mod status_register;
mod std_data_processing;

#[cfg(all(test, feature = "has-fp"))]
mod fp_arith_tests;
#[cfg(feature = "has-fp")]
mod fp_data_processing;
#[cfg(feature = "has-fp")]
mod fp_generic;
#[cfg(feature = "has-fp")]
mod fp_load_and_store;
#[cfg(feature = "has-fp")]
mod fp_real;
#[cfg(feature = "has-fp")]
mod fp_register_transfer;
#[cfg(all(test, feature = "has-fp"))]
mod fp_test_support;

use branch::IsaBranch;
use coproc::IsaCoprocessor;
use divide::IsaDivide;
use exception::IsaException;
use load_and_store::IsaLoadAndStore;
use load_and_store_multiple::IsaLoadAndStoreMultiple;
use misc::IsaMisc;
use misc_data_processing::IsaMiscDataProcessing;
use multiply::IsaMultiply;
use packing::IsaPacking;
#[cfg(feature = "has-dsp-ext")]
use parallel_add::IsaParallelAddSub;
use shift::IsaShift;
use signed_multiply::IsaSignedMultiply;
use status_register::IsaStatusRegister;
use std_data_processing::IsaStandardDataProcessing;

#[cfg(feature = "has-fp")]
use fp_data_processing::IsaFloatingPointDataProcessing;
#[cfg(feature = "has-fp")]
pub(crate) use fp_generic::FloatingPointChecks;
#[cfg(feature = "has-fp")]
use fp_load_and_store::IsaFloatingPointLoadAndStore;
#[cfg(feature = "has-fp")]
use fp_register_transfer::IsaFloatingPointRegisterTransfer;

///
/// Stepping processor with instructions
///
pub trait Executor {
    ///
    /// Run processor forward. Simulates core + peripherals.
    ///
    fn step(&mut self);

    ///
    /// Run processor forward with core sleeping (peripherals only)
    ///
    fn step_sleep(&mut self);

    ///
    /// Execute given instruction. Returns number of clock cycles burn.
    ///
    fn execute(&mut self, instruction: &Instruction, instruction_size: usize) -> u32;
}

/// The cycles of an exception entry: the 12 cycles of interrupt latency ARM gives for the
/// Cortex-M7 with zero-wait-state memory (as for the M3 and M4). Every path that actually enters
/// an exception counts them: SVC, BKPT, a fault, an exception taken after an instruction or from
/// sleep, and the entry that starts the debugger's step after a halt; one taken straight after an
/// exception return counts the tail-chain cycles instead (`Processor::entry_cycles`). A lockup, on
/// any path, and a failed reset enter nothing: the step counts the one cycle of the fetch of the
/// lockup address (B1.5.15), and the core stops. (On silicon the stacking writes that came before
/// a failed vector read did happen; the emulator does not charge for them.)
pub(crate) const EXCEPTION_ENTRY_CYCLES: u32 = 12;

/// The cycles of an exception taken straight after an exception return: tail-chaining, which
/// skips the unstacking and the stacking. 6 is the figure ARM gives for the Cortex-M3 and M4; I
/// have found no authoritative figure for the Cortex-M7 (NOTICE.md).
pub(crate) const EXCEPTION_TAIL_CHAIN_CYCLES: u32 = 6;

type ExecuteResult = Result<ExecuteSuccess, Fault>;
#[derive(PartialEq, Debug, Copy, Clone)]
/// Succesfull execution  an instruction
pub enum ExecuteSuccess {
    /// The instruction was taken normally
    Taken {
        /// Number of clock cycles taken for the operation
        cycles: u32,
    },
    /// The instruction was not taken as the condition did not pass
    NotTaken,
    /// The execution branched to a new address, pc was set accordingly
    Branched {
        /// Number of clock cycles taken for the operation
        cycles: u32,
    },
}

trait ExecutorHelper {
    fn condition_passed(&self) -> bool;
    fn condition_passed_b(&self, cond: Condition) -> bool;
    fn integer_zero_divide_trapping_enabled(&self) -> bool;
    fn set_itstate(&mut self, state: u8);
    fn it_advance(&mut self);
    fn in_it_block(&self) -> bool;
    #[allow(dead_code)]
    fn last_in_it_block(&self) -> bool;
    fn execute_internal(&mut self, instruction: &Instruction) -> ExecuteResult;
    fn update_flags_check_it_block(
        &mut self,
        setflags: SetFlags,
        result: u32,
        carry: bool,
        overflow: bool,
    );
}

#[inline(always)]
fn resolve_addressing(rn: u32, imm32: u32, add: bool, index: bool) -> (u32, u32) {
    let offset_address = if add {
        rn.wrapping_add(imm32)
    } else {
        rn.wrapping_sub(imm32)
    };
    let address = if index { offset_address } else { rn };
    (address, offset_address)
}

#[inline(always)]
fn expand_conditional_carry(imm32: &Imm32Carry, carry: bool) -> (u32, bool) {
    match imm32 {
        Imm32Carry::NoCarry { imm32 } => (*imm32, carry),
        Imm32Carry::Carry { imm32_c0, imm32_c1 } => {
            if carry {
                *imm32_c1
            } else {
                *imm32_c0
            }
        }
    }
}

#[inline(always)]
fn conditional_setflags(setflags: SetFlags, in_it_block: bool) -> bool {
    match setflags {
        SetFlags::True => true,
        SetFlags::False => false,
        SetFlags::NotInITBlock => !in_it_block,
    }
}

impl Processor {
    /// Lock the core up (B1.5.15): it stops executing and keeps fetching the lockup address, at the
    /// priority of the lockup. What can leave it is what can preempt that priority: an NMI (-2)
    /// from a lockup at -1 (B1-555 names the `HardFault` handler; FAULTMASK and the reset handler
    /// are at -1 too), a reset (-3) from either, and nothing from a lockup at -2 (`step_locked_up`).
    pub(crate) fn lock_up(&mut self, priority: i16) {
        self.locked_up = true;
        self.running = false;
        self.sleeping = false;
        self.execution_priority = priority;
    }

    /// Whether a locked-up core can leave the lockup now: an exception can preempt it (an NMI at a
    /// lockup of priority -1, or a pending reset), which a step takes.
    pub fn lockup_can_end(&self) -> bool {
        self.locked_up && self.get_pending_exception().is_some()
    }

    /// One step of a locked-up core: nothing executes. An exception that can preempt the lockup
    /// (NMI, or a reset) is entered, and leaves the lockup if its entry works; otherwise the step
    /// is the one cycle of the fetch.
    fn step_locked_up(&mut self) {
        self.locked_up = false;
        self.running = true;
        if !self.check_exceptions() {
            self.locked_up = true;
            self.running = false;
        }
        let cycles = self.entry_cycles(false);
        self.account_cycles(cycles);
    }

    /// The cycles to charge for an exception entry that has just been made: 1 if it locked up the
    /// core (nothing was entered), else the tail-chain cycles or the entry cycles.
    #[inline(always)]
    pub(crate) fn entry_cycles(&self, tail_chain: bool) -> u32 {
        if self.locked_up {
            1
        } else if tail_chain {
            EXCEPTION_TAIL_CHAIN_CYCLES
        } else {
            EXCEPTION_ENTRY_CYCLES
        }
    }

    /// Count `count` cycles: the cycle counter, DWT and `SysTick`.
    #[inline(always)]
    pub(crate) fn account_cycles(&mut self, count: u32) {
        self.account_cycles_with(count, true);
    }

    /// Count `count` cycles, in the DWT too only if `dwt` says so.
    #[inline(always)]
    pub(crate) fn account_cycles_with(&mut self, count: u32, dwt: bool) {
        self.cycle_count += u64::from(count);
        if dwt && (self.dwt_ctrl & 1) != 0 {
            self.dwt_tick(count);
        }
        if (self.syst_csr & 1) != 0 {
            self.syst_step(count);
        }
    }

    #[cfg(not(feature = "armv6m"))]
    pub(crate) fn fault_delivery_exception(&mut self, fault: Fault) -> Exception {
        let mapped_exception = fault.exception();

        // A fault is taken as itself when it is enabled and its priority is higher than the
        // execution priority, so that it can preempt what runs (and what PRIMASK, BASEPRI and
        // FAULTMASK mask). Otherwise it is escalated to HardFault.
        let enabled = self.configurable_fault_enabled(mapped_exception);
        let preempts =
            self.get_exception_priority(mapped_exception) < self.get_execution_priority();

        if enabled && (preempts || mapped_exception == Exception::HardFault) {
            mapped_exception
        } else {
            self.set_hfsr_forced();
            Exception::HardFault
        }
    }

    #[cfg(feature = "armv6m")]
    #[allow(clippy::unused_self)] // the armv7 build needs the processor
    pub(crate) fn fault_delivery_exception(&mut self, fault: Fault) -> Exception {
        fault.exception()
    }

    pub(crate) fn queue_fault_trap(
        &mut self,
        fault: Fault,
        exception: Exception,
        pc: u32,
        active_exception: Option<Exception>,
        trap_reason: FaultTrapReason,
    ) {
        self.pending_fault_trap = Some(crate::core::fault::FaultContext {
            trap_reason,
            fault,
            exception,
            pc,
            active_exception,
        });
    }

    fn handle_fault(&mut self, fault: Fault, fault_pc: u32) -> u32 {
        let status = self.take_pending_fault_status();
        self.handle_fault_with_status(fault, fault_pc, status)
    }

    fn handle_fault_with_status(
        &mut self,
        fault: Fault,
        fault_pc: u32,
        status: FaultStatusContext,
    ) -> u32 {
        let exception = self.fault_delivery_exception(fault);
        let active_exception = self.active_exception();

        self.record_fault_status(fault, status);

        // a failing exception return left this flag, for the lockup address
        let from_return = std::mem::take(&mut self.exception_return_faulted);

        if self.is_lockup(exception) {
            self.queue_fault_trap(
                fault,
                exception,
                fault_pc,
                active_exception,
                FaultTrapReason::Lockup,
            );
            // INVPC on a return from the NMI handler to an execution priority of -1 locks up at -2
            // (B1-558); the others at the priority the core runs at
            let priority = if fault == Fault::InvPc
                && self.psr.get_isr_number() == usize::from(Exception::NMI)
            {
                -2
            } else {
                self.get_execution_priority().min(-1)
            };
            self.lock_up(priority);
            // the lockup address of INVPC and of a bus or MemManage fault on unstacking is
            // 0xFFFFFFFE (B1-557, B1-558); an instruction's other faults keep its address
            if from_return
                && matches!(
                    fault,
                    Fault::InvPc
                        | Fault::DAccViol
                        | Fault::Preciserr
                        | Fault::Impreciseerr
                        | Fault::Msunskerr
                )
            {
                self.set_pc(0xffff_fffe);
            }
            // a lockup enters no exception: the step takes the one cycle of the fetch
            return 1;
        }

        // an escalated fault keeps the return address of the original (B1.5.6, p. B1-534)
        let return_address = status.return_address.unwrap_or(fault_pc);

        match self.enter_exception(
            exception,
            return_address,
            status.refused_return,
            crate::core::exception::EntrySource::Synchronous,
        ) {
            crate::core::exception::EntryOutcome::Entered => {
                if self.get_fault_trap_mode().should_trap(exception) {
                    self.queue_fault_trap(
                        fault,
                        exception,
                        fault_pc,
                        active_exception,
                        FaultTrapReason::Fault,
                    );
                }
            }
            crate::core::exception::EntryOutcome::Derived(entry) => {
                self.queue_derived_traps(&entry, Some(fault), fault_pc);
            }
        }

        self.entry_cycles(false)
    }
}

impl ExecutorHelper for Processor {
    fn set_itstate(&mut self, state: u8) {
        self.itstate = state;
    }

    fn it_advance(&mut self) {
        if self.itstate != 0 {
            if self.itstate.get_bits(0..3) == 0 {
                self.itstate = 0;
            } else {
                let it = self.itstate.get_bits(0..5);
                self.itstate.set_bits(0..5, (it << 1) & 0b11111);
            }
        }
    }

    fn in_it_block(&self) -> bool {
        self.itstate.get_bits(0..4) != 0
    }

    fn last_in_it_block(&self) -> bool {
        self.itstate.get_bits(0..4) == 0b1000
    }
    fn integer_zero_divide_trapping_enabled(&self) -> bool {
        self.ccr & (1 << 4) != 0 // CCR.DIV_0_TRP
    }

    #[inline(always)]
    fn condition_passed(&self) -> bool {
        let itstate = self.itstate;

        if itstate == 0 {
            true
        } else {
            let cond = u16::from(itstate.get_bits(4..8));
            condition_test(
                Condition::from_u16(cond).unwrap_or(Condition::AL),
                &self.psr,
            )
        }
    }

    #[inline(always)]
    fn condition_passed_b(&self, cond: Condition) -> bool {
        condition_test(cond, &self.psr)
    }

    #[inline(always)]
    fn update_flags_check_it_block(
        &mut self,
        setflags: SetFlags,
        result: u32,
        carry: bool,
        overflow: bool,
    ) {
        if conditional_setflags(setflags, self.in_it_block()) {
            self.psr.set_n(result);
            self.psr.set_z(result);
            self.psr.set_c(carry);
            self.psr.set_v(overflow);
        }
    }

    #[allow(clippy::too_many_lines)]
    fn execute_internal(&mut self, instruction: &Instruction) -> ExecuteResult {
        match instruction {
            // --------------------------------------------
            //
            // Group: Branch instructions
            //
            // --------------------------------------------
            Instruction::B_t13 { params, .. } => self.exec_b_t13(*params),
            Instruction::B_t24 { imm32, .. } => self.exec_b_t24(*imm32),

            Instruction::BLX { rm } => self.exec_blx(*rm),
            Instruction::BX { rm } => self.exec_bx(*rm),
            Instruction::BL { imm32 } => self.exec_bl(*imm32),

            Instruction::CBZ { params } => self.exec_cbz(*params),
            Instruction::CBNZ { params } => self.exec_cbnz(*params),

            Instruction::TBB { params } => self.exec_tbb(*params),
            Instruction::TBH { params } => self.exec_tbh(*params),

            // --------------------------------------------
            //
            // Group: Standard data-processing instructions
            //
            // --------------------------------------------
            Instruction::ADD_reg { params, .. } => self.exec_add_reg(params),
            Instruction::ADD_imm { params, .. } => self.exec_add_imm(params),
            Instruction::ADD_sp_reg { params, .. } => self.exec_add_sp_reg(params),

            Instruction::ADC_reg { params, .. } => self.exec_adc_reg(params),
            Instruction::ADC_imm { params } => self.exec_adc_imm(params),

            Instruction::ADR { params, .. } => self.exec_adr(*params),

            Instruction::AND_reg { params, .. } => self.exec_and_reg(params),
            Instruction::AND_imm { params } => self.exec_and_imm(params),

            Instruction::BIC_imm { params } => self.exec_bic_imm(params),
            Instruction::BIC_reg { params, .. } => self.exec_bic_reg(params),

            Instruction::CMN_reg { params, .. } => self.exec_cmn_reg(params),
            Instruction::CMN_imm { params } => self.exec_cmn_imm(*params),

            Instruction::CMP_reg { params, .. } => self.exec_cmp_reg(params),
            Instruction::CMP_imm { params, .. } => self.exec_cmp_imm(*params),

            Instruction::EOR_reg { params, .. } => self.exec_eor_reg(params),
            Instruction::EOR_imm { params } => self.exec_eor_imm(params),

            Instruction::MOV_reg { params, .. } => self.exec_mov_reg(params),
            Instruction::MOV_imm { params, .. } => self.exec_mov_imm(params),

            Instruction::MVN_reg { params, .. } => self.exec_mvn_reg(params),
            Instruction::MVN_imm { params } => self.exec_mvn_imm(params),

            Instruction::ORN_imm { params } => self.exec_orn_imm(params),
            Instruction::ORN_reg { params } => self.exec_orn_reg(params),

            Instruction::ORR_imm { params } => self.exec_orr_imm(params),
            Instruction::ORR_reg { params, .. } => self.exec_orr_reg(params),

            Instruction::RSB_reg { params, .. } => self.exec_rsb_reg(params),
            Instruction::RSB_imm { params, .. } => self.exec_rsb_imm(params),

            Instruction::SBC_reg { params, .. } => self.exec_sbc_reg(params),
            Instruction::SBC_imm { params } => self.exec_sbc_imm(params),

            Instruction::SUB_reg { params, .. } => self.exec_sub_reg(params),
            Instruction::SUB_imm { params, .. } => self.exec_sub_imm(params),

            Instruction::TEQ_reg { params } => self.exec_teq_reg(params),
            Instruction::TEQ_imm { params } => self.exec_teq_imm(params),

            Instruction::TST_reg { params, .. } => self.exec_tst_reg(params),
            Instruction::TST_imm { params } => self.exec_tst_imm(params),

            // --------------------------------------------
            //
            // Group: Shift instructions
            //
            // --------------------------------------------
            Instruction::ASR_imm { params, .. } => self.exec_asr_imm(params),
            Instruction::ASR_reg { params, .. } => self.exec_asr_reg(params),

            Instruction::LSL_imm { params, .. } => self.exec_lsl_imm(params),
            Instruction::LSL_reg { params, .. } => self.exec_lsl_reg(params),

            Instruction::LSR_imm { params, .. } => self.exec_lsr_imm(params),
            Instruction::LSR_reg { params, .. } => self.exec_lsr_reg(params),

            Instruction::ROR_imm { params } => self.exec_ror_imm(params),
            Instruction::ROR_reg { params, .. } => self.exec_ror_reg(params),

            Instruction::RRX { params } => self.exec_rrx(params),
            // --------------------------------------------
            //
            // Group: Multiply instructions
            //
            // --------------------------------------------
            Instruction::MLA { params } => self.exec_mla(params),
            Instruction::MLS { params } => self.exec_mls(params),
            Instruction::MUL { params, .. } => self.exec_mul(params),
            // --------------------------------------------
            //
            // Group: Signed multiply instructions (ArmV7-m)
            //
            // --------------------------------------------
            Instruction::SMLAL { params } => self.exec_smlal(params),
            Instruction::SMULL { params } => self.exec_smull(params),

            // --------------------------------------------
            //
            // Group: Unsigned Multiply instructions (ARMv7-M base architecture)
            //
            // --------------------------------------------
            Instruction::UMLAL { params } => self.exec_umlal(params),
            Instruction::UMULL { params } => self.exec_umull(params),

            // --------------------------------------------
            //
            // Group: Signed Multiply instructions (ARMv7-M DSP extension)
            //
            // --------------------------------------------
            #[cfg(feature = "has-dsp-ext")]
            Instruction::SMUL { params } => self.exec_smul(params),
            #[cfg(not(feature = "has-dsp-ext"))]
            Instruction::SMUL { .. } => Err(Fault::UndefInstr),
            #[cfg(feature = "has-dsp-ext")]
            Instruction::SMLA { params } => self.exec_smla(params),
            #[cfg(not(feature = "has-dsp-ext"))]
            Instruction::SMLA { .. } => Err(Fault::UndefInstr),

            // --------------------------------------------
            //
            // Group: Saturating instructions (ARMv7-M base arch)
            //
            // --------------------------------------------

            // --------------------------------------------
            //
            // Group: Saturating instructions (ARMv7-M DSP extensions)
            //
            // --------------------------------------------

            // --------------------------------------------
            //
            // Group: Saturating add/sub (ARMv7-M DSP extensions)
            //
            // --------------------------------------------

            // --------------------------------------------
            //
            // Group: Packing and unpacking instructions
            //
            // --------------------------------------------
            Instruction::SXTB { params, .. } => self.exec_sxtb(params),
            Instruction::SXTH { params, .. } => self.exec_sxth(params),

            Instruction::UXTB { params, .. } => self.exec_uxtb(params),
            Instruction::UXTH { params, .. } => self.exec_uxth(params),

            // --------------------------------------------
            //
            // Group: Packing and unpacking instructions (DSP extensions)
            //
            // --------------------------------------------
            Instruction::UXTAB { params } => self.exec_uxtab(params),
            Instruction::UXTAH { params } => self.exec_uxtah(params),
            Instruction::SXTAB { params } => self.exec_sxtab(params),
            Instruction::SXTAH { params } => self.exec_sxtah(params),
            Instruction::PKH { params, tb_form } => self.exec_pkh(params, *tb_form),

            // --------------------------------------------
            //
            // Group: Divide instructions
            //
            // --------------------------------------------
            // ARMv7-M
            Instruction::SDIV { params } => self.exec_sdiv(params),
            Instruction::UDIV { params } => self.exec_udiv(params),

            // --------------------------------------------
            //
            // Group: Parallel add / sub (DSP extension)
            //
            // --------------------------------------------
            #[cfg(feature = "has-dsp-ext")]
            Instruction::UADD8 { params } => self.exec_uadd8(params),
            #[cfg(not(feature = "has-dsp-ext"))]
            Instruction::UADD8 { .. } => Err(Fault::UndefInstr),

            // --------------------------------------------
            //
            // Group:  Miscellaneous data-processing instructions
            //
            // --------------------------------------------
            Instruction::BFC { params } => self.exec_bfc(params),
            Instruction::BFI { params } => self.exec_bfi(params),

            Instruction::CLZ { params } => self.exec_clz(*params),

            Instruction::MOVT { params } => self.exec_movt(*params),

            Instruction::RBIT { params } => self.exec_rbit(*params),
            Instruction::SSAT { params } => self.exec_sat(params, true),
            Instruction::USAT { params } => self.exec_sat(params, false),
            Instruction::REV { params, .. } => self.exec_rev(*params),
            Instruction::REV16 { params, .. } => self.exec_rev16(*params),
            Instruction::REVSH { params, .. } => self.exec_revsh(*params),

            Instruction::UBFX { params } => self.exec_ubfx(params),
            Instruction::SBFX { params } => self.exec_sbfx(params),

            // --------------------------------------------
            //
            // Group:  Miscellaneous data-processing instructions (DSP extensions)
            //
            // --------------------------------------------
            #[cfg(feature = "has-dsp-ext")]
            Instruction::SEL { params } => self.exec_sel(params),
            #[cfg(not(feature = "has-dsp-ext"))]
            Instruction::SEL { .. } => Err(Fault::UndefInstr),
            // --------------------------------------------
            //
            // Group: Status register access instructions
            //
            // --------------------------------------------
            Instruction::MRS { params } => self.exec_mrs(*params),
            Instruction::MSR_reg { params } => self.exec_msr(*params),

            #[cfg(feature = "armv6m")]
            Instruction::CPS { im } => self.exec_cps(*im),

            #[cfg(not(feature = "armv6m"))]
            Instruction::CPS {
                im,
                affect_pri,
                affect_fault,
            } => self.exec_cps(*im, *affect_pri, *affect_fault),

            // --------------------------------------------
            //
            // Group:  Load and Store instructions
            //
            // --------------------------------------------
            Instruction::LDREX { params } => self.exec_ldrex(*params),
            Instruction::LDREXB { params } => self.exec_ldrexb(*params),
            Instruction::LDREXH { params } => self.exec_ldrexh(*params),

            Instruction::LDR_imm { params, .. } => self.exec_ldr_imm(params),
            Instruction::LDRB_imm { params, .. } => self.exec_ldrb_imm(params),
            Instruction::LDRH_imm { params, .. } => self.exec_ldrh_imm(params),
            Instruction::LDRSB_imm { params, .. } => self.exec_ldrsb_imm(params),
            Instruction::LDRSH_imm { params, .. } => self.exec_ldrsh_imm(params),

            Instruction::LDR_reg { params, .. } => self.exec_ldr_reg(params),
            Instruction::LDRB_reg { params, .. } => self.exec_ldrb_reg(params),
            Instruction::LDRH_reg { params, .. } => self.exec_ldrh_reg(params),
            Instruction::LDRSB_reg { params, .. } => self.exec_ldrsb_reg(params),
            Instruction::LDRSH_reg { params, .. } => self.exec_ldrsh_reg(params),

            Instruction::STR_imm { params, .. } => self.exec_str_imm(params),
            Instruction::STRB_imm { params, .. } => self.exec_strb_imm(params),
            Instruction::STRH_imm { params, .. } => self.exec_strh_imm(params),

            Instruction::STR_reg { params, .. } => self.exec_str_reg(params),
            Instruction::STRB_reg { params, .. } => self.exec_strb_reg(params),
            Instruction::STRH_reg { params, .. } => self.exec_strh_reg(params),

            Instruction::STREX { params } => self.exec_strex(*params),
            Instruction::STREXB { params } => self.exec_strexb(*params),
            Instruction::STREXH { params } => self.exec_strexh(*params),

            Instruction::STRD_imm { params } => self.exec_strd_imm(params),
            Instruction::LDRD_imm { params } => self.exec_ldrd_imm(params),

            Instruction::LDR_lit { params, .. } => self.exec_ldr_lit(params),

            // --------------------------------------------
            //
            // Group:  Load and Store Multiple instructions
            //
            // --------------------------------------------
            Instruction::STM { params, .. } => self.exec_stm(params),
            Instruction::STMDB { params } => self.exec_stmdb(params),
            Instruction::LDM { params, .. } => self.exec_ldm(params),

            Instruction::PUSH { registers, thumb32 } => {
                self.exec_push(*registers, *thumb32 && registers.len() == 1)
            }
            Instruction::POP { registers, thumb32 } => {
                self.exec_pop(*registers, *thumb32 && registers.len() == 1)
            }

            // --------------------------------------------
            //
            // Group: Miscellaneous
            //
            // --------------------------------------------
            Instruction::DMB => self.exec_dmb(),
            Instruction::DSB => self.exec_dsb(),
            Instruction::ISB => self.exec_isb(),

            Instruction::IT {
                firstcond, mask, ..
            } => self.exec_it(*firstcond, *mask),

            Instruction::NOP { .. } => Ok(ExecuteSuccess::Taken { cycles: 1 }),

            Instruction::PLD_imm { .. } => self.exec_pld_imm(),
            Instruction::PLD_lit { .. } => self.exec_pld_lit(),
            Instruction::PLD_reg { .. } => self.exec_pld_reg(),

            Instruction::SEV { .. } => self.exec_sev(),
            Instruction::WFE { .. } => self.exec_wfe(),
            Instruction::YIELD { .. } => self.exec_yield(),
            Instruction::WFI { .. } => self.exec_wfi(),

            // --------------------------------------------
            //
            // Group: Exception generating instructions
            //
            // --------------------------------------------
            Instruction::SVC { .. } => self.exec_svc(),
            Instruction::BKPT { imm32 } => self.exec_bkpt(*imm32),

            // --------------------------------------------
            //
            // Group: Coprocessor instructions
            //
            // --------------------------------------------
            Instruction::MCR { .. } => self.exec_mcr(),
            Instruction::MCR2 { .. } => self.exec_mcr2(),
            Instruction::LDC_imm { .. } => self.exec_ldc_imm(),
            Instruction::LDC2_imm { .. } => self.exec_ldc2_imm(),
            // --------------------------------------------
            //
            // Group: Floating-point load and store instructions
            //
            // --------------------------------------------
            #[cfg(feature = "has-fp")]
            Instruction::VLDR { params } => self.exec_vldr(params),
            #[cfg(feature = "has-fp")]
            Instruction::VSTR { params } => self.exec_vstr(params),
            #[cfg(feature = "has-fp")]
            Instruction::VLDM_T1 { params } => self.exec_vldm_t1(params),
            #[cfg(feature = "has-fp")]
            Instruction::VLDM_T2 { params } => self.exec_vldm_t2(params),
            #[cfg(feature = "has-fp")]
            Instruction::VSTM_T1 { params } => self.exec_vstm_t1(params),
            #[cfg(feature = "has-fp")]
            Instruction::VSTM_T2 { params } => self.exec_vstm_t2(params),
            #[cfg(feature = "has-fp")]
            Instruction::VPUSH { params } => self.exec_vpush(params),
            #[cfg(feature = "has-fp")]
            Instruction::VPOP { params } => self.exec_vpop(params),
            #[cfg(feature = "has-fp")]
            Instruction::VMOV_imm_32 { params } => self.exec_vmov_imm_32(*params),
            #[cfg(feature = "has-fp")]
            Instruction::VMOV_imm_64 { params } => self.exec_vmov_imm_64(*params),
            #[cfg(feature = "has-fp")]
            Instruction::VMOV_reg_f32 { params } => self.exec_vmov_reg_f32(*params),
            #[cfg(feature = "has-fp")]
            Instruction::VMOV_reg_f64 { params } => self.exec_vmov_reg_f64(*params),
            #[cfg(feature = "has-fp")]
            Instruction::VMOV_cr_scalar { params } => self.exec_vmov_cr_scalar(*params),
            #[cfg(feature = "has-fp")]
            Instruction::VMOV_scalar_cr { params } => self.exec_vmov_scalar_cr(*params),
            #[cfg(feature = "has-fp")]
            Instruction::VMOV_cr_sp { params } => self.exec_vmov_cr_sp(params),
            #[cfg(feature = "has-fp")]
            Instruction::VMOV_cr2_sp2 { params } => self.exec_vmov_cr2_sp2(*params),
            #[cfg(feature = "has-fp")]
            Instruction::VMOV_cr2_dp { params } => self.exec_vmov_cr2_dp(params),

            // --------------------------------------------
            //
            // Group: Floating-point register transfer instructions
            //
            // --------------------------------------------
            #[cfg(feature = "has-fp")]
            Instruction::VMRS { rt } => self.exec_vmrs(*rt),
            #[cfg(feature = "has-fp")]
            Instruction::VMSR { rt } => self.exec_vmsr(*rt),

            // --------------------------------------------
            //
            // Group: Floating-point data-processing instructions
            //
            // --------------------------------------------
            #[cfg(feature = "has-fp")]
            Instruction::VABS_f32 { params } => self.exec_vabs_f32(*params),
            #[cfg(feature = "has-fp")]
            Instruction::VABS_f64 { params } => self.exec_vabs_f64(*params),
            #[cfg(feature = "has-fp")]
            Instruction::VNEG_f32 { params } => self.exec_vneg_f32(*params),
            #[cfg(feature = "has-fp")]
            Instruction::VNEG_f64 { params } => self.exec_vneg_f64(*params),
            #[cfg(feature = "has-fp")]
            Instruction::VCMP_f32 { params } => self.exec_vcmp_f32(params),
            #[cfg(feature = "has-fp")]
            Instruction::VCMP_f64 { params } => self.exec_vcmp_f64(params),
            #[cfg(feature = "has-fp")]
            Instruction::VADD_f32 { params } => self.exec_vadd_f32(params),
            #[cfg(feature = "has-fp")]
            Instruction::VADD_f64 { params } => self.exec_vadd_f64(params),
            #[cfg(feature = "has-fp")]
            Instruction::VFMA_f32 { params } => self.exec_vfma_f32(params),
            #[cfg(feature = "has-fp")]
            Instruction::VFMA_f64 { params } => self.exec_vfma_f64(params),
            #[cfg(feature = "has-fp")]
            Instruction::VFMS_f32 { params } => self.exec_vfms_f32(params),
            #[cfg(feature = "has-fp")]
            Instruction::VFMS_f64 { params } => self.exec_vfms_f64(params),
            #[cfg(feature = "has-fp")]
            Instruction::VFNMS_f32 { params } => self.exec_vfnms_f32(params),
            #[cfg(feature = "has-fp")]
            Instruction::VFNMS_f64 { params } => self.exec_vfnms_f64(params),
            #[cfg(feature = "has-fp")]
            Instruction::VFNMA_f32 { params } => self.exec_vfnma_f32(params),
            #[cfg(feature = "has-fp")]
            Instruction::VFNMA_f64 { params } => self.exec_vfnma_f64(params),
            #[cfg(feature = "has-fp")]
            Instruction::VDIV_f32 { params } => self.exec_vdiv_f32(params),
            #[cfg(feature = "has-fp")]
            Instruction::VDIV_f64 { params } => self.exec_vdiv_f64(params),
            #[cfg(feature = "has-fp")]
            Instruction::VMUL_f32 { params } => self.exec_vmul_f32(params),
            #[cfg(feature = "has-fp")]
            Instruction::VMUL_f64 { params } => self.exec_vmul_f64(params),
            #[cfg(feature = "has-fp")]
            Instruction::VNMUL_f32 { params } => self.exec_vnmul_f32(params),
            #[cfg(feature = "has-fp")]
            Instruction::VNMUL_f64 { params } => self.exec_vnmul_f64(params),
            #[cfg(feature = "has-fp")]
            Instruction::VSQRT_f32 { params } => self.exec_vsqrt_f32(*params),
            #[cfg(feature = "has-fp")]
            Instruction::VSQRT_f64 { params } => self.exec_vsqrt_f64(*params),
            #[cfg(feature = "has-fp")]
            Instruction::VRINT_f32 { params } => self.exec_vrint_f32(*params),
            #[cfg(feature = "has-fp")]
            Instruction::VRINT_f64 { params } => self.exec_vrint_f64(*params),
            #[cfg(feature = "has-fp")]
            Instruction::VMINMAXNM_f32 { params } => self.exec_vminmaxnm_f32(params),
            #[cfg(feature = "has-fp")]
            Instruction::VMINMAXNM_f64 { params } => self.exec_vminmaxnm_f64(params),
            #[cfg(feature = "has-fp")]
            Instruction::VSUB_f32 { params } => self.exec_vsub_f32(params),
            #[cfg(feature = "has-fp")]
            Instruction::VSUB_f64 { params } => self.exec_vsub_f64(params),
            #[cfg(feature = "has-fp")]
            Instruction::VCVT { params } => self.exec_vcvt(params),
            #[cfg(feature = "has-fp")]
            Instruction::VCVT_fixed { params } => self.exec_vcvt_fixed(params),
            #[cfg(feature = "has-fp")]
            Instruction::VCVT_f64_f32 { params } => self.exec_vcvt_f64_f32(*params),
            #[cfg(feature = "has-fp")]
            Instruction::VCVT_f32_f64 { params } => self.exec_vcvt_f32_f64(*params),
            #[cfg(feature = "has-fp")]
            Instruction::VSEL_f32 { params } => self.exec_vsel_f32(*params),
            #[cfg(feature = "has-fp")]
            Instruction::VSEL_f64 { params } => self.exec_vsel_f64(*params),

            // --------------------------------------------
            //
            // Fallback: unknown instruction
            //
            // --------------------------------------------
            Instruction::UDF { .. } => Err(Fault::UndefInstr),
        }
    }
}

impl Executor for Processor {
    #[inline(always)]
    fn step_sleep(&mut self) {
        // the cycle of the sleeping step: the cycle counter and SysTick; the DWT only if the
        // embedder says its clock runs in sleep (`dwt_runs_in_sleep`, NOTICE.md)
        self.account_cycles_with(1, self.dwt_runs_in_sleep);
        let entered = self.check_exceptions();
        if entered {
            let cycles = self.entry_cycles(false);
            self.account_cycles(cycles);
        }
    }

    #[inline(always)]
    fn step(&mut self) {
        if self.locked_up {
            self.step_locked_up();
            return;
        }
        self.exception_returned = false;
        let pc = self.get_pc();
        let mapped_pc = (self.map_address(pc).wrapping_sub(self.code.base()) >> 1) as usize;
        let count = match self.instruction_cache.get(mapped_pc).copied() {
            Some(CachedInstruction::Decoded {
                instruction,
                instruction_size,
            }) => self.execute(&instruction, instruction_size),
            Some(CachedInstruction::FetchFault { fault, status }) => {
                self.handle_fault_with_status(fault, pc, status)
            }
            None => match self.fetch(pc) {
                Ok(thumb) => {
                    let instruction = self.decode(thumb);
                    self.execute(&instruction, instruction_size(&instruction))
                }
                Err(fault) => self.handle_fault(fault, pc),
            },
        };
        self.account_cycles(count);
        // a halt into a debugger takes no exception in the same step
        if (self.pending_exception_count != 0 || self.sleeping)
            && !self.debug_halt
            && !self.locked_up
            && self.check_exceptions()
        {
            // straight after an exception return it is tail-chained
            let cycles = self.entry_cycles(self.exception_returned);
            self.account_cycles(cycles);
        }
    }

    #[inline(always)]
    fn execute(&mut self, instruction: &Instruction, instruction_size: usize) -> u32 {
        self.instruction_count += 1;

        let in_it_block = self.in_it_block();

        match self.execute_internal(instruction) {
            Err(fault) => {
                let pc = self.get_pc();
                self.handle_fault(fault, pc)
            }
            Ok(ExecuteSuccess::NotTaken) => {
                self.add_pc(instruction_size as u32);
                if in_it_block {
                    self.it_advance();
                }
                1
            }
            Ok(ExecuteSuccess::Branched { cycles }) => {
                // a BKPT that halts into a debugger leaves the PC, and so the IT state, alone
                if in_it_block && !self.debug_halt {
                    self.it_advance();
                }
                cycles
            }
            Ok(ExecuteSuccess::Taken { cycles }) => {
                self.add_pc(instruction_size as u32);

                if in_it_block {
                    self.it_advance();
                }
                cycles
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(feature = "armv6m"))]
    use crate::bus::Bus;
    use crate::core::condition::Condition;
    use crate::core::fault::{Fault, FaultTrapReason};
    use crate::core::instruction::instruction_size;
    #[cfg(not(feature = "armv6m"))]
    use crate::core::{fault::FaultTrapMode, register::Ipsr};
    use crate::core::{
        instruction::{ITCondition, SRType, SetFlags},
        register::{BaseReg, Reg},
    };

    #[cfg(not(feature = "armv6m"))]
    const CFSR_UNDEFINSTR: u32 = 1 << 16;
    #[cfg(not(feature = "armv6m"))]
    const CFSR_INVPC: u32 = 1 << 18;
    #[cfg(not(feature = "armv6m"))]
    const CFSR_IACCVIOL: u32 = 1 << 0;
    #[cfg(not(feature = "armv6m"))]
    const CFSR_DACCVIOL: u32 = 1 << 1;
    #[cfg(not(feature = "armv6m"))]
    const CFSR_MSTKERR: u32 = 1 << 4;
    #[cfg(not(feature = "armv6m"))]
    const CFSR_MMARVALID: u32 = 1 << 7;
    #[cfg(not(feature = "armv6m"))]
    const CFSR_IBUSERR: u32 = 1 << 8;
    #[cfg(not(feature = "armv6m"))]
    const CFSR_PRECISERR: u32 = 1 << 9;
    #[cfg(not(feature = "armv6m"))]
    const CFSR_BFARVALID: u32 = 1 << 15;
    #[cfg(not(feature = "armv6m"))]
    const HFSR_VECTTBL: u32 = 1 << 1;
    #[cfg(not(feature = "armv6m"))]
    const CFSR_INVSTATE: u32 = 1 << 17;
    #[cfg(not(feature = "armv6m"))]
    const HFSR_FORCED: u32 = 1 << 30;
    #[cfg(not(feature = "armv6m"))]
    const SHCSR_MEMFAULTENA: u32 = 1 << 16;
    #[cfg(not(feature = "armv6m"))]
    const SHCSR_BUSFAULTENA: u32 = 1 << 17;
    #[cfg(not(feature = "armv6m"))]
    const SHCSR_USGFAULTENA: u32 = 1 << 18;

    #[test]
    fn test_execute_internal_udf_returns_undefinstr_fault() {
        let mut core = Processor::new();

        let result = core.execute_internal(&Instruction::UDF {
            imm32: 0,
            opcode: crate::core::thumb::ThumbCode::Thumb16 { opcode: 0xde00 },
            thumb32: false,
        });

        assert_eq!(result, Err(Fault::UndefInstr));
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_execute_udf_escalates_to_hardfault_when_usagefault_disabled() {
        let mut core = Processor::new();
        core.fault_trap_mode(FaultTrapMode::none());
        core.set_msp(0x2000_0100);
        core.set_pc(0x0800_0004);

        let instruction = Instruction::UDF {
            imm32: 0,
            opcode: crate::core::thumb::ThumbCode::Thumb16 { opcode: 0xde00 },
            thumb32: false,
        };

        let cycles = core.execute(&instruction, instruction_size(&instruction));

        assert_eq!(cycles, 12);
        assert_eq!(core.psr.get_isr_number(), Exception::HardFault.into());
        assert_eq!(core.cfsr, CFSR_UNDEFINSTR);
        assert_eq!(core.hfsr, HFSR_FORCED);
        assert_eq!(core.take_pending_fault_trap(), None);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_execute_udf_enters_usagefault_when_enabled() {
        let mut core = Processor::new();
        core.fault_trap_mode(FaultTrapMode::none());
        core.set_msp(0x2000_0100);
        core.set_pc(0x0800_0004);
        core.write32(0xE000_ED24, SHCSR_USGFAULTENA).unwrap();

        let instruction = Instruction::UDF {
            imm32: 0,
            opcode: crate::core::thumb::ThumbCode::Thumb16 { opcode: 0xde00 },
            thumb32: false,
        };

        let cycles = core.execute(&instruction, instruction_size(&instruction));

        assert_eq!(cycles, 12);
        assert_eq!(core.psr.get_isr_number(), Exception::UsageFault.into());
        assert_eq!(core.cfsr, CFSR_UNDEFINSTR);
        assert_eq!(core.hfsr, 0);
        assert_eq!(core.take_pending_fault_trap(), None);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_execute_svc_enters_svcall_and_stacks_next_pc() {
        const STACK_TOP: u32 = 0x2000_0100;
        const SVC_VECTOR: u32 = 11 * 4;
        const SVC_HANDLER: u32 = 0x0800_0101;

        let mut core = Processor::new();
        core.fault_trap_mode(FaultTrapMode::none());
        core.set_msp(STACK_TOP);
        core.set_pc(0x0800_0044);
        core.vtor = 0;

        let mut image = vec![0_u8; 0x100];
        image[SVC_VECTOR as usize..SVC_VECTOR as usize + 4]
            .copy_from_slice(&SVC_HANDLER.to_le_bytes());
        core.flash_memory(image.len(), &image);

        let instruction = Instruction::SVC { imm32: 0 };

        let cycles = core.execute(&instruction, instruction_size(&instruction));

        assert_eq!(cycles, 12);
        assert_eq!(core.psr.get_isr_number(), Exception::SVCall.into());
        assert_eq!(core.get_pc(), 0x0800_0100);
        assert_eq!(core.get_msp(), STACK_TOP - 0x20);
        assert_eq!(core.read32(STACK_TOP - 0x08).unwrap(), 0x0800_0046);
        assert_eq!(core.get_r(Reg::LR), 0xFFFF_FFF9);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_fault_trap_mode_matrix_for_all_mapped_faults() {
        let cases = [
            (Fault::Forced, Exception::HardFault),
            (Fault::DAccViol, Exception::MemoryManagementFault),
            (Fault::Preciserr, Exception::BusFault),
            (Fault::UndefInstr, Exception::UsageFault),
        ];

        for trap_bits in 0u8..16 {
            let mut mode = FaultTrapMode::none();
            mode.set_trap(Exception::HardFault, (trap_bits & 0b0001) != 0);
            mode.set_trap(Exception::MemoryManagementFault, (trap_bits & 0b0010) != 0);
            mode.set_trap(Exception::BusFault, (trap_bits & 0b0100) != 0);
            mode.set_trap(Exception::UsageFault, (trap_bits & 0b1000) != 0);

            for (index, (fault, exception)) in cases.iter().copied().enumerate() {
                let mut core = Processor::new();
                core.fault_trap_mode(mode);
                core.set_msp(0x2000_0100);
                core.write32(
                    0xE000_ED24,
                    SHCSR_MEMFAULTENA | SHCSR_BUSFAULTENA | SHCSR_USGFAULTENA,
                )
                .unwrap();

                let pc = 0x0800_0000 + u32::from(trap_bits) * 0x10 + u32::try_from(index).unwrap();
                let cycles = core.handle_fault(fault, pc);

                assert_eq!(cycles, 12);
                assert_eq!(core.psr.get_isr_number(), exception.into());

                let trap = core.take_pending_fault_trap();
                let should_trap = mode.should_trap(exception);

                assert_eq!(
                    trap.is_some(),
                    should_trap,
                    "trap_bits={trap_bits:04b} fault={fault:?}"
                );

                if let Some(trap) = trap {
                    assert_eq!(trap.trap_reason, FaultTrapReason::Fault);
                    assert_eq!(trap.fault, fault);
                    assert_eq!(trap.exception, exception);
                    assert_eq!(trap.pc, pc);
                }
            }
        }
    }

    #[test]
    fn test_default_fault_trap_traps_hardfault() {
        let mut core = Processor::new();
        core.set_msp(0x2000_0100);

        let cycles = core.handle_fault(Fault::Forced, 0x0800_0000);

        assert_eq!(cycles, 12);
        let trap = core
            .take_pending_fault_trap()
            .expect("hardfault trap expected");
        assert_eq!(trap.trap_reason, FaultTrapReason::Fault);
        assert_eq!(trap.fault, Fault::Forced);
        assert_eq!(trap.exception, Exception::HardFault);
        assert_eq!(trap.pc, 0x0800_0000);
    }

    #[test]
    fn test_lockup_always_traps_even_when_hardfault_trap_disabled() {
        let mut core = Processor::new();
        core.fault_trap_mode(crate::core::fault::FaultTrapMode::none());
        core.test_set_active_exception(Exception::HardFault);

        let cycles = core.handle_fault(Fault::Forced, 0x0800_0100);

        assert_eq!(cycles, 1);
        let trap = core
            .take_pending_fault_trap()
            .expect("lockup trap expected");
        assert_eq!(trap.trap_reason, FaultTrapReason::Lockup);
        assert_eq!(trap.fault, Fault::Forced);
        assert_eq!(trap.exception, Exception::HardFault);
        assert_eq!(trap.pc, 0x0800_0100);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_handle_fault_sets_cfsr_undefinstr_bit() {
        let mut core = Processor::new();
        core.fault_trap_mode(FaultTrapMode::none());
        core.set_msp(0x2000_0100);
        core.write32(0xE000_ED24, SHCSR_USGFAULTENA).unwrap();

        let cycles = core.handle_fault(Fault::UndefInstr, 0x0800_0200);

        assert_eq!(cycles, 12);
        assert_eq!(core.psr.get_isr_number(), Exception::UsageFault.into());
        assert_eq!(core.cfsr, CFSR_UNDEFINSTR);
        assert_eq!(core.hfsr, 0);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_handle_fault_sets_cfsr_invpc_bit() {
        let mut core = Processor::new();
        core.fault_trap_mode(FaultTrapMode::none());
        core.set_msp(0x2000_0100);
        core.write32(0xE000_ED24, SHCSR_USGFAULTENA).unwrap();

        let cycles = core.handle_fault(Fault::InvPc, 0x0800_0210);

        assert_eq!(cycles, 12);
        assert_eq!(core.psr.get_isr_number(), Exception::UsageFault.into());
        assert_eq!(core.cfsr, CFSR_INVPC);
        assert_eq!(core.hfsr, 0);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_invalid_exception_return_enters_usagefault_when_enabled() {
        let mut core = Processor::new();
        core.fault_trap_mode(FaultTrapMode::none());
        core.set_msp(0x2000_0100);
        core.write32(0xE000_ED24, SHCSR_USGFAULTENA).unwrap();
        core.psr.set_isr_number(Exception::SysTick.into());
        core.mode = crate::ProcessorMode::HandlerMode;

        let fault = core
            .bx_write_pc(0xFFFF_FFF0)
            .expect_err("INVPC fault expected");
        let cycles = core.handle_fault(fault, 0x0800_0300);

        assert_eq!(cycles, 12);
        assert_eq!(core.psr.get_isr_number(), Exception::UsageFault.into());
        assert_eq!(core.cfsr, CFSR_INVPC);
        assert_eq!(core.hfsr, 0);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_invalid_exception_return_escalates_to_hardfault_when_usagefault_disabled() {
        let mut core = Processor::new();
        core.fault_trap_mode(FaultTrapMode::none());
        core.set_msp(0x2000_0100);
        core.psr.set_isr_number(Exception::SysTick.into());
        core.mode = crate::ProcessorMode::HandlerMode;

        let fault = core
            .bx_write_pc(0xFFFF_FFF0)
            .expect_err("INVPC fault expected");
        let cycles = core.handle_fault(fault, 0x0800_0310);

        assert_eq!(cycles, 12);
        assert_eq!(core.psr.get_isr_number(), Exception::HardFault.into());
        assert_eq!(core.cfsr, CFSR_INVPC);
        assert_eq!(core.hfsr, HFSR_FORCED);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_invalid_exception_return_with_bad_thumb_state_enters_usagefault_when_enabled() {
        const FRAMEPTR: u32 = 0x2000_00e0;

        let mut core = Processor::new();
        core.fault_trap_mode(FaultTrapMode::none());
        core.set_msp(FRAMEPTR);
        core.write32(0xE000_ED24, SHCSR_USGFAULTENA).unwrap();
        core.write32(FRAMEPTR.wrapping_add(0x18), 0x0800_0001)
            .unwrap();
        core.write32(FRAMEPTR.wrapping_add(0x1c), 0).unwrap();
        core.test_set_active_exception(Exception::SysTick);

        let fault = core
            .bx_write_pc(0xFFFF_FFF9)
            .expect_err("INVSTATE fault expected");
        let cycles = core.handle_fault(fault, 0x0800_0320);

        assert_eq!(cycles, 12);
        assert_eq!(core.psr.get_isr_number(), Exception::UsageFault.into());
        assert_eq!(core.cfsr, CFSR_INVSTATE);
        assert_eq!(core.hfsr, 0);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_invalid_exception_return_with_bad_thumb_state_escalates_to_hardfault_when_usagefault_disabled()
     {
        const FRAMEPTR: u32 = 0x2000_00e0;

        let mut core = Processor::new();
        core.fault_trap_mode(FaultTrapMode::none());
        core.set_msp(FRAMEPTR);
        core.write32(FRAMEPTR.wrapping_add(0x18), 0x0800_0001)
            .unwrap();
        core.write32(FRAMEPTR.wrapping_add(0x1c), 0).unwrap();
        core.test_set_active_exception(Exception::SysTick);

        let fault = core
            .bx_write_pc(0xFFFF_FFF9)
            .expect_err("INVSTATE fault expected");
        let cycles = core.handle_fault(fault, 0x0800_0330);

        assert_eq!(cycles, 12);
        assert_eq!(core.psr.get_isr_number(), Exception::HardFault.into());
        assert_eq!(core.cfsr, CFSR_INVSTATE);
        assert_eq!(core.hfsr, HFSR_FORCED);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_invalid_exception_return_with_nonzero_stacked_ipsr_enters_usagefault_when_enabled() {
        const FRAMEPTR: u32 = 0x2000_00e0;

        let mut core = Processor::new();
        core.fault_trap_mode(FaultTrapMode::none());
        core.set_msp(FRAMEPTR);
        core.write32(0xE000_ED24, SHCSR_USGFAULTENA).unwrap();
        core.write32(FRAMEPTR.wrapping_add(0x18), 0x0800_0001)
            .unwrap();
        core.write32(
            FRAMEPTR.wrapping_add(0x1c),
            (1 << 24) | usize::from(Exception::HardFault) as u32,
        )
        .unwrap();
        core.test_set_active_exception(Exception::SysTick);

        let fault = core
            .bx_write_pc(0xFFFF_FFF9)
            .expect_err("INVPC fault expected");
        let cycles = core.handle_fault(fault, 0x0800_0340);

        assert_eq!(cycles, 12);
        assert_eq!(core.psr.get_isr_number(), Exception::UsageFault.into());
        assert_eq!(core.cfsr, CFSR_INVPC);
        assert_eq!(core.hfsr, 0);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_invalid_exception_return_with_nonzero_stacked_ipsr_escalates_to_hardfault_when_usagefault_disabled()
     {
        const FRAMEPTR: u32 = 0x2000_00e0;

        let mut core = Processor::new();
        core.fault_trap_mode(FaultTrapMode::none());
        core.set_msp(FRAMEPTR);
        core.write32(FRAMEPTR.wrapping_add(0x18), 0x0800_0001)
            .unwrap();
        core.write32(
            FRAMEPTR.wrapping_add(0x1c),
            (1 << 24) | usize::from(Exception::HardFault) as u32,
        )
        .unwrap();
        core.test_set_active_exception(Exception::SysTick);

        let fault = core
            .bx_write_pc(0xFFFF_FFF9)
            .expect_err("INVPC fault expected");
        let cycles = core.handle_fault(fault, 0x0800_0350);

        assert_eq!(cycles, 12);
        assert_eq!(core.psr.get_isr_number(), Exception::HardFault.into());
        assert_eq!(core.cfsr, CFSR_INVPC);
        assert_eq!(core.hfsr, HFSR_FORCED);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_handle_fault_sets_cfsr_invstate_bit() {
        let mut core = Processor::new();
        core.fault_trap_mode(FaultTrapMode::none());
        core.set_msp(0x2000_0100);
        core.write32(0xE000_ED24, SHCSR_USGFAULTENA).unwrap();

        let cycles = core.handle_fault(Fault::Invstate, 0x0800_0214);

        assert_eq!(cycles, 12);
        assert_eq!(core.psr.get_isr_number(), Exception::UsageFault.into());
        assert_eq!(core.cfsr, CFSR_INVSTATE);
        assert_eq!(core.hfsr, 0);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_handle_fault_sets_hfsr_vecttbl_bit() {
        let mut core = Processor::new();
        core.fault_trap_mode(FaultTrapMode::none());
        core.set_msp(0x2000_0100);

        let cycles = core.handle_fault(Fault::VectorTable, 0x0800_0220);

        assert_eq!(cycles, 12);
        assert_eq!(core.psr.get_isr_number(), Exception::HardFault.into());
        assert_eq!(core.hfsr, HFSR_VECTTBL);
        assert_eq!(core.cfsr, 0);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_handle_fault_sets_hfsr_forced_bit_for_forced_fault() {
        let mut core = Processor::new();
        core.fault_trap_mode(FaultTrapMode::none());
        core.set_msp(0x2000_0100);

        let cycles = core.handle_fault(Fault::Forced, 0x0800_0240);

        assert_eq!(cycles, 12);
        assert_eq!(core.psr.get_isr_number(), Exception::HardFault.into());
        assert_eq!(core.hfsr, HFSR_FORCED);
        assert_eq!(core.cfsr, 0);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_handle_fault_escalates_entry_stacking_fault_to_hardfault() {
        let mut core = Processor::new();
        core.set_msp(0);

        let cycles = core.handle_fault(Fault::Forced, 0x0800_0250);

        assert_eq!(cycles, 12);
        let trap = core.take_pending_fault_trap().expect("fault trap expected");
        assert_eq!(trap.fault, Fault::Mstkerr);
        assert_eq!(trap.exception, Exception::HardFault);
        assert_eq!(core.cfsr, CFSR_MSTKERR);
        assert_eq!(core.hfsr, HFSR_FORCED);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_execute_str_reg_latches_mmfar_for_data_access_violation() {
        let mut core = Processor::new();
        core.fault_trap_mode(FaultTrapMode::none());
        core.set_msp(0x2000_0100);
        core.write32(0xE000_ED24, SHCSR_MEMFAULTENA).unwrap();
        core.set_r(Reg::R0, 0x1234_5678);
        core.set_r(Reg::R1, 0x6000_0000);
        core.set_r(Reg::R2, 0);

        let instruction = Instruction::STR_reg {
            params: crate::core::instruction::Reg3FullParams {
                rt: Reg::R0,
                rn: Reg::R1,
                rm: Reg::R2,
                shift_t: SRType::LSL,
                shift_n: 0,
                index: true,
                add: true,
                wback: false,
            },
            thumb32: false,
        };

        let cycles = core.execute(&instruction, instruction_size(&instruction));

        assert_eq!(cycles, 12);
        assert_eq!(
            core.psr.get_isr_number(),
            Exception::MemoryManagementFault.into()
        );
        assert_eq!(core.cfsr, CFSR_DACCVIOL | CFSR_MMARVALID);
        assert_eq!(core.mmfar, 0x6000_0000);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_step_latches_mmfar_for_instruction_access_violation() {
        let mut core = Processor::new();
        core.fault_trap_mode(FaultTrapMode::none());
        core.set_msp(0x2000_0100);
        core.write32(0xE000_ED24, SHCSR_MEMFAULTENA).unwrap();
        core.set_pc(0x6000_0000);

        core.step();

        assert_eq!(
            core.psr.get_isr_number(),
            Exception::MemoryManagementFault.into()
        );
        assert_eq!(core.cfsr, CFSR_IACCVIOL | CFSR_MMARVALID);
        assert_eq!(core.mmfar, 0x6000_0000);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_handle_fault_sets_bfar_for_precise_bus_fault() {
        let mut core = Processor::new();
        core.fault_trap_mode(FaultTrapMode::none());
        core.set_msp(0x2000_0100);
        core.write32(0xE000_ED24, SHCSR_BUSFAULTENA).unwrap();

        let cycles = core.handle_fault_with_status(
            Fault::Preciserr,
            0x0800_0230,
            FaultStatusContext::with_fault_address(0x4000_1000),
        );

        assert_eq!(cycles, 12);
        assert_eq!(core.psr.get_isr_number(), Exception::BusFault.into());
        assert_eq!(core.cfsr, CFSR_PRECISERR | CFSR_BFARVALID);
        assert_eq!(core.bfar, 0x4000_1000);
    }

    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_handle_fault_without_address_does_not_set_bfarvalid_or_overwrite_bfar() {
        let mut core = Processor::new();
        core.fault_trap_mode(FaultTrapMode::none());
        core.set_msp(0x2000_0100);
        core.write32(0xE000_ED24, SHCSR_BUSFAULTENA).unwrap();
        core.bfar = 0xfeed_face;

        let cycles = core.handle_fault_with_status(
            Fault::IBusErr,
            0x0800_0240,
            FaultStatusContext::default(),
        );

        assert_eq!(cycles, 12);
        assert_eq!(core.psr.get_isr_number(), Exception::BusFault.into());
        assert_eq!(core.cfsr, CFSR_IBUSERR);
        assert_eq!(core.bfar, 0xfeed_face);
    }

    #[test]
    fn test_it_block_branch_clears_state() {
        // This test reproduces a bug: when a conditional branch inside
        // an IT block executes and branches, the IT state is not advanced,
        // leaving residual itstate that affects the next instruction.
        //
        // This is the exact sequence from hello_world-cm3.elf:
        // 3fc: subs r3, #4
        // 3fe: ittt ge
        // 400: ldrge r0, [r1, r3]  <- conditional
        // 402: strge r0, [r2, r3]  <- conditional
        // 404: bge 0x3fc            <- conditional branch that loops back
        //
        // When the branch executes, IT state should advance (clearing it since
        // it's the last instruction), but in bug case it didn't, so the SUB at
        // 0x3fc thought it's still in an IT block and didn't set flags!

        use crate::core::instruction::Reg2ImmParams;

        let mut core = Processor::new();
        core.set_r(Reg::R1, 0x2000_0000);
        core.set_r(Reg::R2, 0x2000_1000);
        core.set_r(Reg::R3, 0x0000_0008);

        // Set flags so GE is true initially (N=0, V=0)
        core.psr.set_n(0);
        core.psr.set_v(false);
        core.psr.set_z(0);
        core.psr.set_c(true);

        // Execute "ittt ge" - 3 Then instructions with GE condition
        let it_inst = Instruction::IT {
            x: Some(ITCondition::Then),
            y: Some(ITCondition::Then),
            z: None,
            firstcond: Condition::GE,
            mask: 0b0010,
        };
        core.execute(&it_inst, instruction_size(&it_inst));

        // Execute 3 conditional instructions, the last being a branch
        // Instruction 1: ldr r0, [r1, r3]
        let ldr = Instruction::LDR_reg {
            params: crate::core::instruction::Reg3FullParams {
                rt: Reg::R0,
                rn: Reg::R1,
                rm: Reg::R3,
                shift_t: SRType::LSL,
                shift_n: 0,
                index: true,
                add: true,
                wback: false,
            },
            thumb32: false,
        };
        core.execute(&ldr, instruction_size(&ldr));

        // Instruction 2: str r0, [r2, r3]
        let str = Instruction::STR_reg {
            params: crate::core::instruction::Reg3FullParams {
                rt: Reg::R0,
                rn: Reg::R2,
                rm: Reg::R3,
                shift_t: SRType::LSL,
                shift_n: 0,
                index: true,
                add: true,
                wback: false,
            },
            thumb32: false,
        };
        core.execute(&str, instruction_size(&str));

        // Instruction 3: b (conditional branch back)
        // When this executes and branches, itstate should be cleared
        let b = Instruction::B_t13 {
            params: crate::core::instruction::CondBranchParams {
                cond: Condition::GE,
                imm32: -4, // Branch back (negative offset)
            },
            thumb32: false,
        };

        // Before the branch, we should still be in IT block
        assert!(core.in_it_block(), "Should be in IT block before branch");

        core.execute(&b, instruction_size(&b));

        // After the branch executes, IT state should be cleared!
        assert!(
            !core.in_it_block(),
            "IT state should be cleared after the last instruction in IT block (the branch)"
        );

        // Now execute a SUBS instruction (SetFlags::NotInITBlock)
        // This should set flags because we're no longer in an IT block
        let sub_inst = Instruction::SUB_imm {
            params: Reg2ImmParams {
                rd: Reg::R3,
                rn: Reg::R3,
                imm32: 4,
                setflags: SetFlags::NotInITBlock,
            },
            thumb32: false,
        };

        // Clear flags first to make the test clear
        core.psr.set_n(0);
        core.psr.set_z(0);

        core.execute(&sub_inst, instruction_size(&sub_inst));

        // R3 should be 4 (8 - 4)
        assert_eq!(core.get_r(Reg::R3), 0x0000_0004);

        // Flags should have been set by SUBS (since we're not in IT block)
        // With result = 4, N should be 0 and Z should be 0
        assert!(
            !core.psr.get_n(),
            "N flag should be clear when result is positive"
        );
        assert!(
            !core.psr.get_z(),
            "Z flag should be clear when result is non-zero"
        );
    }

    #[test]
    fn test_resolve_addressing_wraps_on_add_overflow() {
        assert_eq!(resolve_addressing(u32::MAX, 4, true, true), (3, 3));
    }

    #[test]
    fn test_resolve_addressing_wraps_on_sub_underflow() {
        assert_eq!(
            resolve_addressing(1, 4, false, true),
            (u32::MAX - 2, u32::MAX - 2)
        );
    }

    // STM T1 (A7.7.159, p. A7-383) has an 8-bit register list: r4-r7 stores four words and writes
    // the base back 16 higher.
    #[test]
    fn test_stm_t1_stores_r7_too() {
        use crate::bus::Bus;
        let mut processor = Processor::new();
        for (n, reg) in [Reg::R4, Reg::R5, Reg::R6, Reg::R7].into_iter().enumerate() {
            processor.set_r(reg, 0x100 + n as u32);
        }
        processor.set_r(Reg::R0, 0x2000_0100);
        processor.set_pc(0x100);
        // stmia r0!, {r4-r7}
        processor.execute(&crate::decoder::decode_16(0xc0f0), 2);
        assert_eq!(processor.get_r(Reg::R0), 0x2000_0110);
        for n in 0..4_u32 {
            assert_eq!(processor.read32(0x2000_0100 + 4 * n), Ok(0x100 + n));
        }
        // and ldmia reads four back
        processor.set_r(Reg::R1, 0x2000_0100);
        processor.execute(&crate::decoder::decode_16(0xc9f0), 2); // ldmia r1!, {r4-r7}
        assert_eq!(processor.get_r(Reg::R1), 0x2000_0110);
        assert_eq!(processor.get_r(Reg::R7), 0x103);
    }

    // A lockup enters no exception, so it costs no entry cycles: the step that finds it takes the
    // one cycle of the fetch. (The core then keeps fetching the lockup address; here it stops.)
    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_a_lockup_costs_one_cycle_and_no_exception_entry() {
        let mut core = Processor::new();
        core.test_set_active_exception(Exception::HardFault);
        core.execution_priority = core.get_execution_priority();
        let cycles = core.handle_fault(Fault::UndefInstr, 0x100);
        assert!(!core.running);
        assert_eq!(cycles, 1);
    }

    // A fault whose entry ends in a derived lockup (the vectors cannot be read) is not an entry
    // either: it costs the one cycle.
    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_a_fault_that_ends_in_a_derived_lockup_costs_one_cycle() {
        let mut core = Processor::new();
        core.write32(0xE000_ED08, 0x1fff_ff80).unwrap(); // no vector can be read
        core.write32(0xE000_ED24, 1 << 18).unwrap(); // UsageFault enabled
        core.set_msp(0x2000_0200);
        let cycles = core.handle_fault(Fault::UndefInstr, 0x100);
        assert!(core.is_locked_up());
        assert_eq!(cycles, 1);
    }

    // The lockup address of a derived lockup (a failed stacking or vector read) is 0xFFFFFFFE.
    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_a_derived_lockup_sets_the_pc_to_the_lockup_address() {
        let mut core = Processor::new();
        core.write32(0xE000_ED08, 0x1fff_ff80).unwrap();
        core.write32(0xE000_ED24, 1 << 18).unwrap();
        core.set_msp(0x2000_0200);
        core.handle_fault(Fault::UndefInstr, 0x100);
        assert_eq!(core.get_pc(), 0xffff_fffe);
    }

    // ---- alignment faults (A3.2.1, B3.2.8) ---------------------------------------------------

    #[cfg(not(feature = "armv6m"))]
    enum Opcode {
        Half(u16),
        Wide(u32),
    }

    // Runs one instruction with the base register (or SP, for the stack ones) at `address`, with
    // UsageFault enabled, and says whether it raised UNALIGNED (CFSR bit 24).
    #[cfg(not(feature = "armv6m"))]
    fn raises_unaligned(opcode: &Opcode, address: u32, unalign_trp: bool, stack: bool) -> bool {
        use crate::core::instruction::instruction_size;
        let mut core = Processor::new();
        #[cfg(feature = "has-fp")]
        {
            core.cpacr = 0x00f0_0000;
        }
        core.write32(0xE000_ED24, 1 << 18).unwrap(); // USGFAULTENA
        core.ccr = if unalign_trp { 1 << 3 } else { 0 };
        core.set_msp(if stack { address } else { 0x2000_0400 });
        core.set_r(Reg::R0, address);
        core.set_r(Reg::R1, 1);
        core.set_r(Reg::R2, 2);
        core.set_pc(0x100);
        let instruction = match opcode {
            Opcode::Half(op) => crate::decoder::decode_16(*op),
            Opcode::Wide(op) => crate::decoder::decode_32(*op),
        };
        core.execute(&instruction, instruction_size(&instruction));
        let faulted = core.cfsr & (1 << 24) != 0;
        if faulted {
            assert_eq!(core.psr.get_isr_number(), 6, "UsageFault");
        }
        faulted
    }

    // The accesses that always fault on an address that is not word aligned, or halfword aligned
    // for LDREXH and STREXH, whatever CCR.UNALIGN_TRP says; and work on an aligned one.
    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_the_always_aligned_accesses_fault_on_unaligned_addresses() {
        use Opcode::{Half, Wide};
        // (name, opcode, uses SP, offset from an aligned address that is not aligned enough)
        #[cfg_attr(not(feature = "has-fp"), allow(unused_mut))]
        let mut rows: Vec<(&str, Opcode, bool, u32)> = vec![
            ("ldm.w", Wide(0xe890_0006), false, 2),
            ("stm.w", Wide(0xe880_0006), false, 2),
            ("stmdb", Wide(0xe900_0006), false, 2),
            ("ldrd", Wide(0xe9d0_1200), false, 2),
            ("strd", Wide(0xe9c0_1200), false, 2),
            ("push {r1}", Half(0xb402), true, 2),
            ("pop {r1}", Half(0xbc02), true, 2),
            ("push", Half(0xb406), true, 2),
            ("pop", Half(0xbc06), true, 2),
            ("push.w", Wide(0xe92d_0006), true, 2),
            ("pop.w", Wide(0xe8bd_0006), true, 2),
            ("ldrex", Wide(0xe850_1f00), false, 2),
            ("strex", Wide(0xe840_1200), false, 2),
            ("ldrexh", Wide(0xe8d0_1f5f), false, 1),
            ("strexh", Wide(0xe8c0_1f52), false, 1),
        ];
        #[cfg(feature = "has-fp")]
        rows.extend(vec![
            ("vldr d", Wide(0xed90_0b00), false, 2),
            ("vldr s", Wide(0xed90_0a00), false, 2),
            ("vstr s", Wide(0xed80_0a00), false, 2),
            ("vldmia", Wide(0xec90_0b04), false, 2),
            ("vstmia", Wide(0xec80_0a02), false, 2),
            ("vldmia s", Wide(0xec90_0a02), false, 2),
            ("vstmia d", Wide(0xec80_0b04), false, 2),
            ("vpush", Wide(0xed2d_0a01), true, 2),
            ("vpop", Wide(0xecbd_0a01), true, 2),
        ]);
        let mut wrong = Vec::new();
        for (name, opcode, stack, offset) in &rows {
            let base = 0x2000_0300;
            for trp in [false, true] {
                if raises_unaligned(opcode, base, trp, *stack) {
                    wrong.push(format!("{name}: aligned faulted (trp {trp})"));
                }
                if !raises_unaligned(opcode, base + offset, trp, *stack) {
                    wrong.push(format!(
                        "{name}: unaligned at +{offset} did not fault (trp {trp})"
                    ));
                }
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    // LDR, STR, LDRH, LDRSH, STRH and TBH support unaligned addresses, and fault on them only
    // when CCR.UNALIGN_TRP is set; the byte accesses and the single-register PUSH and POP forms of
    // 32 bits (STR and LDR with write back) never do.
    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_the_other_accesses_fault_on_unaligned_addresses_only_with_unalign_trp() {
        use Opcode::{Half, Wide};
        // (name, opcode, stack, offset, faults with UNALIGN_TRP)
        let rows: Vec<(&str, Opcode, bool, u32, bool)> = vec![
            ("ldr", Half(0x6801), false, 2, true),
            ("ldr.w", Wide(0xf8d0_1000), false, 1, true),
            ("str", Half(0x6001), false, 2, true),
            ("ldrh", Half(0x8801), false, 1, true),
            ("strh", Half(0x8001), false, 1, true),
            ("ldrsh.w", Wide(0xf9b0_1000), false, 1, true),
            ("tbh", Wide(0xe8d0_f011), false, 1, true),
            ("ldrb", Half(0x7801), false, 1, false),
            ("strb", Half(0x7001), false, 1, false),
            ("ldrex b", Wide(0xe8d0_1f4f), false, 1, false),
            ("push {r1} (T3)", Wide(0xf84d_1d04), true, 2, true),
            ("pop {r1} (T3)", Wide(0xf85d_1b04), true, 2, true),
        ];
        let mut wrong = Vec::new();
        for (name, opcode, stack, offset, trapped) in &rows {
            // TBH takes the table address from r0 + r1 * 2 with r1 = 1, so it starts 2 lower
            // TBH takes the table address from r0 + r1 * 2 with r1 = 1, so r0 starts 2 lower
            let base = 0x2000_0300
                - if matches!(opcode, Wide(0xe8d0_f011)) {
                    2
                } else {
                    0
                };
            if raises_unaligned(opcode, base + offset, false, *stack) {
                wrong.push(format!("{name}: faulted without UNALIGN_TRP"));
            }
            if raises_unaligned(opcode, base + offset, true, *stack) != *trapped {
                wrong.push(format!(
                    "{name}: with UNALIGN_TRP, expected fault {trapped}"
                ));
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    // The fault is recorded when UsageFault is disabled too: it is escalated to HardFault (FORCED),
    // and the UFSR bit is set all the same (B1.5.14).
    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_an_alignment_fault_is_escalated_with_usagefault_disabled() {
        use crate::core::instruction::instruction_size;
        let mut core = Processor::new();
        core.set_msp(0x2000_0400);
        core.set_r(Reg::R0, 0x2000_0302);
        core.set_pc(0x100);
        let instruction = crate::decoder::decode_32(0xe890_0006); // ldm.w r0, {r1, r2}
        core.execute(&instruction, instruction_size(&instruction));
        assert_eq!(core.psr.get_isr_number(), 3);
        assert_eq!(core.cfsr & (1 << 24), 1 << 24);
        assert_eq!(core.hfsr & (1 << 30), 1 << 30, "FORCED");
        // nothing was loaded and the base was not written back
        assert_eq!(core.get_r(Reg::R1), 0);
    }

    // The fault comes before any access or write back: an LDM that is refused loads nothing, and
    // a POP leaves SP where it was.
    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_a_refused_access_changes_nothing() {
        use crate::core::instruction::instruction_size;
        let mut core = Processor::new();
        core.write32(0xE000_ED24, 1 << 18).unwrap();
        core.ccr = 0;
        core.set_msp(0x2000_0302);
        core.set_pc(0x100);
        let instruction = crate::decoder::decode_16(0xbc06); // pop {r1, r2}
        core.execute(&instruction, instruction_size(&instruction));
        assert_eq!(core.cfsr & (1 << 24), 1 << 24);
        // UsageFault entry stacked the frame on the same stack: SP is 0x20 below what it was
        assert_eq!(core.get_msp(), 0x2000_0302 - 0x20);
    }

    // The fault is precise: the base register keeps its value for every form that writes it back,
    // and SP for POP (T3 too, whose only check is the UNALIGN_TRP one).
    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_a_faulting_access_leaves_the_base_register_alone() {
        use crate::core::instruction::instruction_size;
        let mut wrong = Vec::new();
        // (name, opcode, base register value, UNALIGN_TRP)
        for (name, opcode, base, trp) in [
            ("str r1, [r0, #-4]!", 0xf840_1d04_u32, 0x2000_0301_u32, true),
            ("str r1, [r0], #4", 0xf840_1b04, 0x2000_0301, true),
            ("strh r1, [r0, #-2]!", 0xf820_1d02, 0x2000_0303, true),
            ("ldr r1, [r0, #-4]!", 0xf850_1d04, 0x2000_0301, true),
            ("ldrh r1, [r0], #2", 0xf830_1b02, 0x2000_0301, true),
            ("strd r1, r2, [r0, #-8]!", 0xe960_1202, 0x2000_0302, false),
            ("ldrd r1, r2, [r0], #8", 0xe8f0_1202, 0x2000_0302, false),
            ("strb r1, [r0, #-4]!", 0xf800_1d04, 0x1fff_0004, false), // unmapped: a bus fault
        ] {
            let mut core = Processor::new();
            core.write32(0xE000_ED24, (1 << 18) | (1 << 17)).unwrap();
            core.ccr = if trp { 1 << 3 } else { 0 };
            core.set_msp(0x2000_0400);
            core.set_r(Reg::R0, base);
            core.set_pc(0x100);
            let instruction = crate::decoder::decode_32(opcode);
            core.execute(&instruction, instruction_size(&instruction));
            if core.get_r(Reg::R0) != base {
                wrong.push(format!("{name}: r0 is {:#x}", core.get_r(Reg::R0)));
            }
        }
        // POP T3 with UNALIGN_TRP
        let mut core = Processor::new();
        core.write32(0xE000_ED24, 1 << 18).unwrap();
        core.ccr = 1 << 3;
        core.set_msp(0x2000_0302);
        core.set_pc(0x100);
        let instruction = crate::decoder::decode_32(0xf85d_1b04); // pop {r1}
        core.execute(&instruction, instruction_size(&instruction));
        if core.cfsr & (1 << 24) == 0 {
            wrong.push("pop {r1}: no fault".to_string());
        }
        // the entry lowered SP by the frame only
        if core.get_msp() != 0x2000_0302 - 0x20 {
            wrong.push(format!("pop {{r1}}: SP is {:#x}", core.get_msp()));
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    // LDREXH and STREXH need halfword alignment only.
    #[test]
    #[cfg(not(feature = "armv6m"))]
    fn test_halfword_exclusives_accept_halfword_aligned_addresses() {
        for opcode in [0xe8d0_1f5f_u32, 0xe8c0_1f52] {
            let opcode = Opcode::Wide(opcode);
            assert!(!raises_unaligned(&opcode, 0x2000_0302, false, false));
            assert!(raises_unaligned(&opcode, 0x2000_0301, false, false));
        }
    }

    // ARMv6-M has no unaligned accesses: they fault, to HardFault (there is no UsageFault), and
    // the byte accesses do not; CCR reads UNALIGN_TRP and STKALIGN as one and ignores writes.
    #[test]
    #[cfg(feature = "armv6m")]
    fn test_armv6m_unaligned_accesses_fault_to_hardfault() {
        use crate::bus::Bus;
        use crate::core::instruction::instruction_size;
        for (name, opcode, faults) in [
            ("ldr", 0x6801_u16, true),
            ("ldrh", 0x8801, true),
            ("ldm", 0xc806, true),
            ("ldrb", 0x7801, false),
        ] {
            let mut core = Processor::new();
            core.set_msp(0x2000_0400);
            core.set_r(Reg::R0, 0x2000_0301);
            core.set_pc(0x100);
            let instruction = crate::decoder::decode_16(opcode);
            core.execute(&instruction, instruction_size(&instruction));
            assert_eq!(core.psr.get_isr_number() == 3, faults, "{name}");
        }
        let mut core = Processor::new();
        core.write32(0xE000_ED14, 0).unwrap();
        assert_eq!(core.read32(0xE000_ED14), Ok((1 << 3) | (1 << 9)));
    }
}
