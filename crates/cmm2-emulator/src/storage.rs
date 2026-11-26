//! The `--flash` and `--sd` files: read into the machine at the start and written back at the end.

use crate::args::RunOptions;
use crate::atomic_write::{
    create_new_atomically, remove_stale_temps, remove_stale_temps_beside, replace_atomically,
    write_atomically,
};
use crate::error::{RunError, say, shown};
use crate::saveable::{access, check_regular, why_not_replaceable, why_not_saveable};
use cmm2_core::{
    Cpu, FileKind, MAGIC, MIN_IMAGE_SIZE, NoLayout, PersistentError, PersistentSizeError, Profile,
    SdSlot, SettingsError, classify, decode_v2,
};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// Loads the saved flash and clock from the `--flash` file. A file that is not there yet is the
/// first run: the flash stays as the firmware leaves it. The options have to be saved at the end,
/// so a file that could not be is refused now and not after a long run.
pub(crate) fn load_flash(cpu: &mut Cpu, path: &str) -> Result<(), RunError> {
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
    // The v2 save replaces the name it is given and does not follow a link there, so the board's
    // file is judged, and its stale temporary files cleared, in the directory of that name; the
    // raw save writes through a link, and uses the directory the link leads to.
    let v2 = cpu.settings_v2();
    if let Some(why) = if v2 {
        why_not_replaceable(path)
    } else {
        why_not_saveable(path)
    } {
        return Err(flash_error(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{why}, so the run could not save its flash and clock"),
        )));
    }
    if v2 {
        remove_stale_temps_beside(path);
    } else {
        remove_stale_temps(path);
    }
    // A file is measured before it is read, because the bound is the format's and not the
    // file's: nothing larger than the longest file either format can be is a settings file, and
    // a stranger's huge file should cost a `stat`. A file that cannot be measured is left for
    // the read to say.
    let largest = Profile::largest_settings_file() as u64;
    if let Ok(meta) = std::fs::metadata(path)
        && meta.len() > largest
    {
        return Err(settings_error(
            path,
            SettingsError::Corrupt(format!(
                "it is {} bytes, more than the {largest} bytes any settings file of the \
                 firmwares I know can be",
                meta.len()
            )),
        ));
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(flash_error(source)),
    };
    if v2 {
        return load_v2(cpu, path, &bytes);
    }
    // The raw layout, read as always, except that a file with the v2 magic is not raw: nothing
    // of it is read or written, and the message names the ways round it.
    if bytes.starts_with(&MAGIC) {
        return Err(RunError::V2InRawRun {
            path: path.to_string(),
        });
    }
    cpu.load_persistent(&bytes)
        .map_err(|source| RunError::FlashSize {
            path: path.to_string(),
            source,
        })
}

/// The v2 load order, and the only order it is read in, so that "corrupt" fires only once
/// every legitimate kind of file has been ruled out (docs/settings-format.md, reading rule 7).
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
            // A v1 file of this profile: applied by the raw reader, then migrated to v2.
            cpu.load_persistent(bytes)
                .map_err(|source| RunError::FlashSize {
                    path: path.to_string(),
                    source,
                })?;
            migrate(cpu, path, bytes)
        }
        Ok(FileKind::OtherRelease(_)) => Err(RunError::FlashSize {
            path: path.to_string(),
            // The raw reader's own message for this file: it names the other release and
            // never says corrupt.
            source: PersistentError::Size(PersistentSizeError {
                got: bytes.len(),
                profile,
            }),
        }),
        Err(source) => Err(settings_error(path, source)),
    }
}

/// A refusal's error. A corrupt file also lists the copies of older settings that sit beside it,
/// if there are any, as ways back: they are named as they are, the `--flash` path with `.v1` or
/// `.v1.2` and so on on the end, whatever that path was called.
fn settings_error(path: &str, source: SettingsError) -> RunError {
    let backups = if matches!(source, SettingsError::Corrupt(_)) {
        (1..=MAX_BACKUPS)
            .map(|n| backup_name(path, n))
            .filter(|name| std::fs::symlink_metadata(name).is_ok_and(|m| m.file_type().is_file()))
            .collect()
    } else {
        Vec::new()
    };
    RunError::Settings {
        path: path.to_string(),
        source: Box::new(source),
        backups,
    }
}

/// The most copies of old settings that are kept beside one file: `.v1`, then `.v1.2` to `.v1.99`.
const MAX_BACKUPS: usize = 99;

/// The name of the `n`th copy of the settings that a migration replaced.
fn backup_name(path: &str, n: usize) -> String {
    match n {
        1 => format!("{path}.v1"),
        n => format!("{path}.v1.{n}"),
    }
}

/// A whole file, by its name.
fn read_file(path: &str) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}

/// Where the raw bytes of a file being migrated are kept.
enum Backup {
    /// A plain file with exactly these bytes is already there: a migration that was cut short
    /// after its backup made it, so it is used and nothing is written.
    Kept,
    /// Nothing holds this name.
    Free(String),
}

