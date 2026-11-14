//! The part of the STM32H743 address space that the machine owns.
//!
//! The flash and the DTCM RAM are not here: the CPU core keeps them for speed, so reads and
//! writes to those windows are unmapped as far as this map is concerned. The flash base is
//! `FLASH_BASE`, in the firmware module.

use std::cell::Cell;
use std::fmt;
use std::rc::Rc;

use crate::clock::Clock;
use crate::console::Console;
use crate::display::{Display, Frame, FrameError};
use crate::ds3231::Ds3231Slot;
use crate::flash_ops::FlashOps;
use crate::peripheral::{Peripheral, PeripheralAccess, RegisterStub, ResetCause};
use crate::rtc::Rtc;
use crate::sd_slot::SdSlot;
use crate::svd::Svd;
use crate::usb::UsbPort;

/// DTCM RAM. Owned by the CPU core, like the flash.
pub const DTCM_BASE: u32 = 0x2000_0000;
pub const DTCM_SIZE: usize = 128 * 1024;

/// How many unmapped accesses are kept; later ones are only counted.
const LOG_LIMIT: usize = 1024;

pub struct RegionSpec {
    pub name: &'static str,
    pub base: u32,
    pub size: usize,
}

/// The regions this map provides, sorted by base address.
pub const REGIONS: [RegionSpec; 7] = [
    RegionSpec {
        name: "ITCM",
        base: 0x0000_0000,
        size: 64 * 1024,
    },
    RegionSpec {
        name: "AXI SRAM",
        base: 0x2400_0000,
        size: 512 * 1024,
    },
    RegionSpec {
        name: "SRAM1/2",
        base: 0x3000_0000,
        size: 256 * 1024,
    },
    RegionSpec {
        name: "SRAM3",
        base: 0x3004_0000,
        size: 32 * 1024,
    },
    RegionSpec {
        name: "SRAM4",
        base: 0x3800_0000,
        size: 64 * 1024,
    },
    RegionSpec {
        name: "Backup SRAM",
        base: 0x3880_0000,
        size: 4 * 1024,
    },
    RegionSpec {
        name: "SDRAM",
        base: 0xD000_0000,
        size: 32 * 1024 * 1024,
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessKind {
    Read,
    /// The value that was written.
    Write(u32),
}

/// An access that touched at least one unmapped byte. A partly mapped access is one record,
/// with the original address and size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnmappedAccess {
    pub addr: u32,
    pub size: u8,
    pub kind: AccessKind,
    /// Address of the instruction that made the access.
    pub pc: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum MemoryError {
    Unmapped(u32),
    /// A peripheral window would overlap a RAM region or another window.
    Overlap(u32),
}

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemoryError::Unmapped(addr) => write!(f, "address {addr:#010x} is not mapped"),
            MemoryError::Overlap(addr) => {
                write!(f, "window at {addr:#010x} overlaps mapped memory")
            }
        }
    }
}

impl std::error::Error for MemoryError {}

struct Region {
    base: u32,
    data: Box<[u8]>,
}

struct Window {
    base: u32,
    size: u32,
    device: Box<dyn Peripheral>,
}

type Trace = Box<dyn FnMut(&PeripheralAccess)>;

pub struct MemoryMap {
    regions: Vec<Region>,
    windows: Vec<Window>,
    // Consulted only when no window holds the access.
    fallbacks: Vec<Window>,
    trace: Option<Trace>,
    unmapped: Vec<UnmappedAccess>,
    dropped: u64,
    pc: Rc<Cell<u32>>,
    flash_ops: FlashOps,
    clock: Clock,
    console: Console,
    sd_slot: SdSlot,
    ds3231: Ds3231Slot,
    rtc: Rtc,
    usb_port: UsbPort,
    display: Display,
}

