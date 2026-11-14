//! Building the machine a run starts from: the firmware, the board, and the `--flash` and `--sd`
//! files loaded into it.

use crate::args::RunOptions;
use crate::error::RunError;
use crate::local_clock::LocalClock;
use crate::storage::{load_flash, load_sd};
use crate::trace::{TraceOut, trace_out};
use crate::usb_keys::UsbKeys;
use cmm2_core::{Console, Cpu, Firmware, HidKeyboard, Irqs, KeyReport, SdSlot, cmm2_memory};
use std::cell::RefCell;
use std::rc::Rc;

pub(crate) struct Machine {
    pub(crate) cpu: Cpu,
    pub(crate) console: Console,
    pub(crate) sd: SdSlot,
    pub(crate) trace: Option<Rc<RefCell<TraceOut>>>,
}

pub(crate) fn build_machine(path: &str, options: &RunOptions) -> Result<Machine, RunError> {
    let firmware = Firmware::load(path)?;
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let trace = trace_out(options)?;
    let console = memory.console();
    let sd = memory.sd_slot();
    // The board's DS3231 is always fitted, and keeps the host's time.
    memory.ds3231().set_host_clock(Box::new(LocalClock));
    let mut cpu = Cpu::new(&firmware, memory, irqs)?;
    if let Some(path) = &options.flash {
        load_flash(&mut cpu, path)?;
    }
    if let Some(path) = &options.sd {
        load_sd(&sd, path)?;
    }
    Ok(Machine {
        cpu,
        console,
        sd,
        trace,
    })
}

/// Plugs a USB keyboard into the port, and returns the keys that drive it. The core attaches
/// nothing itself, so that a run with no window sees no keyboard.
pub(crate) fn plug_in_keyboard(cpu: &Cpu) -> UsbKeys {
    let report = KeyReport::new();
    cpu.memory()
        .usb_port()
        .attach(HidKeyboard::new(report.clone()));
    UsbKeys::new(report)
}

#[cfg(test)]
mod tests;
