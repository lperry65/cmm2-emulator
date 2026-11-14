//! The root port: its status register, and the device connecting and disconnecting.

use super::regs::{
    DISCINT, HPRT_RW, HPRT_W1C, PCSTS, PENA, PENCHNG, PLSTS_LOW, PPWR, PRST, PSPD_LOW,
};
use super::{CONNECT_DELAY, ENABLE_DELAY, OtgRegisters};

impl OtgRegisters {
    // The port follows the device in the socket and the power: a device that is plugged in
    // to a port with power connects, one that is pulled out or has lost its power disconnects.
    pub(super) fn sync_port(&mut self) {
        let plugs = self.port.plugs();
        let replaced = plugs != self.seen_plugs;
        self.seen_plugs = plugs;
        let wanted = self.state.hprt & PPWR != 0 && self.port.attached();
        if self.state.hprt & PCSTS != 0 && (replaced || !wanted) {
            self.disconnect();
            // A device that has lost its power starts again when it gets it back.
            if !wanted {
                self.port.with_device(|device| device.reset());
            }
        }
        if !wanted {
            self.state.connect_at = None;
        } else if self.state.hprt & PCSTS == 0 && self.state.connect_at.is_none() {
            self.state.connect_at = Some(self.clock.now() + CONNECT_DELAY);
        }
    }

    fn disconnect(&mut self) {
        let state = &mut self.state;
        if state.hprt & PENA != 0 {
            state.hprt |= PENCHNG;
        }
        state.hprt &= !(PCSTS | PENA);
        state.latched |= DISCINT;
        state.enable_at = None;
        state.rx.clear();
        for channel in &mut state.channels {
            channel.due = None;
            channel.staged.clear();
        }
    }

    pub(super) fn hprt_read(&self) -> u32 {
        let hprt = self.state.hprt;
        if hprt & PCSTS == 0 {
            hprt
        } else {
            hprt | PLSTS_LOW | PSPD_LOW
        }
    }

    // `value` is what the firmware wrote, with the bytes it did not write cleared.
    pub(super) fn hprt_write(&mut self, value: u32, mask: u32) {
        let was = self.state.hprt;
        let mut hprt = was & !(value & HPRT_W1C);
        // Writing a 1 to PENA disables the port.
        hprt &= !(value & PENA);
        hprt = hprt & !(HPRT_RW & mask) | value & HPRT_RW;
        self.state.hprt = hprt;
        if was & PRST == 0 && hprt & PRST != 0 {
            // A reset disables the port, and resets the device.
            self.state.hprt &= !PENA;
            self.state.enable_at = None;
            self.port.with_device(|device| device.reset());
        } else if was & PRST != 0 && hprt & PRST == 0 && hprt & PCSTS != 0 {
            self.state.enable_at = Some(self.clock.now() + ENABLE_DELAY);
        }
    }
}