/// The backup for `original`: a copy of it that is already there, or else the first name that
/// nothing holds. A plain file with other bytes is an older copy, and is left alone. A plain file
/// whose bytes cannot be read is left alone too, and the name the copy goes to is said, because a
/// silent step past it would leave the settings not where their owner expects them. Anything else
/// at a name - a link, a directory, a device - is refused, and so is a name that cannot be looked
/// at: it is not this board's to write through or over.
fn find_backup(
    path: &str,
    original: &[u8],
    read: &impl Fn(&str) -> io::Result<Vec<u8>>,
) -> Result<Backup, RunError> {
    let mut unreadable: Vec<String> = Vec::new();
    for n in 1..=MAX_BACKUPS {
        let backup = backup_name(path, n);
        match std::fs::symlink_metadata(&backup) {
            // The length is compared first, so a file is read only if it could be the same, and
            // that is never more than the file being migrated (itself at most the largest size).
            Ok(held) if held.file_type().is_file() => {
                if held.len() == original.len() as u64 {
                    match read(&backup) {
                        Ok(bytes) if bytes == original => {
                            say_skipped(&unreadable, &backup);
                            return Ok(Backup::Kept);
                        }
                        Ok(_) => {}
                        Err(_) => unreadable.push(backup),
                    }
                }
            }
            Ok(_) => {
                return Err(RunError::BackupNotAFile {
                    path: path.to_string(),
                    backup,
                });
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                say_skipped(&unreadable, &backup);
                return Ok(Backup::Free(backup));
            }
            Err(source) => {
                return Err(RunError::Flash {
                    path: backup,
                    source,
                });
            }
        }
    }
    Err(RunError::BackupsFull {
        path: path.to_string(),
        first: backup_name(path, 1),
        last: backup_name(path, MAX_BACKUPS),
    })
}

/// Says that every name in `unreadable` was there and could not be read, so the copy of the old
/// settings goes to `backup` instead. The two are the user's to know: the first is a file that was
/// left exactly as it was, and the second is where their settings are now.
fn say_skipped(unreadable: &[String], backup: &str) {
    if unreadable.is_empty() {
        return;
    }
    let which: Vec<String> = unreadable
        .iter()
        .map(|name| shown(name).into_owned())
        .collect();
    let were = if unreadable.len() == 1 {
        "copy"
    } else {
        "copies"
    };
    say(format!(
        "the {were} at {} cannot be read; the backup goes to {}",
        which.join(", "),
        shown(backup)
    ));
}

/// Makes sure the raw bytes are kept beside the live file, so that none are ever lost. A name
/// that is taken between looking and writing is not replaced: the names are looked at again. Losing
/// that race every time is refused as a race, not as a cascade with every name in it: nothing that
/// was written by this board need be there at all.
fn keep_backup(
    path: &str,
    original: &[u8],
    mut write_backup: impl FnMut(&str, &[u8]) -> io::Result<()>,
    read_backup: impl Fn(&str) -> io::Result<Vec<u8>>,
) -> Result<(), RunError> {
    for _ in 0..MAX_BACKUPS {
        let backup = match find_backup(path, original, &read_backup)? {
            Backup::Kept => return Ok(()),
            Backup::Free(backup) => backup,
        };
        match write_backup(&backup, original) {
            Ok(()) => return Ok(()),
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {}
            Err(source) if source.kind() == io::ErrorKind::Unsupported => {
                return Err(RunError::BackupNeedsLink {
                    path: path.to_string(),
                    backup,
                    source,
                });
            }
            Err(source) => {
                return Err(RunError::Flash {
                    path: backup,
                    source,
                });
            }
        }
    }
    Err(RunError::BackupRaceLost {
        path: path.to_string(),
        tries: MAX_BACKUPS,
    })
}

/// Replaces the v1 file with its v2 form, in the order that can never destroy the last good
/// copy: every check first, then the backup, then the live file. A crash between two steps leaves
/// the old file or the new one, never half of either.
fn migrate(cpu: &mut Cpu, path: &str, original: &[u8]) -> Result<(), RunError> {
    // The copy is as private as the file it copies: the mode of the file as it was read, which
    // the live file is about to stop being.
    let mode = std::fs::metadata(path).map_or(0o600, |meta| meta.mode() & 0o777);
    migrate_with(
        cpu,
        path,
        original,
        Cpu::save_settings,
        |name, bytes| create_new_atomically(name, bytes, mode),
        read_file,
        replace_atomically,
    )
}

/// `migrate`, with the encoder and the two writers given, so that a test can hand it an encoder
/// that writes a bad file, or a write that fails. `write_backup` must never replace a name that
/// is taken, and `read_backup` reads a copy that is already there; `write_live` replaces the live
/// file.
fn migrate_with(
    cpu: &mut Cpu,
    path: &str,
    original: &[u8],
    encode: impl FnOnce(&mut Cpu) -> Result<Vec<u8>, NoLayout>,
    write_backup: impl FnMut(&str, &[u8]) -> io::Result<()>,
    read_backup: impl Fn(&str) -> io::Result<Vec<u8>>,
    write_live: impl FnOnce(&str, &[u8]) -> io::Result<()>,
) -> Result<(), RunError> {
    // Step 1: encode from the machine that already has the file applied, and read the bytes
    // back, before any file is touched.
    let v2 = encode(cpu).map_err(|source| RunError::NoLayout {
        path: path.to_string(),
        source,
    })?;
    let profile = cpu.profile().expect("load_flash checked for a profile");
    decode_v2(&v2, cpu.image(), profile).map_err(|source| RunError::NotReadBack {
        path: path.to_string(),
        reason: match source {
            SettingsError::Corrupt(reason) => reason,
            other => other.to_string(),
        },
    })?;
    // Step 2: the raw bytes are kept first, in a copy that is made whole and never replaces a
    // name, and never lands where a link points.
    keep_backup(path, original, write_backup, read_backup)?;
    // Step 3: only now the live file, as the board's own files are replaced: a symlink at that
    // name is the board's, and is replaced rather than followed.
    write_live(path, &v2).map_err(|source| RunError::Flash {
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
        // The name of the card is the user's, and the reason is `saveable`'s words, which name a
        // path when the directory is what stopped the save. Both are escaped at the line they are
        // said on (see `shown`).
        say(format!(
            "{}: the card is write-protected because {}",
            shown(path),
            shown(&why)
        ));
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
