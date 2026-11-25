//! The settings file format v2 (issue 82): a header that names the firmware the layout
//! belongs to, one entry per 32-byte flash row that differs from the burned image, the
//! clock's tail, and a CRC-32. The contract is docs/settings-format.md.

use crate::Profile;
use crate::ds3231::STATE_SIZE;
use crate::firmware::FLASH_BASE;
use crate::profile::CLOCK_SIZE;
use sha2::{Digest as _, Sha256};
use std::fmt;

/// The first four bytes of every v2-or-later file. A raw region dump that starts with these
/// is refused, never guessed at (docs/settings-format.md).
pub const MAGIC: [u8; 4] = *b"CMM2";

/// The format version this emulator writes, and the only one it reads.
pub const VERSION: u8 = 2;

/// The fixed header: magic, version, flags, reserved, image length, hash, entry count.
pub(crate) const HEADER_SIZE: usize = 48;

/// The STM32H743 programming row: the unit a diff is taken in.
pub(crate) const ROW: usize = 32;

/// The CRC-32 that ends the file.
pub(crate) const CRC_SIZE: usize = 4;

/// An entry's own bytes: the address it starts at and the length of its rows, before the rows.
pub(crate) const ENTRY_SIZE: usize = 8;

/// What a settings file turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// A v2 file for this emulator to read.
    V2,
    /// A raw dump this profile wrote before the format existed: read it, then migrate it.
    V1,
    /// A raw dump of the other release's size: say so, and touch nothing.
    OtherRelease(&'static Profile),
}

/// What a v2 file holds: the flash rows that differ from the image, the clock's time and
/// date, and the DS3231's state when it is not as it was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// (address, bytes), sorted by address, whole rows inside the profile's regions.
    pub entries: Vec<(u32, Vec<u8>)>,
    /// The clock's TR and DR.
    pub clock: [u32; 2],
    pub ds3231: Option<[u8; STATE_SIZE]>,
}

/// The firmware a settings file names, as its header reads: the length of the image it was
/// made against, its SHA-256, and the name only where both together are one I know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    pub name: Option<&'static str>,
    pub len: u32,
    pub sha256: [u8; 32],
}

/// Why a settings file was refused. `Cpu` and the front end turn these into the user's
/// message; the path comes from whoever has the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsError {
    /// A version byte above `VERSION`: never guessed at.
    Newer { version: u32 },
    /// Flags or reserved bits set: only a newer emulator may mean them.
    ReservedBits { version: u32 },
    /// The header names a different image than the one running: the hash gate.
    Mismatch { saved: ImageRef, running: ImageRef },
    /// A file that is none of the legitimate things a settings file can be.
    Corrupt(String),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::Newer { version } => write!(
                f,
                "settings format version {version} is newer than this emulator's \
                 {VERSION}. Update the emulator."
            ),
            SettingsError::ReservedBits { version } => write!(
                f,
                "settings format version {version} uses flags or reserved bits this emulator \
                 does not know. Update the emulator."
            ),
            SettingsError::Mismatch { saved, running } => write!(
                f,
                "these settings were saved for a different firmware ({}; this run's is {}). \
                 Burn the firmware they were made for, or start fresh by deleting them.",
                saved.describe(),
                running.describe()
            ),
            SettingsError::Corrupt(reason) => write!(
                f,
                "the settings file is corrupt ({reason}); nothing was loaded."
            ),
        }
    }
}

impl std::error::Error for SettingsError {}

impl ImageRef {
    fn describe(&self) -> String {
        let name = self.name.unwrap_or("unknown firmware");
        let hash: String = self.sha256[..6]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        format!("{name}, image {} bytes, sha256 {hash}", self.len)
    }
}

/// The SHA-256 of an image, as the header stores it.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// The IEEE CRC-32 (the reflected one, as gzip and zip use it).
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let low = crc & 1;
            crc >>= 1;
            if low == 1 {
                crc ^= 0xEDB8_8320;
            }
        }
    }
    !crc
}

