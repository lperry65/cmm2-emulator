//! Building the machine a run starts from: the firmware, the board, and the `--flash` and `--sd`
//! files loaded into it.

use crate::args::RunOptions;
use crate::error::RunError;
use crate::local_clock::LocalClock;
use crate::storage::{load_flash, load_sd};
use crate::trace::{TraceOut, trace_out};
use crate::usb_keys::UsbKeys;
use cmm2_core::{
    Console, Cpu, Firmware, HidKeyboard, Irqs, KeyReport, Profile, SdSlot, cmm2_memory,
    cmm2_memory_with_rng_seed,
};
use std::cell::RefCell;
use std::rc::Rc;

pub(crate) struct Machine {
    pub(crate) cpu: Cpu,
    pub(crate) console: Console,
    pub(crate) sd: SdSlot,
    pub(crate) trace: Option<Rc<RefCell<TraceOut>>>,
}

/// Names the layout of the settings for a firmware I do not know, in a scratch run with a
/// `--flash` file. This is for the tests, whose programs are not a firmware, and for trying a
/// new release by hand: what it names may be wrong for the image, and then the settings are too.
/// A firmware I know keeps its own. The board never looks at it.
pub(crate) const LAYOUT_VARIABLE: &str = "CMM2_SETTINGS_LAYOUT";

/// The same for the board (`--burn` and the start of the board), for the tests that burn a
/// program. For tests only, and not in the README. Set by accident to `b5` it would burn an
/// unknown firmware with b5's layout, and the board's settings file would then hold that
/// firmware's code and not its settings.
pub(crate) const BOARD_LAYOUT_VARIABLE: &str = "CMM2_BOARD_LAYOUT_FOR_TESTS";

/// What the variable says, if it is set.
pub(crate) fn layout_named_in(variable: &str) -> Option<String> {
    std::env::var_os(variable).map(|name| name.to_string_lossy().into_owned())
}

/// The firmware with the layout that `name` (the value of `variable`) gives it, if it has none.
pub(crate) fn with_layout(
    firmware: Firmware,
    variable: &str,
    name: Option<String>,
) -> Result<Firmware, RunError> {
    let Some(name) = name.filter(|_| firmware.profile().is_none()) else {
        return Ok(firmware);
    };
    match Profile::named(&name) {
        Some(profile) => Ok(firmware.with_profile(profile)),
        None => Err(RunError::BadLayoutName {
            variable: variable.to_string(),
            name,
        }),
    }
}

/// Builds the machine for the firmware at `path`. `variable` is the one that may name a layout for
/// it, which only matters when there are settings to keep: with no `--flash` a bad value is
/// nothing. `on_board` says this is the board starting (not a `--firmware` scratch run), which
/// with an identified firmware is what opens the v2 settings format (the gate, issue 82).
pub(crate) fn build_machine(
    path: &str,
    options: &RunOptions,
    variable: &str,
    on_board: bool,
) -> Result<Machine, RunError> {
    let mut firmware = Firmware::load(path)?;
    if options.flash.is_some() {
        firmware = with_layout(firmware, variable, layout_named_in(variable))?;
    }
    let irqs = Irqs::new();
    // The RNG takes its values from the host's entropy unless the run fixes the seed.
    let memory = match options.rng_seed {
        Some(seed) => cmm2_memory_with_rng_seed(&irqs, seed),
        None => cmm2_memory(&irqs),
    };
    let trace = trace_out(options)?;
    let console = memory.console();
    let sd = memory.sd_slot();
    // The board's DS3231 is always fitted, and keeps the host's time.
    memory.ds3231().set_host_clock(Box::new(LocalClock));
    let mut cpu = Cpu::new(&firmware, memory, irqs)?;
    // The gate, once here at the start and never at a save: v2 only for a board boot whose
    // firmware `Profile::identify` recognised. A scratch run, and a board whose layout came
    // from a variable, keep the raw file: a named layout is an assumption about an image I
    // do not know, and the v2 header states the image as fact.
    if on_board && firmware.identified() {
        cpu.use_settings_v2();
    }
    if let Some(path) = &options.flash {
        // A board reads the raw layout for one reason only: a variable named a layout for a
        // firmware I cannot recognise, since an image I know puts it on v2 and a scratch run is
        // raw whatever a variable says. `load_flash` refuses a firmware with no layout at all
        // before it says anything about a file, so naming the variable here is never a guess.
        let labelled_by = on_board.then_some(variable);
        load_flash(&mut cpu, path, labelled_by)?;
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
