//! The emulation core. It has no UI dependency: the front end lives in cmm2-emulator.

mod firmware;

pub use firmware::{FLASH_BASE, FLASH_SIZE, Firmware, FirmwareError, VectorTable};
