use crate::Processor;

use crate::core::fault::Fault;
#[cfg(feature = "has-fp")]
use crate::executor::FloatingPointChecks;
use crate::executor::{ExecuteSuccess, ExecutorHelper};

use super::ExecuteResult;

/// Coprocessor instructions
pub trait IsaCoprocessor {
    fn exec_coprocessor(&mut self, coproc: u8) -> ExecuteResult;
}

impl IsaCoprocessor for Processor {
    /// A coprocessor instruction for a coprocessor that does not exist: the only coprocessors of
    /// the Cortex-M7 are CP10 and CP11, the FPU, so no other number is accepted
    /// (`Coproc_Accepted` is false, A7.7.72 and the others), and the instruction takes a
    /// `UsageFault` with UFSR.NOCP (A5.3.18). Which instruction it was does not change that, nor do
    /// its operands. The UFSR bit is recorded where every fault's status is, from `Fault::Nocp`.
    ///
    /// CP10 and CP11 with the FPU built in exist: these are the encodings that no FP instruction
    /// matched, which are UNDEFINED (UFSR.UNDEFINSTR) when CPACR enables the FPU and NOCP when it
    /// does not (A2.6, A5.3.18).
    fn exec_coprocessor(&mut self, coproc: u8) -> ExecuteResult {
        if self.condition_passed() {
            #[cfg(feature = "has-fp")]
            if coproc == 10 || coproc == 11 {
                self.check_vfp_enabled()?;
                return Err(Fault::UndefInstr);
            }
            let _ = coproc;
            return Err(Fault::Nocp);
        }
        Ok(ExecuteSuccess::NotTaken)
    }
}
