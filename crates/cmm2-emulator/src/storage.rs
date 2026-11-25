//! The `--flash` and `--sd` files: read into the machine at the start and written back at the end.

use crate::args::RunOptions;
use crate::atomic_write::{remove_stale_temps, replace_atomically, write_atomically};
use crate::error::{RunError, say};
use crate::saveable::{access, check_regular, why_not_saveable};
use cmm2_core::{
    Cpu, FileKind, MAGIC, MIN_IMAGE_SIZE, NoLayout, PersistentError, PersistentSizeError, Profile,
    SdSlot, SettingsError, classify, decode_v2,
};
use std::io;
use std::path::Path;

/// Loads the saved flash and clock from the `--flash` file. A file that is not there yet is the first run:
/// the flash stays as the firmware leaves it. The options have to be saved at the end, so a
/// file that could not be is refused now and not after a long run. `labelled_by` is the variable
/// that named a layout for a firmware I cannot recognise, given only for the board, whose only
/// reason to be reading the raw layout is that variable.
pub(crate) fn load_flash(
    cpu: &mut Cpu,
    path: &str,
    labelled_by: Option<&str>,
) -> Result<(), RunError> {
    let flash_error = |source| RunError::Flash {
        path: path.to_string(),
        source,
    };
    // Before anything else, so that a first run, with no file yet, is refused too: it could not
    // save at the end.
    if cpu.profile().is_none() {
        return Err(RunError::NoLayout {
            path: path.to_string(),
            source: NoLayout,
        });
    }
    check_regular(path).map_err(flash_error)?;
    if let Some(why) = why_not_saveable(path) {
        return Err(flash_error(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{why}, so the run could not save its flash and clock"),
        )));
    }
    remove_stale_temps(path);
    // A file is measured before it is read, because the bound is the format's and not the
    // file's: nothing larger than the longest file either format can be could be a settings
    // file at all, and reading one whole is what a stranger should not cost. What is wrong with
    // a file that cannot be measured is left for the read to say, as it always was.
    let largest = Profile::largest_settings_file() as u64;
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > largest => {
            return Err(settings_error(
                path,
                SettingsError::Corrupt(format!(
                    "it is {} bytes, more than the {largest} bytes any settings file of the \
                     firmwares I know can be",
                    meta.len()
                )),
            ));
        }
        // Everything else, a file that cannot be measured with it, is for the read to say.
        _ => {}
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(flash_error(source)),
    };
    if cpu.settings_v2() {
        return load_v2(cpu, path, &bytes);
    }
    // The raw layout, read as always. One thing is new: a file that starts with the v2 magic
    // is a board's own file, and raw bytes are exactly what it is not. Nothing of it is
    // read, and nothing is written; the message names the two ways round it.
    if bytes.starts_with(&MAGIC) {
        return Err(RunError::V2InRawRun {
            path: path.to_string(),
            labelled_by: labelled_by.map(str::to_string),
        });
    }
    cpu.load_persistent(&bytes)
        .map_err(|source| RunError::FlashSize {
            path: path.to_string(),
            source,
        })
}

/// The v2 load order, and the only order it is read in, so that "corrupt" fires only once
/// every legitimate kind of file has been ruled out (docs/settings-format.md, rule 7 of the
/// reading rules).
fn load_v2(cpu: &mut Cpu, path: &str, bytes: &[u8]) -> Result<(), RunError> {
    let profile = cpu.profile().expect("load_flash checked for a profile");
    match classify(bytes, profile) {
        Ok(FileKind::V2) => {
            let settings = decode_v2(bytes, cpu.image(), profile)
                .map_err(|source| settings_error(path, source))?;
            cpu.load_settings(&settings)
                .map_err(|source| RunError::FlashSize {
                    path: path.to_string(),
                    source: source.into(),
                })
        }
        Ok(FileKind::V1) => {
            // A v1 file of this profile: applied by today's reader unchanged, then migrated
            // to v2 (the machine first, the files after, in `migrate`).
            cpu.load_persistent(bytes)
                .map_err(|source| RunError::FlashSize {
                    path: path.to_string(),
                    source,
                })?;
            migrate(cpu, path, bytes)
        }
        Ok(FileKind::OtherRelease(_)) => Err(RunError::FlashSize {
            path: path.to_string(),
            // The shipped message, from the same error the raw reader makes for this file:
            // it names the other release and never says corrupt.
            source: PersistentError::Size(PersistentSizeError {
                got: bytes.len(),
                profile,
            }),
        }),
        Err(source) => Err(settings_error(path, source)),
    }
}