impl Default for MemoryMap {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryMap {
    pub fn new() -> MemoryMap {
        let regions = REGIONS
            .iter()
            .map(|spec| Region {
                base: spec.base,
                data: vec![0; spec.size].into_boxed_slice(),
            })
            .collect();
        let clock = Clock::new();
        MemoryMap {
            regions,
            windows: Vec::new(),
            fallbacks: Vec::new(),
            trace: None,
            unmapped: Vec::new(),
            dropped: 0,
            pc: Rc::new(Cell::new(0)),
            flash_ops: FlashOps::new(),
            usb_port: UsbPort::new(clock.clone()),
            rtc: Rtc::new(clock.clone()),
            clock,
            console: Console::new(),
            sd_slot: SdSlot::new(),
            ds3231: Ds3231Slot::default(),
            display: Display::default(),
        }
    }

    /// The address of the instruction that is running, which the CPU keeps up to date. It is
    /// 0 until a CPU is attached.
    pub fn pc_cell(&self) -> Rc<Cell<u32>> {
        self.pc.clone()
    }

    /// The queue of flash changes that the flash controller makes and the CPU carries out.
    pub fn flash_ops(&self) -> FlashOps {
        self.flash_ops.clone()
    }

    /// The machine's time, which the CPU keeps up to date. It is 0 until a CPU is attached.
    pub fn clock(&self) -> Clock {
        self.clock.clone()
    }

    /// The text channel of the console USART, shared with the front end.
    pub fn console(&self) -> Console {
        self.console.clone()
    }

    /// The SD socket, shared with the front end, which puts a card image in and takes it back.
    pub fn sd_slot(&self) -> SdSlot {
        self.sd_slot.clone()
    }

    /// The DS3231 on PD4 and PD5, shared with the front end, which gives it the host's clock and
    /// keeps its offset from one run to the next.
    pub fn ds3231(&self) -> Ds3231Slot {
        self.ds3231.clone()
    }

    /// The real-time clock, shared with the front end, which reads and sets its time and date
    /// off the bus. `cmm2_memory` maps it where the guest sees it.
    pub fn rtc(&self) -> Rtc {
        self.rtc.clone()
    }

    /// The USB host port, shared with the front end, which plugs devices into it.
    pub fn usb_port(&self) -> UsbPort {
        self.usb_port.clone()
    }

    /// The LTDC's side of the display, which the controller model keeps up to date.
    pub(crate) fn display(&self) -> Display {
        self.display.clone()
    }

    /// The picture the LTDC is scanning out now, read from the emulated RAM.
    pub fn frame(&self) -> Result<Frame, FrameError> {
        self.display.frame(|addr, len| self.ram(addr, len))
    }

    /// `len` bytes of RAM from `addr`, if they all lie in one region.
    fn ram(&self, addr: u32, len: usize) -> Option<&[u8]> {
        let (region, offset) = self.locate(addr)?;
        self.regions[region].data.get(offset..offset + len)
    }

    pub fn read8(&mut self, addr: u32) -> u8 {
        self.read_bytes::<1>(addr)[0]
    }

    pub fn read16(&mut self, addr: u32) -> u16 {
        u16::from_le_bytes(self.read_bytes(addr))
    }

    pub fn read32(&mut self, addr: u32) -> u32 {
        u32::from_le_bytes(self.read_bytes(addr))
    }

    pub fn write8(&mut self, addr: u32, value: u8) {
        self.write_bytes(addr, [value], u32::from(value));
    }

    pub fn write16(&mut self, addr: u32, value: u16) {
        self.write_bytes(addr, value.to_le_bytes(), u32::from(value));
    }

    pub fn write32(&mut self, addr: u32, value: u32) {
        self.write_bytes(addr, value.to_le_bytes(), value);
    }

    /// Maps `device` over `size` bytes at `base`. Nothing is allocated for the window, so it
    /// can be as large as the address range it stands for. An access that runs past the end
    /// of the window never reaches `device`: it is logged as partly unmapped, unless a
    /// fallback holds all of it. The window may lie inside a fallback, where it wins.
    pub fn map_peripheral(
        &mut self,
        base: u32,
        size: u32,
        device: Box<dyn Peripheral>,
    ) -> Result<(), MemoryError> {
        self.check_free(base, size, &self.windows)?;
        self.windows.push(Window { base, size, device });
        Ok(())
    }