/// What a file is, before any of it is read into a machine: the magic decides everything.
/// Without it, the size alone decides v1 (this profile's `regions_for`), the other release,
/// or corrupt — so "Legacy" can never widen into "any non-magic bytes".
pub fn classify(bytes: &[u8], profile: &'static Profile) -> Result<FileKind, SettingsError> {
    if bytes.starts_with(&MAGIC) {
        // A version and header check, then the caller reads it whole with `decode_v2`.
        header_kind(bytes)
    } else if profile.regions_for(bytes.len()).is_some() {
        Ok(FileKind::V1)
    } else if let Some(other) = profile.other_with_size(bytes.len()) {
        Ok(FileKind::OtherRelease(other))
    } else {
        Err(SettingsError::Corrupt(format!(
            "{} bytes is not a size this firmware's settings come in, and it does not \
             start with the v2 magic",
            bytes.len()
        )))
    }
}

/// The header-only part of `classify`, for a file that starts with the magic.
fn header_kind(bytes: &[u8]) -> Result<FileKind, SettingsError> {
    if bytes.len() < HEADER_SIZE {
        return Err(SettingsError::Corrupt(
            "the file ends inside the header".to_string(),
        ));
    }
    let version = u32::from(bytes[4]);
    if version > u32::from(VERSION) {
        return Err(SettingsError::Newer { version });
    }
    if version < u32::from(VERSION) {
        return Err(SettingsError::Corrupt(format!(
            "version {version} is not readable by this emulator"
        )));
    }
    if bytes[5] != 0 || bytes[6..8] != [0, 0] {
        return Err(SettingsError::ReservedBits { version });
    }
    Ok(FileKind::V2)
}

/// Reads a v2 file whole: header, hash gate, entries, tail and CRC. Nothing is applied
/// before every check has passed, so a refusal never leaves a half-loaded machine.
pub fn decode_v2(
    bytes: &[u8],
    image: &[u8],
    profile: &'static Profile,
) -> Result<Settings, SettingsError> {
    if !bytes.starts_with(&MAGIC) {
        return Err(SettingsError::Corrupt(
            "the file does not start with the v2 magic".to_string(),
        ));
    }
    header_kind(bytes)?;
    let mut saved_hash = [0u8; 32];
    saved_hash.copy_from_slice(&bytes[12..44]);
    let saved_len = u32::from_le_bytes(bytes[8..12].try_into().expect("four bytes"));
    let running_hash = sha256(image);
    let running_len = image.len() as u32;
    if saved_len != running_len || saved_hash != running_hash {
        // The hash gate, and only here (issue 82): nothing else of the file is trusted
        // enough to be worth reading, and nothing of it is applied.
        return Err(SettingsError::Mismatch {
            saved: ImageRef {
                name: Profile::named_release(saved_len, &saved_hash),
                len: saved_len,
                sha256: saved_hash,
            },
            running: ImageRef {
                name: Some(profile.name),
                len: running_len,
                sha256: running_hash,
            },
        });
    }
    let count = u32::from_le_bytes(bytes[44..48].try_into().expect("four bytes"));
    let rows = (profile.flash_size() / ROW) as u32;
    if count > rows {
        // Checked before anything is reserved: a four-byte lie cannot ask for gigabytes.
        return Err(SettingsError::Corrupt(format!(
            "the entry count {count} is more than the {rows} rows of the firmware's regions"
        )));
    }
    if count > 0
        // The table's own bytes, before any of them is read as an entry: a file that promises
        // `count` of them owes an address and a length for each, and still has the tail's clock
        // and the checksum to come. Without this the reader takes the clock for the first entry,
        // and refuses an address the file never claimed. `count` is bounded just above, so the
        // sum cannot wrap.
        && bytes.len() < HEADER_SIZE + count as usize * ENTRY_SIZE + CLOCK_SIZE + CRC_SIZE
    {
        return Err(SettingsError::Corrupt(
            "the file ends inside the entry table".to_string(),
        ));
    }
    let mut entries: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut at = HEADER_SIZE;
    let mut previous_end: Option<u32> = None;
    for _ in 0..count {
        if at + ENTRY_SIZE > bytes.len() {
            return Err(SettingsError::Corrupt(
                "the file ends inside the entry table".to_string(),
            ));
        }
        let addr = u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"));
        let len = u32::from_le_bytes(
            bytes[at + 4..at + ENTRY_SIZE]
                .try_into()
                .expect("four bytes"),
        );
        if len < ROW as u32 || len % ROW as u32 != 0 {
            return Err(SettingsError::Corrupt(format!(
                "the entry at {addr:#010X} holds {len} bytes, not whole rows of {ROW}"
            )));
        }
        if addr % ROW as u32 != 0 {
            return Err(SettingsError::Corrupt(format!(
                "the entry at {addr:#010X} does not start on a row"
            )));
        }
        let end = match addr.checked_add(len) {
            Some(end) => end,
            None => return Err(SettingsError::Corrupt("entry bounds overflow".to_string())),
        };
        if !profile
            .regions
            .iter()
            .any(|region| addr >= region.addr && end <= region.addr + region.len)
        {
            return Err(SettingsError::Corrupt(format!(
                "the entry at {addr:#010X} is not whole inside one region of the firmware"
            )));
        }
        if previous_end.is_some_and(|previous| addr < previous) {
            return Err(SettingsError::Corrupt(format!(
                "the entry at {addr:#010X} overlaps the one before it, or is out of order"
            )));
        }
        let value_end = at + 8 + len as usize;
        if value_end > bytes.len() {
            return Err(SettingsError::Corrupt(format!(
                "the file ends inside the entry at {addr:#010X}"
            )));
        }
        entries.push((addr, bytes[at + 8..value_end].to_vec()));
        at = value_end;
        previous_end = Some(end);
    }
    if bytes.len() < at + CRC_SIZE {
        return Err(SettingsError::Corrupt(
            "the file ends before the checksum".to_string(),
        ));
    }
    let crc_at = bytes.len() - CRC_SIZE;
    let tail = &bytes[at..crc_at];
    let ds3231 = if tail.len() == CLOCK_SIZE {
        None
    } else if tail.len() == CLOCK_SIZE + STATE_SIZE {
        Some(
            tail[CLOCK_SIZE..]
                .try_into()
                .expect("the DS3231 state's size"),
        )
    } else {
        return Err(SettingsError::Corrupt(format!(
            "the tail is {} bytes, not the clock's {CLOCK_SIZE} or those plus the DS3231's \
             {STATE_SIZE}",
            tail.len()
        )));
    };
    let want = u32::from_le_bytes(bytes[crc_at..].try_into().expect("four bytes"));
    if crc32(&bytes[..crc_at]) != want {
        return Err(SettingsError::Corrupt(
            "the checksum does not match the file".to_string(),
        ));
    }
    let mut clock = [0u32; 2];
    for (word, at) in clock.iter_mut().zip(0..) {
        *word = u32::from_le_bytes(tail[at * 4..at * 4 + 4].try_into().expect("four bytes"));
    }
    Ok(Settings {
        entries,
        clock,
        ds3231,
    })
}

