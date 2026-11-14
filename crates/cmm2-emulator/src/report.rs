//! What a run says and writes at its end: the summary, the guest's console, the screenshot, and
//! the resets as they happen.

use crate::args::RunOptions;
use crate::error::{RunError, say};
use crate::output::is_signal_end;
use cmm2_core::{AccessKind, Console, Cpu, Stop};
use std::io;
use std::io::Write;

/// How many unmapped accesses the summary lists; the count covers all of them.
const UNMAPPED_SHOWN: usize = 5;

/// Writes the screen to `--screenshot`, if it was given.
pub(crate) fn write_screenshot(cpu: &mut Cpu, options: &RunOptions) -> Result<(), RunError> {
    let Some(path) = &options.screenshot else {
        return Ok(());
    };
    let frame = cpu.memory().frame().map_err(RunError::Frame)?;
    std::fs::write(path, frame.to_ppm()).map_err(|source| RunError::Screenshot {
        path: path.clone(),
        source,
    })
}

/// Says on stderr, outside the guest's console stream, that the guest reset the machine, once
/// for each reset after the first `seen`. Returns how many there have been.
pub(crate) fn report_resets(cpu: &Cpu, seen: u32) -> u32 {
    for _ in seen..cpu.resets() {
        say("system reset");
    }
    cpu.resets()
}

/// What the guest sent to its console, after a run with no terminal attached.
pub(crate) fn print_console(console: &Console, out: &mut impl Write) -> io::Result<()> {
    let output = console.take_output();
    if output.is_empty() {
        return Ok(());
    }
    writeln!(out, "console output ({} bytes):", output.len())?;
    let text = String::from_utf8_lossy(&output);
    write!(out, "{text}")?;
    if !text.ends_with('\n') {
        writeln!(out)?;
    }
    Ok(())
}

pub(crate) fn print_summary(
    cpu: &mut Cpu,
    stop: &Stop,
    interrupted: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    match stop {
        Stop::InstructionLimit if interrupted => writeln!(out, "stopped: interrupted by a signal")?,
        Stop::InstructionLimit => writeln!(out, "stopped: instruction limit")?,
        Stop::Address(addr) => writeln!(out, "stopped: address {addr:#010X}")?,
        Stop::Fault(fault) => writeln!(out, "stopped: {}", fault.description)?,
    }
    writeln!(out, "instructions {}", cpu.instruction_count())?;
    writeln!(out, "pc {:#010X}", cpu.pc())?;
    let unmapped = cpu.memory().take_unmapped();
    if unmapped.is_empty() {
        return Ok(());
    }
    writeln!(out, "unmapped accesses {}", unmapped.len())?;
    for access in unmapped.iter().take(UNMAPPED_SHOWN) {
        let kind = match access.kind {
            AccessKind::Read => "read",
            AccessKind::Write(_) => "write",
        };
        writeln!(
            out,
            "  {:#010X} {kind} {:#010X} size {}",
            access.pc, access.addr, access.size
        )?;
    }
    Ok(())
}

/// What writing to standard output came to: a write that a signal gave up on is the run ending
/// and not an error.
pub(crate) fn output_result(result: io::Result<()>) -> Result<(), RunError> {
    match result {
        Err(err) if !is_signal_end(&err) => Err(RunError::Console(err)),
        _ => Ok(()),
    }
}
