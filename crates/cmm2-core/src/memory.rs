//! The part of the STM32H743 address space that the machine owns.
//!
//! The flash and the DTCM RAM are not here: the CPU core keeps them for speed, so reads and
//! writes to those windows are unmapped as far as this map is concerned. The flash base is
//! `FLASH_BASE`, in the firmware module.

use std::fmt;

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
}

#[derive(Debug, PartialEq, Eq)]
pub enum MemoryError {
    Unmapped(u32),
}

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemoryError::Unmapped(addr) => write!(f, "address {addr:#010x} is not mapped"),
        }
    }
}

impl std::error::Error for MemoryError {}

struct Region {
    base: u32,
    data: Box<[u8]>,
}

pub struct MemoryMap {
    regions: Vec<Region>,
    unmapped: Vec<UnmappedAccess>,
    dropped: u64,
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
        MemoryMap {
            regions,
            unmapped: Vec::new(),
            dropped: 0,
        }
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

    /// Unmapped bytes read as 0.
    fn read_bytes<const N: usize>(&mut self, addr: u32) -> [u8; N] {
        let mut out = [0; N];
        if let Some((region, offset)) = self.locate(addr)
            && let Some(src) = self.regions[region].data.get(offset..offset + N)
        {
            out.copy_from_slice(src);
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
            });
        } else {
            self.dropped += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FLASH_BASE;

    fn region(name: &str) -> &'static RegionSpec {
        REGIONS.iter().find(|r| r.name == name).unwrap()
    }

    fn round_trip(name: &str) {
        let spec = region(name);
        let first = spec.base;
        let last = spec.base + (spec.size as u32 - 1);
        let mut m = MemoryMap::new();

        m.write8(first, 0xA5);
        m.write8(last, 0x5A);
        assert_eq!(m.read8(first), 0xA5);
        assert_eq!(m.read8(last), 0x5A);

        m.write16(first, 0xBEEF);
        m.write16(last - 1, 0xCAFE);
        assert_eq!(m.read16(first), 0xBEEF);
        assert_eq!(m.read16(last - 1), 0xCAFE);

        m.write32(first, 0xDEAD_BEEF);
        m.write32(last - 3, 0x1234_5678);
        assert_eq!(m.read32(first), 0xDEAD_BEEF);
        assert_eq!(m.read32(last - 3), 0x1234_5678);

        assert!(m.take_unmapped().is_empty());
    }

    #[test]
    fn itcm_round_trips() {
        round_trip("ITCM");
    }

    #[test]
    fn axi_sram_round_trips() {
        round_trip("AXI SRAM");
    }

    #[test]
    fn sram12_round_trips() {
        round_trip("SRAM1/2");
    }

    #[test]
    fn sram3_round_trips() {
        round_trip("SRAM3");
    }

    #[test]
    fn sram4_round_trips() {
        round_trip("SRAM4");
    }

    #[test]
    fn backup_sram_round_trips() {
        round_trip("Backup SRAM");
    }

    #[test]
    fn sdram_round_trips() {
        round_trip("SDRAM");
    }

    #[test]
    fn memory_starts_zeroed() {
        let mut m = MemoryMap::new();
        assert_eq!(m.read32(0xD000_0000), 0);
        assert_eq!(m.read32(0xD1FF_FFFC), 0);
    }

    #[test]
    fn writes_are_little_endian() {
        let mut m = MemoryMap::new();
        m.write32(0x2400_0000, 0x1122_3344);
        assert_eq!(m.read8(0x2400_0000), 0x44);
        assert_eq!(m.read8(0x2400_0001), 0x33);
        assert_eq!(m.read8(0x2400_0002), 0x22);
        assert_eq!(m.read8(0x2400_0003), 0x11);
        assert_eq!(m.read16(0x2400_0002), 0x1122);
    }

    #[test]
    fn unaligned_access_round_trips() {
        let mut m = MemoryMap::new();
        m.write32(0x3000_0001, 0xA1B2_C3D4);
        assert_eq!(m.read32(0x3000_0001), 0xA1B2_C3D4);
        m.write16(0x3000_0101, 0x7788);
        assert_eq!(m.read16(0x3000_0101), 0x7788);
    }

    #[test]
    fn access_across_adjacent_regions() {
        let mut m = MemoryMap::new();
        m.write32(0x3003_FFFE, 0x4433_2211);
        assert_eq!(m.read32(0x3003_FFFE), 0x4433_2211);
        assert_eq!(m.read16(0x3003_FFFE), 0x2211);
        assert_eq!(m.read16(0x3004_0000), 0x4433);
        assert!(m.take_unmapped().is_empty());
    }

    #[test]
    fn addresses_around_each_region_are_unmapped() {
        let mut m = MemoryMap::new();
        for spec in &REGIONS {
            let end = spec.base.wrapping_add(spec.size as u32);
            let before = spec.base.wrapping_sub(1);
            for addr in [end, before] {
                // The neighbours that are other regions are mapped.
                let mapped = REGIONS
                    .iter()
                    .any(|r| addr.wrapping_sub(r.base) < r.size as u32);
                if !mapped {
                    m.read8(addr);
                    assert_eq!(m.take_unmapped().len(), 1, "{} {addr:#x}", spec.name);
                }
            }
        }
        assert_eq!(m.read8(0xD200_0000), 0);
        assert_eq!(m.take_unmapped().len(), 1);
        assert_eq!(m.read8(0xCFFF_FFFF), 0);
        assert_eq!(m.take_unmapped().len(), 1);
    }

    #[test]
    fn unmapped_read_returns_zero_and_is_logged() {
        let mut m = MemoryMap::new();
        assert_eq!(m.read16(0x4000_0000), 0);
        assert_eq!(
            m.take_unmapped(),
            [UnmappedAccess {
                addr: 0x4000_0000,
                size: 2,
                kind: AccessKind::Read
            }]
        );
    }

    #[test]
    fn unmapped_write_is_ignored_and_logged() {
        let mut m = MemoryMap::new();
        m.write32(0xE000_ED00, 0xCAFE_F00D);
        assert_eq!(
            m.take_unmapped(),
            [UnmappedAccess {
                addr: 0xE000_ED00,
                size: 4,
                kind: AccessKind::Write(0xCAFE_F00D)
            }]
        );
        assert_eq!(m.read32(0xE000_ED00), 0);
    }

    #[test]
    fn access_off_the_end_of_sdram_is_split() {
        let mut m = MemoryMap::new();
        m.write32(0xD1FF_FFFE, 0x4433_2211);
        assert_eq!(
            m.take_unmapped(),
            [UnmappedAccess {
                addr: 0xD1FF_FFFE,
                size: 4,
                kind: AccessKind::Write(0x4433_2211)
            }]
        );
        // The two bytes inside SDRAM were written.
        assert_eq!(m.read16(0xD1FF_FFFE), 0x2211);
        m.take_unmapped();

        // A read gets the mapped bytes and zeros for the rest.
        assert_eq!(m.read32(0xD1FF_FFFE), 0x0000_2211);
        assert_eq!(
            m.take_unmapped(),
            [UnmappedAccess {
                addr: 0xD1FF_FFFE,
                size: 4,
                kind: AccessKind::Read
            }]
        );
    }

    #[test]
    fn straddle_below_a_region_is_split() {
        let mut m = MemoryMap::new();
        m.write16(0xCFFF_FFFF, 0xAABB);
        assert_eq!(m.take_unmapped().len(), 1);
        assert_eq!(m.read8(0xD000_0000), 0xAA);
    }

    #[test]
    fn flash_and_dtcm_windows_belong_to_the_cpu() {
        let mut m = MemoryMap::new();
        assert_eq!(m.read32(FLASH_BASE), 0);
        assert_eq!(m.read32(DTCM_BASE), 0);
        m.write32(DTCM_BASE + DTCM_SIZE as u32 - 4, 1);
        assert_eq!(m.take_unmapped().len(), 3);
    }

    #[test]
    fn log_is_bounded() {
        let mut m = MemoryMap::new();
        for i in 0..1030 {
            m.read8(0x4000_0000 + i);
        }
        assert_eq!(m.unmapped_dropped(), 6);
        let log = m.take_unmapped();
        assert_eq!(log.len(), 1024);
        assert_eq!(log[0].addr, 0x4000_0000);
        assert_eq!(log[1023].addr, 0x4000_0000 + 1023);

        m.read8(0x5000_0000);
        assert_eq!(m.take_unmapped().len(), 1);
    }

    #[test]
    fn load_copies_into_memory() {
        let mut m = MemoryMap::new();
        m.load(0x0000_0000, &[1, 2, 3, 4]).unwrap();
        assert_eq!(m.read32(0), 0x0403_0201);
        m.load(0xD000_1000, &[9, 8, 7]).unwrap();
        assert_eq!(m.read8(0xD000_1002), 7);
    }

    #[test]
    fn load_into_unmapped_memory_fails() {
        let mut m = MemoryMap::new();
        assert_eq!(
            m.load(0xD1FF_FFFE, &[1, 2, 3, 4]),
            Err(MemoryError::Unmapped(0xD200_0000))
        );
        assert_eq!(
            m.load(FLASH_BASE, &[1]),
            Err(MemoryError::Unmapped(FLASH_BASE))
        );
        assert!(m.take_unmapped().is_empty());
    }

    #[test]
    fn creating_the_map_is_cheap() {
        // Zeroed allocations come straight from the OS, so the 32 MB of SDRAM costs nothing.
        let _ = MemoryMap::new();
    }
}
