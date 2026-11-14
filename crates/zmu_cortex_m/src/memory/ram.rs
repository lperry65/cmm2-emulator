//!
//! RAM simulation
//!
//!

use crate::bus::Bus;
use crate::core::fault::Fault;
use crate::memory::{window, window_mut};
use byteorder::{ByteOrder, LittleEndian};

#[derive(Debug)]
/// RAM memory with configurable start address
pub struct RAM {
    start_address: u32,
    data: Box<[u8]>,
}

impl RAM {
    /// Create RAM memory data struct with configurable start address and size
    pub fn new(start_address: u32, size: usize) -> Self {
        let data = vec![0_u8; size].into_boxed_slice();

        Self {
            start_address,
            data,
        }
    }
    ///
    pub fn new_with_fill(start_address: u32, size: usize, fill: u8) -> Self {
        let data = vec![fill; size].into_boxed_slice();

        Self {
            start_address,
            data,
        }
    }
}

impl Bus for RAM {
    #[inline(always)]
    fn read8(&self, addr: u32) -> Result<u8, Fault> {
        Ok(window(&self.data, addr.wrapping_sub(self.start_address), 1)?[0])
    }

    #[inline(always)]
    fn read16(&self, addr: u32) -> Result<u16, Fault> {
        let bytes = window(&self.data, addr.wrapping_sub(self.start_address), 2)?;
        Ok(LittleEndian::read_u16(bytes))
    }

    #[inline(always)]
    fn read32(&mut self, addr: u32) -> Result<u32, Fault> {
        let bytes = window(&self.data, addr.wrapping_sub(self.start_address), 4)?;
        Ok(LittleEndian::read_u32(bytes))
    }

    #[inline(always)]
    fn write8(&mut self, addr: u32, value: u8) -> Result<(), Fault> {
        window_mut(&mut self.data, addr.wrapping_sub(self.start_address), 1)?[0] = value;
        Ok(())
    }

    #[inline(always)]
    fn write16(&mut self, addr: u32, value: u16) -> Result<(), Fault> {
        let bytes = window_mut(&mut self.data, addr.wrapping_sub(self.start_address), 2)?;
        LittleEndian::write_u16(bytes, value);
        Ok(())
    }

    #[inline(always)]
    fn write32(&mut self, addr: u32, value: u32) -> Result<(), Fault> {
        let bytes = window_mut(&mut self.data, addr.wrapping_sub(self.start_address), 4)?;
        LittleEndian::write_u32(bytes, value);
        Ok(())
    }

    #[inline(always)]
    fn in_range(&self, addr: u32) -> bool {
        addr.wrapping_sub(self.start_address) < self.data.len() as u32
    }
}

#[test]
fn test_new() {
    // should be able to make new instance of memory
    let _mem = RAM::new(0, 1024);
}

#[test]
fn test_in_range() {
    {
        /* no offset */
        let mem = RAM::new(0, 1024);
        assert!(mem.in_range(0));
        assert!(mem.in_range(1023));
        assert!(!mem.in_range(1024));
        assert!(!mem.in_range(0xFFFF_FFFF));
    }

    {
        /* offset of 0x8000_0000 */
        let mem = RAM::new(0x8000_0000, 1024);
        assert!(mem.in_range(0x8000_0000));
        assert!(mem.in_range(0x8000_0001));
        assert!(!mem.in_range(0x8000_0000 + 1024));
        assert!(!mem.in_range(0x8000_0000 + 0xffff));
    }
}

#[test]
fn test_write_read() {
    {
        let mut mem = RAM::new(0, 1024);
        mem.write32(0, 0xAABB_CCDD).unwrap();
        assert_eq!(mem.read32(0).unwrap(), 0xAABB_CCDD);
        mem.write32(1020, 0xAABB_CCDD).unwrap();
        assert_eq!(mem.read32(1020).unwrap(), 0xAABB_CCDD);
    }

    {
        let mut mem = RAM::new(0, 1024);
        mem.write16(0, 0xAABB).unwrap();
        assert_eq!(mem.read16(0).unwrap(), 0xAABB);
        mem.write16(1022, 0xCCDD).unwrap();
        assert_eq!(mem.read16(1022).unwrap(), 0xCCDD);
    }

    {
        let mut mem = RAM::new(0, 1024);
        mem.write8(0, 0xAA).unwrap();
        assert_eq!(mem.read8(0).unwrap(), 0xAA);
        mem.write8(1022, 0xCC).unwrap();
        assert_eq!(mem.read8(1022).unwrap(), 0xCC);
    }
}

#[test]
fn test_access_past_the_end_is_a_fault() {
    let mut mem = RAM::new(0x2000_0000, 1024);
    let end = 0x2000_0000 + 1024;
    assert_eq!(mem.read32(end - 3), Err(Fault::Preciserr));
    assert_eq!(mem.read16(end - 1), Err(Fault::Preciserr));
    assert_eq!(mem.read8(end), Err(Fault::Preciserr));
    assert_eq!(mem.write32(end - 2, 1), Err(Fault::Preciserr));
    assert_eq!(mem.write16(end - 1, 1), Err(Fault::Preciserr));
    assert_eq!(mem.write8(end, 1), Err(Fault::Preciserr));
    // below the start, the subtraction must not underflow either
    assert_eq!(mem.read8(0), Err(Fault::Preciserr));
    assert_eq!(mem.write8(0x1fff_ffff, 1), Err(Fault::Preciserr));
    // nothing was written by the failed write
    assert_eq!(mem.read32(end - 4), Ok(0));
}