/// A migration's or a refusal's error, naming the copy of the old settings that can be restored:
/// the clause is only true, and only worth saying, when that file is there. It is named as it is
/// named, which is the `--flash` path with `.v1` on the end, whatever that path was called.
fn settings_error(path: &str, source: SettingsError) -> RunError {
    let backup = backup_path(path);
    let worth_naming = matches!(source, SettingsError::Corrupt(_)) && backup.exists();
    RunError::Settings {
        path: path.to_string(),
        source: Box::new(source),
        backup: worth_naming.then(|| backup.to_string_lossy().into_owned()),
    }
}

/// The file a migrated v1 is kept in: beside the live one, under a name that says so.
fn backup_path(path: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{path}.v1"))
}

/// Replaces the v1 file with its v2 form, in the order that can never destroy the last
/// good copy (the plan's M2): every check first, then the complete-or-absent backup, then
/// the live file. A crash between any two leaves the old file or the new one, never half
/// of either; a run that restarts before step 3 just takes step 2's skipped path.
fn migrate(cpu: &mut Cpu, path: &str, original: &[u8]) -> Result<(), RunError> {
    // Step 1: encode from the machine that already has the file applied, and read the
    // bytes back, all before any file is touched.
    let v2 = cpu.save_settings().map_err(|source| RunError::NoLayout {
        path: path.to_string(),
        source,
    })?;
    let profile = cpu.profile().expect("load_flash checked for a profile");
    decode_v2(&v2, cpu.image(), profile).map_err(|source| settings_error(path, source))?;
    // Step 2: the backup, replaced rather than followed as the live file is, so that it lands
    // beside the live file and not wherever a link points. If a plain file is already there, it
    // came from an earlier migration of this board's history: leave it alone, it is the older
    // copy. Anything else at that name - a link, a directory, a device - is refused rather than
    // followed or replaced, because it is not this board's to write through.
    let backup = backup_path(path);
    let backup = backup.to_string_lossy().into_owned();
    let an_older_backup_is_there = match std::fs::symlink_metadata(&backup) {
        Ok(held) if held.file_type().is_file() => true,
        Ok(_) => {
            return Err(RunError::BackupNotAFile {
                path: path.to_string(),
                backup,
            });
        }
        // Nothing at the name: the first migration of this file, which is the only case that
        // writes one.
        Err(source) if source.kind() == io::ErrorKind::NotFound => false,
        Err(source) => {
            return Err(RunError::Flash {
                path: backup,
                source,
            });
        }
    };
    if !an_older_backup_is_there {
        replace_atomically(&backup, original).map_err(|source| RunError::Flash {
            path: backup.clone(),
            source,
        })?;
    }
    // Step 3: replace the live file, as the board's own files are replaced: a symlink at
    // that name is the board's, and is replaced rather than followed.
    replace_atomically(path, &v2).map_err(|source| RunError::Flash {
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
    if cpu.settings_v2() {
        // The board's own file, written as its own file is written: replaced, not followed.
        let bytes = cpu.save_settings().map_err(|source| RunError::NoLayout {
            path: path.clone(),
            source,
        })?;
        return replace_atomically(path, &bytes).map_err(|source| RunError::Flash {
            path: path.clone(),
            source,
        });
    }
    let bytes = cpu.persistent().map_err(|source| RunError::NoLayout {
        path: path.clone(),
        source,
    })?;
    write_atomically(path, &bytes).map_err(|source| RunError::Flash {
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