/// Writes a v2 file: the header naming `image` (whose SHA-256 the caller has), the entries
/// as given, today's tail, and the CRC-32 of it all. `Cpu::save_settings` hands it sorted,
/// merged rows inside the profile's regions; the tests hand it wrong things on purpose,
/// which is `decode_v2`'s business to refuse.
pub fn encode(image: &[u8], image_hash: &[u8; 32], settings: &Settings) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER_SIZE + 12);
    bytes.extend(MAGIC);
    bytes.push(VERSION);
    bytes.push(0); // flags: a reader refuses non-zero, so a writer writes zero.
    bytes.extend([0u8; 2]); // reserved, the same.
    bytes.extend((image.len() as u32).to_le_bytes());
    bytes.extend(image_hash);
    bytes.extend((settings.entries.len() as u32).to_le_bytes());
    for (addr, value) in &settings.entries {
        bytes.extend(addr.to_le_bytes());
        bytes.extend((value.len() as u32).to_le_bytes());
        bytes.extend(value);
    }
    for word in settings.clock {
        bytes.extend(word.to_le_bytes());
    }
    if let Some(state) = settings.ds3231 {
        bytes.extend(state);
    }
    let crc = crc32(&bytes);
    bytes.extend(crc.to_le_bytes());
    bytes
}

/// The baseline byte rule as a word: the image's own bytes inside the image, FF past its
/// end, which is what an erased flash reads (docs/settings-format.md).
pub(crate) fn baseline_word(image: &[u8], addr: u32) -> u32 {
    let at = (addr - FLASH_BASE) as usize;
    let mut word = [0xFFu8; 4];
    for (byte, at) in word.iter_mut().zip(at..) {
        if let Some(&image_byte) = image.get(at) {
            *byte = image_byte;
        }
    }
    u32::from_le_bytes(word)
}

#[cfg(test)]
mod tests;
