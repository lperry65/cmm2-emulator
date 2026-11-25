//! Starting a run: where its firmware, settings and card come from. A run that names `--firmware`
//! is a machine of its own from that file, and nothing of the board is read or written; any
//! other run is on the board, which is switched on, and kept switched on for the run.

use crate::args::{RunOptions, Start};
use crate::batch::run_firmware;
use crate::board::{Board, about_the_remembered_card, board_dir};
use crate::error::{RunError, say};
use crate::live::run_live;
use crate::machine::{
    BOARD_LAYOUT_VARIABLE, LAYOUT_VARIABLE, Machine, build_machine, layout_named_in, with_layout,
};
use cmm2_core::{Firmware, NoLayout};

/// How the card of a board run was settled.
enum Card {
    /// `--sd` named it: it is remembered once it has been read.
    Given(String),
    /// The board's own, from an earlier run.
    Remembered,
    /// None in the socket.
    Empty,
}

pub(crate) fn run(start: Start, options: RunOptions) -> Result<bool, RunError> {
    match start {
        Start::File(path) => {
            let machine = build_machine(&path, &options, LAYOUT_VARIABLE, false)?;
            go(machine, options)
        }
        Start::Board => on_the_board(options),
    }
}

/// Runs what the options ask for: with the console or window when they are on, as a batch if not.
fn go(machine: Machine, options: RunOptions) -> Result<bool, RunError> {
    if options.console || options.window {
        run_live(machine, options)
    } else {
        run_firmware(machine, &options)
    }
}

/// The image to burn. The board always keeps settings, so a firmware with no settings layout is
/// refused here, before the board is touched, and not on every start after.
fn load_for_the_board(file: &str) -> Result<Firmware, RunError> {
    let firmware = with_layout(
        Firmware::load(file)?,
        BOARD_LAYOUT_VARIABLE,
        layout_named_in(BOARD_LAYOUT_VARIABLE),
    )?;
    match firmware.profile() {
        Some(_) => Ok(firmware),
        None => Err(RunError::NoLayout {
            path: file.to_string(),
            source: NoLayout,
        }),
    }
}

/// Switches the board on, burning the `--burn` image onto it first if there is one, and runs it
/// with its own settings and card unless the options name others.
fn on_the_board(mut options: RunOptions) -> Result<bool, RunError> {
    let dir = board_dir(|name| std::env::var_os(name))?;
    // The image is loaded (and its vector table checked by the loader) before the board is
    // touched, so that one that is refused does not even make the directory. Nothing in an
    // image says whether it is whole, so that is not known here: the size is printed below.
    let image = options
        .burn
        .take()
        .map(|file| load_for_the_board(&file))
        .transpose()?;
    let board = Board::switch_on(dir, image.is_some())?;
    if let Some(firmware) = &image {
        board.burn(firmware)?;
        let vectors = firmware.vector_table();
        say(format!(
            "burned {} bytes (initial SP {:#010X}, reset handler {:#010X})",
            firmware.image().len(),
            vectors.initial_sp,
            vectors.reset_handler
        ));
    }
    if options.flash.is_none() {
        options.flash = Some(board.settings_path());
    }
    let card = if options.no_sd {
        board.forget_card()?;
        Card::Empty
    } else if let Some(path) = options.sd.clone() {
        Card::Given(path)
    } else if let Some(path) = board.remembered_card()? {
        options.sd = Some(path);
        Card::Remembered
    } else {
        Card::Empty
    };
    let firmware = board.firmware_path();
    let machine =
        build_machine(&firmware, &options, BOARD_LAYOUT_VARIABLE, true).map_err(|err| {
            match (err, &card) {
                // The settings file is not at fault, the firmware is.
                (RunError::NoLayout { .. }, _) => RunError::BoardNoLayout { path: firmware },
                (err, Card::Remembered) => about_the_remembered_card(err),
                (err, _) => err,
            }
        })?;
    if let Card::Given(path) = &card {
        board.remember_card(path)?;
    }
    let result = go(machine, options);
    drop(board);
    result
}
