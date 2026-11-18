//! The run with no console and no window: to the limit, a stop or a fault, then the summary.

use crate::args::RunOptions;
use crate::error::{RunError, settle};
use crate::machine::Machine;
use crate::output::Output;
use crate::report::{output_result, print_console, print_summary, report_resets, write_screenshot};
use crate::run_and_save::run_and_save;
use crate::signals;
use crate::slice::{SLICE, run_slice};
use crate::trace::finish_trace;
use cmm2_core::Stop;
use std::io::{BufWriter, Write};

/// Returns whether the run ended without a fault. The run goes in slices so that a signal ends
/// it too, as `run_live` does.
pub(crate) fn run_firmware(machine: Machine, options: &RunOptions) -> Result<bool, RunError> {
    let Machine {
        mut cpu,
        console,
        sd,
        trace,
    } = machine;

    let limit = options.run.unwrap_or(u64::MAX);
    signals::begin_run();
    let (stop, saved) = run_and_save(&mut cpu, &sd, options, |cpu| {
        loop {
            let budget = limit.saturating_sub(cpu.instruction_count()).min(SLICE);
            let stop = run_slice(cpu, options, trace.as_ref(), budget);
            if stop != Stop::InstructionLimit
                || cpu.instruction_count() >= limit
                || signals::caught()
            {
                break stop;
            }
        }
    });
    report_resets(&cpu, 0);
    let traced = finish_trace(trace);

    // A fault is in the summary, which comes first; the errors after it are all said.
    let ok = !matches!(stop, Stop::Fault(_));
    let mut out = BufWriter::new(Output::stdout());
    let printed = print_summary(&mut cpu, &stop, signals::caught(), &mut out)
        .and_then(|()| print_console(&console, &mut out))
        .and_then(|()| out.flush());
    let shot = write_screenshot(&mut cpu, options);
    settle(
        Ok(ok),
        [traced, output_result(printed).and(shot), saved],
        &mut Output::stderr(),
    )
}
