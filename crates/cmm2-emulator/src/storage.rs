//! The `--flash` and `--sd` files: read into the machine at the start and written back at the end.

use crate::args::RunOptions;
use crate::atomic_write::{remove_stale_temps, write_atomically};
use crate::error::{RunError, say};
use crate::saveable::{access, check_regular, why_not_saveable};
use cmm2_core::{Cpu, MIN_IMAGE_SIZE, SdSlot};
use std::io;
use std::path::Path;

/// Loads the saved flash and clock from the `--flash` file. A file that is not there yet is the first run:
/// the flash stays as the firmware leaves it. The options have to be saved at the end, so a
/// file that could not be is refused now and not after a long run.
pub(crate) fn load_flash(cpu: &mut Cpu, path: &str) -> Result<(), RunError> {
    let flash_error = |source| RunError::Flash {
        path: path.to_string(),
        source,
    };
    check_regular(path).map_err(flash_error)?;
    if let Some(why) = why_not_saveable(path) {
        return Err(flash_error(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{why}, so the run could not save its flash and clock"),
        )));
    }
    remove_stale_temps(path);
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(flash_error(source)),
    };
    cpu.load_persistent(&bytes)
        .map_err(|source| RunError::FlashSize {
            path: path.to_string(),
            source,
        })
}

/// Why the card of the `--sd` file is write-protected, if it is: the file cannot be written
/// back (as for `--flash`), or it is read-only, which is the same switch on a real card.
fn why_write_protected(path: &str) -> Option<String> {
    why_not_saveable(path).or_else(|| {
        access(Path::new(path), libc::W_OK)
            .is_err()
            .then(|| "the file is read-only".to_string())
    })
}

/// Puts the card image from the `--sd` file in the socket. The file has to exist: a card is
/// never made out of nothing. A card that could not be written back is one with its
/// write-protect switch on: the firmware refuses to write to it, and it is never written back.
pub(crate) fn load_sd(sd: &SdSlot, path: &str) -> Result<(), RunError> {
    let sd_error = |source| RunError::Sd {
        path: path.to_string(),
        source,
    };
    check_regular(path).map_err(sd_error)?;
    let image = std::fs::read(path).map_err(sd_error)?;
    let size = image.len();
    sd.insert(image).map_err(|source| RunError::SdSize {
        path: path.to_string(),
        source,
    })?;
    if size < MIN_IMAGE_SIZE {
        return Err(RunError::SdTooSmall {
            path: path.to_string(),
            size,
        });
    }
    remove_stale_temps(path);
    if let Some(why) = why_write_protected(path) {
        sd.set_write_protected(true);
        say(format!("{path}: the card is write-protected because {why}"));
    }
    Ok(())
}

/// Writes the flash and clock to the `--flash` file, if it was given.
fn save_flash(cpu: &mut Cpu, options: &RunOptions) -> Result<(), RunError> {
    let Some(path) = &options.flash else {
        return Ok(());
    };
    write_atomically(path, &cpu.persistent()).map_err(|source| RunError::Flash {
        path: path.clone(),
        source,
    })
}

/// Writes the card in the socket back to the `--sd` file, if it was given and the guest changed
/// the card. A card that was only read leaves the file alone, so whatever else touched it
/// meanwhile is not overwritten.
fn save_sd(sd: &SdSlot, options: &RunOptions) -> Result<(), RunError> {
    let Some(path) = &options.sd else {
        return Ok(());
    };
    // A card with its write-protect switch on is never written back, whatever the guest did.
    if !sd.modified() || sd.write_protected() {
        return Ok(());
    }
    let Some(image) = sd.eject() else {
        return Ok(());
    };
    write_atomically(path, &image).map_err(|source| RunError::Sd {
        path: path.clone(),
        source,
    })
}

/// Writes back everything the run changed that a file keeps. Both are tried, and the first
/// error is the one returned.
pub(crate) fn save_files(cpu: &mut Cpu, sd: &SdSlot, options: &RunOptions) -> Result<(), RunError> {
    let flash = save_flash(cpu, options);
    flash.and(save_sd(sd, options))
}

#[cfg(test)]
mod tests;