    /// Like `map_peripheral`, but the window only serves the accesses that no
    /// `map_peripheral` window holds. It may not overlap RAM or another fallback.
    pub fn map_fallback(
        &mut self,
        base: u32,
        size: u32,
        device: Box<dyn Peripheral>,
    ) -> Result<(), MemoryError> {
        self.check_free(base, size, &self.fallbacks)?;
        self.fallbacks.push(Window { base, size, device });
        Ok(())
    }

    // Whether `base..base + size` is clear of the RAM regions and of `windows`.
    fn check_free(&self, base: u32, size: u32, windows: &[Window]) -> Result<(), MemoryError> {
        let start = u64::from(base);
        let end = start + u64::from(size);
        let overlaps = |other_base: u32, other_size: u64| {
            let other_start = u64::from(other_base);
            start < other_start + other_size && other_start < end
        };
        let taken = self
            .regions
            .iter()
            .any(|r| overlaps(r.base, r.data.len() as u64))
            || windows.iter().any(|w| overlaps(w.base, u64::from(w.size)));
        if taken {
            return Err(MemoryError::Overlap(base));
        }
        Ok(())
    }

    /// Puts a `RegisterStub` over the peripheral space (0x4000_0000 up to 0x6000_0000) and
    /// the Cortex-M private peripheral bus (0xE000_0000 up to 0xE010_0000), as fallbacks:
    /// peripherals that are modelled win. Returns the error of the first window that cannot
    /// be mapped; a window mapped before it stays.
    pub fn map_unmodelled_peripherals(&mut self) -> Result<(), MemoryError> {
        for (base, size) in [(0x4000_0000, 0x2000_0000), (0xE000_0000, 0x10_0000)] {
            let stub = RegisterStub::with_reset_values(base, Svd::stm32h743());
            self.map_fallback(base, size, Box::new(stub))?;
        }
        Ok(())
    }

    /// Resets every window and fallback. The RAM is left alone: SRAM keeps its contents through
    /// a system reset on the chip. The unmapped log and the trace callback stay too.
    pub fn reset_peripherals(&mut self, cause: ResetCause) {
        for window in self.windows.iter_mut().chain(&mut self.fallbacks) {
            window.device.reset(cause);
        }
    }

    /// Ticks every window and fallback; the CPU calls it when the time a device asked to be
    /// woken at (`Clock::wake_at`) has come.
    pub fn tick(&mut self) {
        for window in self.windows.iter_mut().chain(&mut self.fallbacks) {
            window.device.tick();
        }
    }

    /// Calls `f` after every peripheral access, modelled or not, in order. `None` stops it.
    pub fn set_peripheral_trace(&mut self, f: Option<Trace>) {
        self.trace = f;
    }

    /// Copies `bytes` to `addr`. Stops at the first unmapped byte, so the bytes before it
    /// have already been copied.
    pub fn load(&mut self, addr: u32, bytes: &[u8]) -> Result<(), MemoryError> {
        let mut addr = addr;
        let mut rest = bytes;
        while !rest.is_empty() {
            let Some((region, offset)) = self.locate(addr) else {
                return Err(MemoryError::Unmapped(addr));
            };
            let data = &mut self.regions[region].data;
            let n = rest.len().min(data.len() - offset);
            data[offset..offset + n].copy_from_slice(&rest[..n]);
            rest = &rest[n..];
            addr = addr.wrapping_add(n as u32);
        }
        Ok(())
    }

    /// Drains the log of unmapped accesses (the first 1024 since the last drain).
    pub fn take_unmapped(&mut self) -> Vec<UnmappedAccess> {
        std::mem::take(&mut self.unmapped)
    }

    /// How many records were lost because the log was full. Draining does not reset it.
    pub fn unmapped_dropped(&self) -> u64 {
        self.dropped
    }

