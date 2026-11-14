//! The `--trace` and `--trace-mmio` output: where it goes, and how an access is written.

use crate::args::RunOptions;
use crate::error::RunError;
use crate::output::{Output, is_signal_end};
use cmm2_core::{PeripheralAccess, Svd};
use std::cell::RefCell;
use std::fmt;
use std::fs::File;
use std::io;
use std::io::{BufWriter, Write};
use std::rc::Rc;

/// Where the trace goes. The instruction lines and the peripheral lines share one, so they
/// stay in order. The first write error is kept for the end, as the callbacks cannot return it.
pub(crate) struct TraceOut {
    out: BufWriter<Box<dyn Write>>,
    error: Option<io::Error>,
}

impl TraceOut {
    pub(crate) fn line(&mut self, line: fmt::Arguments) {
        if self.error.is_none()
            && let Err(err) = writeln!(self.out, "{line}")
        {
            self.error = Some(err);
        }
    }

    // A write that a signal gave up on is the run ending, not a trace that failed.
    fn finish(&mut self) -> io::Result<()> {
        match self.error.take() {
            Some(err) => Err(err),
            None => self.out.flush(),
        }
        .or_else(|err| {
            if is_signal_end(&err) {
                Ok(())
            } else {
                Err(err)
            }
        })
    }
}

pub(crate) fn trace_out(options: &RunOptions) -> Result<Option<Rc<RefCell<TraceOut>>>, RunError> {
    let out: Box<dyn Write> = match &options.trace {
        Some(path) => Box::new(Output::file(File::create(path).map_err(|source| {
            RunError::Trace {
                path: path.clone(),
                source,
            }
        })?)),
        None if options.trace_mmio => Box::new(Output::stdout()),
        None => return Ok(None),
    };
    Ok(Some(Rc::new(RefCell::new(TraceOut {
        out: BufWriter::new(out),
        error: None,
    }))))
}

/// A register name for the trace, with the byte offset when the access is not at its start.
fn register_name(addr: u32) -> String {
    match Svd::stm32h743().lookup(addr) {
        Some(found) if found.offset != 0 => format!("{} +{}", found.name, found.offset),
        Some(found) => found.name.to_string(),
        None => "?".to_string(),
    }
}

pub(crate) fn access_line(access: &PeripheralAccess) -> String {
    format!(
        "    {} {} {:#010X} size {} = {:#010X}",
        if access.write { 'W' } else { 'R' },
        register_name(access.addr),
        access.addr,
        access.size,
        access.value
    )
}

pub(crate) fn finish_trace(trace: Option<Rc<RefCell<TraceOut>>>) -> Result<(), RunError> {
    match trace {
        Some(trace) => trace.borrow_mut().finish().map_err(RunError::Write),
        None => Ok(()),
    }
}
