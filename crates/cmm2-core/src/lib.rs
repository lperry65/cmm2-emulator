//! The emulation core. It has no UI dependency: the front end lives in cmm2-emulator.

mod cpu;
mod firmware;
mod irq;
mod memory;
mod peripheral;
mod svd;

pub use cpu::{Cpu, CpuError, Fault, Stop};
pub use firmware::{FLASH_BASE, FLASH_SIZE, Firmware, FirmwareError, VectorTable};
pub use irq::Irqs;
pub use memory::{
    AccessKind, DTCM_BASE, DTCM_SIZE, MemoryError, MemoryMap, REGIONS, RegionSpec, UnmappedAccess,
};
pub use peripheral::{Peripheral, PeripheralAccess, RegisterStub};
pub use svd::{Field, Lookup, Svd, SvdError};
