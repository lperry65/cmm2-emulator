//!
//! Memory simulation
//!
//!

pub mod flash;
pub mod map;
pub mod ram;

use crate::core::fault::Fault;

/// The `len` bytes of `data` at `offset`. The bus only checks the first byte of an access
/// against a region, so one that runs past the end of the memory is a precise bus fault.
pub(crate) fn window(data: &[u8], offset: u32, len: usize) -> Result<&[u8], Fault> {
    let start = offset as usize;
    let end = start.checked_add(len).ok_or(Fault::Preciserr)?;
    data.get(start..end).ok_or(Fault::Preciserr)
}

/// Same as `window`, to write to.
pub(crate) fn window_mut(data: &mut [u8], offset: u32, len: usize) -> Result<&mut [u8], Fault> {
    let start = offset as usize;
    let end = start.checked_add(len).ok_or(Fault::Preciserr)?;
    data.get_mut(start..end).ok_or(Fault::Preciserr)
}
