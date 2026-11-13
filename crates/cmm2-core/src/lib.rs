//! The emulation core. It has no UI dependency: the front end lives in cmm2-emulator.

mod clock;
mod console;
mod cpu;
mod firmware;
mod flash;
mod flash_ops;
mod gpio;
mod irq;
mod ltdc;
mod machine;
mod memory;
mod peripheral;
mod pwr;
mod rcc;
mod registers;
mod rtc;
mod svd;
mod timer;
mod usart;

pub use clock::Clock;
pub use console::Console;
pub use cpu::{Cpu, CpuError, Fault, Stop};
pub use firmware::{FLASH_BASE, FLASH_SIZE, Firmware, FirmwareError, VectorTable};
pub use flash::{FLASH_REGS_BASE, FLASH_REGS_SIZE, FlashController, FlashRegisters};
pub use flash_ops::{FlashOp, FlashOps};
pub use gpio::{GPIO_BASE, GPIO_PORT_SIZE, GPIO_PORTS, Gpio, GpioRegisters};
pub use irq::Irqs;
pub use ltdc::{LTDC_BASE, LTDC_SIZE, Ltdc, LtdcRegisters};
pub use machine::cmm2_memory;
pub use memory::{
    AccessKind, DTCM_BASE, DTCM_SIZE, MemoryError, MemoryMap, REGIONS, RegionSpec, UnmappedAccess,
};
pub use peripheral::{Peripheral, PeripheralAccess, RegisterStub, ResetCause, StubRegisters};
pub use pwr::{PWR_BASE, PWR_SIZE, Pwr, PwrRegisters};
pub use rcc::{RCC_BASE, RCC_SIZE, Rcc, RccRegisters};
pub use registers::{Registers, WordAccess};
pub use rtc::{RTC_BASE, RTC_SIZE, Rtc, RtcRegisters};
pub use svd::{Field, Lookup, Svd, SvdError};
pub use timer::{TIM5_BASE, TIM16_BASE, TIMER_SIZE, Timer, TimerRegisters};
pub use usart::{USART_SIZE, USART1_BASE, Usart, UsartRegisters};
