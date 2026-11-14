//! Running a closure and saving the files whichever way it ends, a panic included.

use crate::args::RunOptions;
use crate::error::{RunError, say};
use crate::output::Output;
use crate::panic_hook::{HOLDING, PANIC_SEEN, say_late_panic};
use crate::storage::save_files;
use cmm2_core::{Cpu, SdSlot};
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

/// Runs `run`, and writes the files back whichever way it ends. A panic in it is saved from too
/// and then carries on as it was, so the guest's session is not lost to a bug in the emulator.
/// What this cannot cover is the process being killed outright (SIGKILL), or the power going.
pub(crate) fn run_and_save<T>(
    cpu: &mut Cpu,
    sd: &SdSlot,
    options: &RunOptions,
    run: impl FnOnce(&mut Cpu) -> T,
) -> (T, Result<(), RunError>) {
    run_and_save_saying(cpu, sd, options, run, &mut Output::stderr())
}

/// `run_and_save`, with where a panic's message that was held back is said.
pub(crate) fn run_and_save_saying<T>(
    cpu: &mut Cpu,
    sd: &SdSlot,
    options: &RunOptions,
    run: impl FnOnce(&mut Cpu) -> T,
    late: &mut impl Write,
) -> (T, Result<(), RunError>) {
    // A panic's message is held until the files are written; see `install_panic_hook`.
    HOLDING.store(true, std::sync::atomic::Ordering::Release);
    let ran = catch_unwind(AssertUnwindSafe(|| run(cpu)));
    HOLDING.store(false, std::sync::atomic::Ordering::Release);
    PANIC_SEEN.with(|seen| seen.set(false));
    match ran {
        Ok(result) => (result, save_files(cpu, sd, options)),
        Err(panic) => {
            if let Err(err) = save_files(cpu, sd, options) {
                say(err);
            }
            // The files are written, and now the message, which could have waited for a reader.
            say_late_panic(late);
            resume_unwind(panic)
        }
    }
}

#[cfg(test)]
mod tests;
