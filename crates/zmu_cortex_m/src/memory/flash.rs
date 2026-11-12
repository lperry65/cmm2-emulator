//!
//! Flash Memory simulation
//!
//!

use crate::bus::Bus;
use crate::core::fault::Fault;
use byteorder::{ByteOrder, LittleEndian};

#[derive(Debug)]
/// Flash memory with configurable start address and data content
pub struct FlashMemory {
    base: u32,
    data: Box<[u8]>,
}

impl FlashMemory {
    /// make a flash data instance with given start address, size and data content
    pub fn new(size: usize, new_data: &[u8]) -> Self {
        let mut data = vec![0_u8; size].into_boxed_slice();
        data.copy_from_slice(new_data);

        Self { base: 0, data }
    }

    /// Same, but the flash sits at `base` in the address space.
    pub fn new_at(base: u32, size: usize, new_data: &[u8]) -> Self {
        let mut f = Self::new(size, new_data);
        f.base = base;
        f
    }

    /// Address of the first byte.
    pub fn base(&self) -> u32 {
        self.base
    }

    /// Get the length
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// check if the memory is empty
    pub fn is_empty(&self) -> bool {
        self.data.len() == 0
    }
}

impl Bus for FlashMemory {
    #[inline(always)]
    fn read8(&self, addr: u32) -> Result<u8, Fault> {
        let a = addr.wrapping_sub(self.base) as usize;
        Ok(self.data[a])
    }
    #[inline(always)]
    fn read16(&self, addr: u32) -> Result<u16, Fault> {
        let a = addr.wrapping_sub(self.base) as usize;

        Ok(LittleEndian::read_u16(&self.data[a..a + 2]))
    }

    #[inline(always)]
    fn read32(&mut self, addr: u32) -> Result<u32, Fault> {
        let a = addr.wrapping_sub(self.base) as usize;
        Ok(LittleEndian::read_u32(&self.data[a..a + 4]))
    }

    #[inline(always)]
    fn write32(&mut self, addr: u32, value: u32) -> Result<(), Fault> {
        let a = addr.wrapping_sub(self.base) as usize;
        self.data[a..a + 4].copy_from_slice(&value.to_le_bytes());
        Ok(())
    }

    #[inline(always)]
    fn write16(&mut self, addr: u32, value: u16) -> Result<(), Fault> {
        let a = addr.wrapping_sub(self.base) as usize;
        self.data[a..a + 2].copy_from_slice(&value.to_le_bytes());
        Ok(())
    }
    #[inline(always)]
    fn write8(&mut self, addr: u32, value: u8) -> Result<(), Fault> {
        let a = addr.wrapping_sub(self.base) as usize;
        self.data[a] = value;
        Ok(())
    }

    #[inline(always)]
    fn in_range(&self, addr: u32) -> bool {
        addr.wrapping_sub(self.base) < (self.data.len() as u32)
    }
}

#[test]
fn test_new() {
    // should be able to make new instance of memory
    let data = [0; 1024];
    let _mem = FlashMemory::new(1024, &data);
}

#[test]
fn test_load() {
    let mut mem = FlashMemory::new(1024, &vec![42u8; 1024]);
    assert_eq!(mem.read8(0).unwrap(), 42);
    assert_eq!(mem.read16(0).unwrap(), (42 << 8) + 42);
    assert_eq!(
        mem.read32(0).unwrap(),
        (42 << 24) + (42 << 16) + (42 << 8) + 42
    );
}

#[test]
fn test_in_range() {
    {
        /* no offset */
        let mem = FlashMemory::new(1024, &vec![0u8; 1024]);
        assert!(mem.in_range(0));
        assert!(mem.in_range(1023));
        assert!(!mem.in_range(1024));
        assert!(!mem.in_range(0xFFFF_FFFF));
    }
}
