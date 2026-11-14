//! Running a slice of instructions, with the traces switched on at `--trace-from`.

use crate::args::RunOptions;
use crate::trace::{TraceOut, access_line};
use cmm2_core::{Cpu, Stop};
use std::cell::RefCell;
use std::rc::Rc;

/// How many instructions run between looks at the console in `--console` mode.
pub(crate) const SLICE: u64 = 50_000;

/// Runs up to `budget` instructions. The traces start at `--trace-from`; until then this is a
/// plain run, so a slice that crosses that point is split in two. The peripheral trace is only
/// hooked in at that point, so it cannot write earlier.
pub(crate) fn run_slice(
    cpu: &mut Cpu,
    options: &RunOptions,
    trace: Option<&Rc<RefCell<TraceOut>>>,
    budget: u64,
) -> Stop {
    let Some(sink) = trace else {
        return cpu.run_until(budget, options.stop_at);
    };
    let untraced = options
        .trace_from
        .unwrap_or(0)
        .saturating_sub(cpu.instruction_count())
        .min(budget);
    if untraced > 0 {
        let stop = cpu.run_until(untraced, options.stop_at);
        if stop != Stop::InstructionLimit || untraced == budget {
            return stop;
        }
    }
    let budget = budget - untraced;
    if options.trace_mmio {
        let sink = sink.clone();
        cpu.memory()
            .set_peripheral_trace(Some(Box::new(move |access| {
                sink.borrow_mut()
                    .line(format_args!("{}", access_line(access)));
            })));
    }
    if options.trace.is_none() {
        return cpu.run_until(budget, options.stop_at);
    }
    cpu.run_traced(budget, options.stop_at, |pc, text| {
        sink.borrow_mut().line(format_args!("{pc:#010X}  {text}"));
    })
}
