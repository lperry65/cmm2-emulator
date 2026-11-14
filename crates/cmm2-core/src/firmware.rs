use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const FLASH_BASE: u32 = 0x0800_0000;
/// The STM32H743 has 2 MB of flash.
pub const FLASH_SIZE: usize = 2 * 1024 * 1024;
/// Where the flash the user changes starts: the stored BASIC program from here, then the saved
/// options in the last sector, at 0x081E_0000. Below it is the firmware.
///
/// I only know this address for MMBasic 7.00.00b5, which leaves 216 bytes between the end of
/// its image and here. I have not checked it against any other release. A firmware image that
/// reaches it is refused, so a release that does not fit here fails loudly and does not have
/// its tail merged into the `--flash` file.
pub const USER_FLASH_BASE: u32 = 0x0812_0000;
/// The most a firmware image can be: what lies below the user flash.
pub const FIRMWARE_MAX_SIZE: usize = (USER_FLASH_BASE - FLASH_BASE) as usize;
/// The user flash runs to the end of the flash.
pub const USER_FLASH_SIZE: usize = FLASH_SIZE - (USER_FLASH_BASE - FLASH_BASE) as usize;

// On-chip RAM sits between the flash and peripheral windows.
const RAM_START: u32 = 0x2000_0000;
const RAM_END: u32 = 0x4000_0000;

/// The first two words of the image, which the core reads on reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VectorTable {
    pub initial_sp: u32,
    pub reset_handler: u32,
}

#[derive(Debug)]
pub struct Firmware {
    image: Vec<u8>,
    vectors: VectorTable,
}

#[derive(Debug)]
pub enum FirmwareError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    TooShort(usize),
    TooLarge,
    StackPointerNotInRam(u32),
    StackPointerUnaligned(u32),
    ResetNotThumb(u32),
    ResetOutsideImage(u32),
}

impl Firmware {
    pub fn from_bytes(image: Vec<u8>) -> Result<Firmware, FirmwareError> {
        if image.len() < 8 {
            return Err(FirmwareError::TooShort(image.len()));
        }
        if image.len() > FIRMWARE_MAX_SIZE {
            return Err(FirmwareError::TooLarge);
        }
        let word = |at: usize| u32::from_le_bytes(image[at..at + 4].try_into().unwrap());
        let vectors = VectorTable {
            initial_sp: word(0),
            reset_handler: word(4),
        };

        let sp = vectors.initial_sp;
        if !(RAM_START..RAM_END).contains(&sp) {
            return Err(FirmwareError::StackPointerNotInRam(sp));
        }
        if !sp.is_multiple_of(4) {
            return Err(FirmwareError::StackPointerUnaligned(sp));
        }
        // The Cortex-M7 only runs Thumb code, so the reset vector must have bit 0 set.
        let reset = vectors.reset_handler;
        if reset & 1 == 0 {
            return Err(FirmwareError::ResetNotThumb(reset));
        }
        let end = FLASH_BASE as u64 + image.len() as u64;
        if !(FLASH_BASE as u64..end).contains(&u64::from(reset & !1)) {
            return Err(FirmwareError::ResetOutsideImage(reset));
        }
        Ok(Firmware { image, vectors })
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Firmware, FirmwareError> {
        let path = path.as_ref();
        let io_error = |source| FirmwareError::Io {
            path: path.to_path_buf(),
            source,
        };
        let file = std::fs::File::open(path).map_err(io_error)?;
        // One byte past the limit is enough to tell the image is too large,
        // and keeps a huge or endless file from being read into memory.
        let mut bytes = Vec::new();
        file.take(FIRMWARE_MAX_SIZE as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        Firmware::from_bytes(bytes)
    }

    pub fn vector_table(&self) -> VectorTable {
        self.vectors
    }

    pub fn image(&self) -> &[u8] {
        &self.image
    }
}

impl fmt::Display for FirmwareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FirmwareError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            FirmwareError::TooShort(len) => {
                write!(f, "image is {len} bytes, too short for a vector table")
            }
            FirmwareError::TooLarge => {
                write!(
                    f,
                    "image is larger than the {FIRMWARE_MAX_SIZE} bytes below the user flash \
                     at {USER_FLASH_BASE:#010X}"
                )
            }
            FirmwareError::StackPointerNotInRam(sp) => {
                write!(f, "initial stack pointer {sp:#010X} is not in RAM")
            }
            FirmwareError::StackPointerUnaligned(sp) => {
                write!(f, "initial stack pointer {sp:#010X} is not word-aligned")
            }
            FirmwareError::ResetNotThumb(addr) => {
                write!(
                    f,
                    "reset handler {addr:#010X} does not have the Thumb bit set"
                )
            }
            FirmwareError::ResetOutsideImage(addr) => {
                write!(f, "reset handler {addr:#010X} is outside the image")
            }
        }
    }
}

