//! The emulation core. It has no UI dependency: the front end lives in cmm2-emulator.

mod cpu;
mod firmware;
mod irq;
mod machine;
mod memory;
mod peripheral;
mod pwr;
mod rcc;
mod registers;
mod svd;

pub use cpu::{Cpu, CpuError, Fault, Stop};
pub use firmware::{FLASH_BASE, FLASH_SIZE, Firmware, FirmwareError, VectorTable};
pub use irq::Irqs;
pub use machine::cmm2_memory;
pub use memory::{
    AccessKind, DTCM_BASE, DTCM_SIZE, MemoryError, MemoryMap, REGIONS, RegionSpec, UnmappedAccess,
};
pub use peripheral::{Peripheral, PeripheralAccess, RegisterStub, StubRegisters};
pub use pwr::{PWR_BASE, PWR_SIZE, Pwr, PwrRegisters};
pub use rcc::{RCC_BASE, RCC_SIZE, Rcc, RccRegisters};
pub use registers::{Registers, WordAccess};
pub use svd::{Field, Lookup, Svd, SvdError};
