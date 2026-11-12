//! The emulation core. It has no UI dependency: the front end lives in cmm2-emulator.

mod firmware;
mod svd;

pub use firmware::{FLASH_BASE, FLASH_SIZE, Firmware, FirmwareError, VectorTable};
pub use svd::{Field, Lookup, Svd, SvdError};
