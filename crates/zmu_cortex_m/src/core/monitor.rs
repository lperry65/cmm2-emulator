//!
//! The local exclusive monitor (A3.4.1, Figure A3-6, Table A3-2)
//!

use crate::Processor;

/// The word that a Load-Exclusive tags. The tagged block is IMPLEMENTATION DEFINED, from one word
/// (a = 2) up (A3.4.3); I take the smallest, so a Store-Exclusive to a different word never passes.
fn tag(address: u32) -> u32 {
    address >> 2
}

///
/// Memory monitoring for atomic instructions (STREX, LDREX, ...)
///
/// There is one core and no other observer, so there is only the local monitor, which is either
/// Open Access (no tag) or Exclusive Access (a tag). The state is `Processor::exclusive_tag`.
///
pub trait Monitor {
    ///
    /// What a Store-Exclusive to `address` asks (`ExclusiveMonitorsPass`, B2.3.7): true if the
    /// monitor is in Exclusive Access and the address is in the tagged block, so that the store
    /// takes place. Whatever the answer, the monitor is Open Access after it (Table A3-2); a store
    /// to another address than the last load's is IMPLEMENTATION DEFINED, and here it fails.
    ///
    fn exclusive_monitors_pass(&mut self, address: u32) -> bool;

    ///
    /// A Load-Exclusive at `address`: the monitor tags it and goes to Exclusive Access
    /// (`SetExclusiveMonitors`, B2.3.7). The size does not matter, the tag is the word.
    ///
    fn set_exclusive_monitors(&mut self, address: u32);

    ///
    /// `ClearExclusiveLocal`: CLREX, exception entry and return, and reset
    ///
    fn clear_exclusive_monitor(&mut self);
}

impl Monitor for Processor {
    fn exclusive_monitors_pass(&mut self, address: u32) -> bool {
        let passed = self.exclusive_tag == Some(tag(address));
        self.exclusive_tag = None;
        passed
    }

    fn set_exclusive_monitors(&mut self, address: u32) {
        self.exclusive_tag = Some(tag(address));
    }

    fn clear_exclusive_monitor(&mut self) {
        self.exclusive_tag = None;
    }
}