impl std::error::Error for FirmwareError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FirmwareError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    // A minimal image: the vector table followed by padding.
    fn image(sp: u32, reset: u32, len: usize) -> Vec<u8> {
        let mut bytes = vec![0; len];
        bytes[..4].copy_from_slice(&sp.to_le_bytes());
        bytes[4..8].copy_from_slice(&reset.to_le_bytes());
        bytes
    }

    fn valid() -> Vec<u8> {
        image(0x2002_0000, 0x0800_0021, 64)
    }

    #[test]
    fn valid_image_gives_its_vector_table() {
        let fw = Firmware::from_bytes(valid()).unwrap();
        assert_eq!(
            fw.vector_table(),
            VectorTable {
                initial_sp: 0x2002_0000,
                reset_handler: 0x0800_0021
            }
        );
        assert_eq!(fw.image(), valid().as_slice());
    }

    #[test]
    fn too_short_is_rejected() {
        let err = Firmware::from_bytes(vec![0; 7]).unwrap_err();
        assert!(matches!(err, FirmwareError::TooShort(7)));
        assert_eq!(
            err.to_string(),
            "image is 7 bytes, too short for a vector table"
        );
    }

    #[test]
    fn too_large_is_rejected() {
        let mut bytes = valid();
        bytes.resize(FIRMWARE_MAX_SIZE + 1, 0);
        let err = Firmware::from_bytes(bytes).unwrap_err();
        assert!(matches!(err, FirmwareError::TooLarge));
    }

    #[test]
    fn an_image_that_reaches_the_user_flash_is_rejected() {
        // One byte into the user flash is enough; the flash file would take it over.
        let mut bytes = valid();
        bytes.resize((USER_FLASH_BASE - FLASH_BASE) as usize + 1, 0);
        assert!(matches!(
            Firmware::from_bytes(bytes),
            Err(FirmwareError::TooLarge)
        ));
    }

    #[test]
    fn image_that_ends_where_the_user_flash_starts_is_accepted() {
        let mut bytes = valid();
        bytes.resize(FIRMWARE_MAX_SIZE, 0);
        assert!(Firmware::from_bytes(bytes).is_ok());
    }

    #[test]
    fn sp_below_ram_is_rejected() {
        let err = Firmware::from_bytes(image(0x1FFF_FFF0, 0x0800_0021, 64)).unwrap_err();
        assert!(matches!(
            err,
            FirmwareError::StackPointerNotInRam(0x1FFF_FFF0)
        ));
        assert_eq!(
            err.to_string(),
            "initial stack pointer 0x1FFFFFF0 is not in RAM"
        );
    }

    #[test]
    fn sp_above_ram_is_rejected() {
        let err = Firmware::from_bytes(image(0x4000_0000, 0x0800_0021, 64)).unwrap_err();
        assert!(matches!(err, FirmwareError::StackPointerNotInRam(_)));
    }

    #[test]
    fn unaligned_sp_is_rejected() {
        let err = Firmware::from_bytes(image(0x2002_0002, 0x0800_0021, 64)).unwrap_err();
        assert!(matches!(
            err,
            FirmwareError::StackPointerUnaligned(0x2002_0002)
        ));
        assert_eq!(
            err.to_string(),
            "initial stack pointer 0x20020002 is not word-aligned"
        );
    }

    #[test]
    fn reset_handler_without_thumb_bit_is_rejected() {
        let err = Firmware::from_bytes(image(0x2002_0000, 0x0800_0100, 64)).unwrap_err();
        assert!(matches!(err, FirmwareError::ResetNotThumb(0x0800_0100)));
        assert_eq!(
            err.to_string(),
            "reset handler 0x08000100 does not have the Thumb bit set"
        );
    }

    #[test]
    fn reset_handler_outside_image_is_rejected() {
        // The image is 64 bytes, so 0x0800_0040 is the first address past it.
        for reset in [0x0800_0041, 0x0000_0101, 0x0900_0001] {
            let err = Firmware::from_bytes(image(0x2002_0000, reset, 64)).unwrap_err();
            assert!(matches!(err, FirmwareError::ResetOutsideImage(r) if r == reset));
        }
        assert_eq!(
            Firmware::from_bytes(image(0x2002_0000, 0x0800_0041, 64))
                .unwrap_err()
                .to_string(),
            "reset handler 0x08000041 is outside the image"
        );
    }

    #[test]
    fn reset_handler_at_last_byte_is_accepted() {
        assert!(Firmware::from_bytes(image(0x2002_0000, 0x0800_003F | 1, 64)).is_ok());
    }

    #[test]
    fn missing_file_error_mentions_the_path() {
        let path = Path::new("/nonexistent/firmware.bin");
        let err = Firmware::load(path).unwrap_err();
        assert!(matches!(err, FirmwareError::Io { .. }));
        assert!(std::error::Error::source(&err).is_some());
        let msg = err.to_string();
        assert!(msg.starts_with("/nonexistent/firmware.bin: "), "{msg}");
    }

    #[test]
    fn load_reads_a_valid_file() {
        let path = std::env::temp_dir().join(format!("cmm2-core-{}-load.bin", std::process::id()));
        std::fs::write(&path, valid()).unwrap();
        let result = Firmware::load(&path);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(result.unwrap().vector_table().reset_handler, 0x0800_0021);
    }

    // Writes the bytes to a unique temp file, loads it and removes the file again.
    fn load_from_temp_file(name: &str, bytes: &[u8]) -> Result<Firmware, FirmwareError> {
        let path =
            std::env::temp_dir().join(format!("cmm2-core-{}-{name}.bin", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let result = Firmware::load(&path);
        std::fs::remove_file(&path).unwrap();
        result
    }

    #[test]
    fn load_rejects_a_file_that_reaches_the_user_flash() {
        let err = load_from_temp_file("oversized", &vec![0; FIRMWARE_MAX_SIZE + 1000]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "image is larger than the 1179648 bytes below the user flash at 0x08120000"
        );
    }

    #[test]
    fn load_accepts_a_file_that_ends_at_the_user_flash() {
        let bytes = image(0x2000_0100, 0x0800_0101, FIRMWARE_MAX_SIZE);
        assert!(load_from_temp_file("exact", &bytes).is_ok());
    }
}