    /// The region holding `addr` and the offset into it.
    fn locate(&self, addr: u32) -> Option<(usize, usize)> {
        self.regions.iter().enumerate().find_map(|(i, r)| {
            let offset = addr.checked_sub(r.base)? as usize;
            (offset < r.data.len()).then_some((i, offset))
        })
    }

    /// The device whose window holds all `size` bytes at `addr`, and the offset into it: a
    /// specific window if there is one, else a fallback. An access that runs past the end of
    /// a window is not served by it, so one that straddles the edge of a specific window goes
    /// to the fallback around it, which sees it whole. If nothing holds it all, it is handled
    /// like any other partly unmapped access: byte by byte against the RAM, one record, and
    /// no peripheral sees any of it.
    fn window(&mut self, addr: u32, size: u8) -> Option<(&mut dyn Peripheral, u32)> {
        let holds = |w: &Window| {
            let offset = addr.checked_sub(w.base)?;
            (u64::from(offset) + u64::from(size) <= u64::from(w.size)).then_some(offset)
        };
        let windows = if self.windows.iter().any(|w| holds(w).is_some()) {
            &mut self.windows
        } else {
            &mut self.fallbacks
        };
        let (i, offset) = windows
            .iter()
            .enumerate()
            .find_map(|(i, w)| Some((i, holds(w)?)))?;
        Some((&mut *windows[i].device, offset))
    }

    fn traced(&mut self, addr: u32, size: u8, write: bool, value: u32) {
        if let Some(trace) = &mut self.trace {
            trace(&PeripheralAccess {
                addr,
                size,
                write,
                value,
                pc: self.pc.get(),
            });
        }
    }

    /// Unmapped bytes read as 0.
    fn read_bytes<const N: usize>(&mut self, addr: u32) -> [u8; N] {
        let mut out = [0; N];
        if let Some((region, offset)) = self.locate(addr)
            && let Some(src) = self.regions[region].data.get(offset..offset + N)
        {
            out.copy_from_slice(src);
            return out;
        }
        if let Some((device, offset)) = self.window(addr, N as u8) {
            let mask = u32::MAX >> (32 - 8 * N as u32);
            let value = device.read(offset, N as u8) & mask;
            self.traced(addr, N as u8, false, value);
            out.copy_from_slice(&value.to_le_bytes()[..N]);
            return out;
        }
        let mut missed = false;
        for (i, byte) in out.iter_mut().enumerate() {
            match self.locate(addr.wrapping_add(i as u32)) {
                Some((region, offset)) => *byte = self.regions[region].data[offset],
                None => missed = true,
            }
        }
        if missed {
            self.record(addr, N, AccessKind::Read);
        }
        out
    }

    /// `value` is only for the log; `bytes` is what is stored.
    fn write_bytes<const N: usize>(&mut self, addr: u32, bytes: [u8; N], value: u32) {
        if let Some((region, offset)) = self.locate(addr)
            && let Some(dst) = self.regions[region].data.get_mut(offset..offset + N)
        {
            dst.copy_from_slice(&bytes);
            return;
        }
        if let Some((device, offset)) = self.window(addr, N as u8) {
            device.write(offset, N as u8, value);
            self.traced(addr, N as u8, true, value);
            return;
        }
        let mut missed = false;
        for (i, byte) in bytes.into_iter().enumerate() {
            match self.locate(addr.wrapping_add(i as u32)) {
                Some((region, offset)) => self.regions[region].data[offset] = byte,
                None => missed = true,
            }
        }
        if missed {
            self.record(addr, N, AccessKind::Write(value));
        }
    }

    fn record(&mut self, addr: u32, size: usize, kind: AccessKind) {
        if self.unmapped.len() < LOG_LIMIT {
            self.unmapped.push(UnmappedAccess {
                addr,
                size: size as u8,
                kind,
                pc: self.pc.get(),
            });
        } else {
            self.dropped += 1;
        }
    }
}

#[cfg(test)]
mod tests;
