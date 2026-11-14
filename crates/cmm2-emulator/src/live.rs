//! The run with the console on the host terminal, or the screen and keyboard in a window.

use crate::args::RunOptions;
use crate::error::{RunError, settle};
use crate::machine::{Machine, build_machine, plug_in_keyboard};
use crate::output::{Output, is_signal_end};
use crate::pacing::{Pacer, RealTime};
use crate::report::{output_result, print_console, report_resets, write_screenshot};
use crate::run_and_save::run_and_save;
use crate::script::Script;
use crate::signals;
use crate::slice::{SLICE, run_slice};
use crate::terminal::RawMode;
use crate::trace::finish_trace;
use crate::window::Window;
use cmm2_core::{CORE_HZ, Console, Stop};
use std::io;
use std::io::{BufWriter, Read, Write};
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::thread;

/// Ctrl-] ends a `--console` run; the terminal is in raw mode, so Ctrl-C goes to the guest.
const QUIT_KEY: u8 = 0x1D;

/// Runs with the console on the host terminal and the screen and keyboard in a window, as asked,
/// until the quit key, the window is closed, the limit, `--stop-at`, a fault or a signal
/// (SIGINT, SIGTERM or SIGHUP). The machine is held to its own speed unless `--unpaced`. The
/// summary is left out, as the screen belongs to the guest. The window's keys are a USB
/// keyboard unless `--console-keys`. Without `--console` the guest's output is printed at the
/// end. The `--flash` and `--sd` files are written whichever way the run ends, and when the
/// emulator panics.
pub(crate) fn run_live(path: &str, mut options: RunOptions) -> Result<bool, RunError> {
    let Machine {
        mut cpu,
        console,
        sd,
        trace,
    } = build_machine(path, &options)?;
    // The window's keys are the USB keyboard's, unless it is asked to type into the console.
    let usb_keys = (options.window && !options.console_keys).then(|| plug_in_keyboard(&cpu));
    let mut window = options
        .window
        .then(|| signals::with_blocked(|| Window::open(usb_keys)))
        .transpose()
        .map_err(RunError::Window)?;
    let _raw = options.console.then(RawMode::enable);
    let keys = options.console.then(|| signals::with_blocked(read_stdin));
    let mut script = Script::new(std::mem::take(&mut options.on));
    let limit = options.run.unwrap_or(u64::MAX);
    let mut stdout = Output::stdout();
    let mut resets = 0;
    let clock = cpu.memory().clock();
    let mut pacer = (!options.unpaced).then(|| Pacer::new(RealTime::start(), CORE_HZ));

    // Every thread that is to have the signals blocked has been made, so the main one takes them.
    signals::begin_run();
    let (result, saved) = run_and_save(&mut cpu, &sd, &options, |cpu| {
        loop {
            if signals::caught() {
                break Ok(true);
            }
            let budget = limit.saturating_sub(cpu.instruction_count()).min(SLICE);
            let stop = run_slice(cpu, &options, trace.as_ref(), budget);
            if keys.is_some()
                && let Err(err) = show_output(&console, &mut script, &mut stdout)
            {
                // A write that a signal cut short is the signal ending the run.
                break if is_signal_end(&err) {
                    Ok(true)
                } else {
                    Err(RunError::Console(err))
                };
            }
            resets = report_resets(cpu, resets);
            if let Some(window) = &mut window {
                match window.update(|bytes| console.push_input(bytes), || cpu.memory().frame()) {
                    Ok(true) => {}
                    Ok(false) => break Ok(true),
                    Err(err) => break Err(RunError::Window(err)),
                }
            }
            match stop {
                Stop::Fault(fault) => break Err(RunError::Fault(fault.description)),
                Stop::Address(_) => break Ok(true),
                Stop::InstructionLimit if cpu.instruction_count() >= limit => break Ok(true),
                Stop::InstructionLimit => {}
            }
            if let Some(keys) = &keys
                && !send_keys(&console, keys)
            {
                break Ok(true);
            }
            if let Some(pacer) = &mut pacer {
                pacer.pace(clock.now());
            }
        }
    });
    // The console and the screen are only for a run that did not fail.
    let shown = if result.is_ok() {
        let mut out = BufWriter::new(Output::stdout());
        let printed = if keys.is_none() {
            print_console(&console, &mut out).and_then(|()| out.flush())
        } else {
            Ok(())
        };
        let shot = write_screenshot(&mut cpu, &options);
        output_result(printed).and(shot)
    } else {
        Ok(())
    };
    settle(
        result,
        [finish_trace(trace), shown, saved],
        &mut Output::stderr(),
    )
}

/// Writes what the guest sent to the terminal and types the keys the script has for it.
fn show_output(console: &Console, script: &mut Script, out: &mut impl Write) -> io::Result<()> {
    let output = console.take_output();
    if output.is_empty() {
        return Ok(());
    }
    out.write_all(&output)?;
    console.push_input(&script.feed(&output));
    Ok(())
}

/// Passes the typed keys on to the guest. Returns false once the quit key has been typed.
fn send_keys(console: &Console, keys: &Receiver<Vec<u8>>) -> bool {
    while let Ok(bytes) = keys.try_recv() {
        let (typed, quit) = until_quit(&bytes);
        console.push_input(typed);
        if quit {
            return false;
        }
    }
    true
}

/// The keys before the quit key, and whether it was among them.
fn until_quit(bytes: &[u8]) -> (&[u8], bool) {
    match bytes.iter().position(|&byte| byte == QUIT_KEY) {
        Some(at) => (&bytes[..at], true),
        None => (bytes, false),
    }
}

/// Reads stdin on a thread, as a read blocks. At the end of stdin the thread stops and the
/// emulator carries on.
fn read_stdin() -> Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        let mut buffer = [0; 256];
        while let Ok(count @ 1..) = stdin.read(&mut buffer) {
            if sender.send(buffer[..count].to_vec()).is_err() {
                break;
            }
        }
    });
    receiver
}

#[cfg(test)]
mod tests;
