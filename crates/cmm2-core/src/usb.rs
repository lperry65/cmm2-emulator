//! The USB host port: what a device has to answer, and the socket it plugs into. The OTG
//! controller asks the device on the other end of the cable what the firmware would hear, one
//! transaction at a time. The port is a cheap handle: clones share the socket, so the
//! controller and the front end each keep one.

use std::cell::RefCell;
use std::rc::Rc;

use crate::clock::Clock;

/// What a device answers to an IN token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum In {
    /// A data packet, at most the endpoint's packet size; empty is a zero-length packet.
    Data(Vec<u8>),
    Nak,
    Stall,
}

/// What a device answers to a data packet sent to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Out {
    Ack,
    Nak,
    Stall,
}

/// A low-speed device, as the bus sees it: transactions in, handshakes and data out. The
/// controller does the framing, the packet sizes and the data toggles, so a device only
/// has to know its own protocol.
pub trait UsbDevice {
    /// The address the host gave it, 0 until then.
    fn address(&self) -> u8;

    /// A bus reset: the device forgets its address and configuration.
    fn reset(&mut self);

    /// A SETUP packet to endpoint 0. A device always acknowledges it, and stalls the stages
    /// that follow if it does not like the request.
    fn setup(&mut self, packet: [u8; 8]);

    fn out(&mut self, endpoint: u8, data: &[u8]) -> Out;

    /// `max` is the packet size the host asked for; the device never sends more.
    fn in_token(&mut self, endpoint: u8, max: usize) -> In;
}

struct Socket {
    device: Option<Box<dyn UsbDevice>>,
    /// How many times a device has been plugged in or pulled out.
    plugs: u32,
    clock: Clock,
}

#[derive(Clone)]
pub struct UsbPort(Rc<RefCell<Socket>>);

impl UsbPort {
    pub fn new(clock: Clock) -> UsbPort {
        UsbPort(Rc::new(RefCell::new(Socket {
            device: None,
            plugs: 0,
            clock,
        })))
    }

    /// Plugs `device` in, in place of any device that was there (which counts as pulling it
    /// out first).
    pub fn attach(&self, device: impl UsbDevice + 'static) {
        self.plug(Some(Box::new(device)));
    }

    /// Pulls the device out.
    pub fn detach(&self) {
        if self.attached() {
            self.plug(None);
        }
    }

    pub fn attached(&self) -> bool {
        self.0.borrow().device.is_some()
    }

    // The controller has to hear of a change at once, though nothing it does is running.
    fn plug(&self, device: Option<Box<dyn UsbDevice>>) {
        let mut socket = self.0.borrow_mut();
        socket.device = device;
        socket.plugs += 1;
        socket.clock.wake_at(socket.clock.now());
    }

    /// Changes with every plug and unplug, so the controller can tell that the device it
    /// knew is gone, even if another one is there now.
    pub(crate) fn plugs(&self) -> u32 {
        self.0.borrow().plugs
    }

    /// Runs `f` on the device, if there is one.
    pub(crate) fn with_device<R>(&self, f: impl FnOnce(&mut dyn UsbDevice) -> R) -> Option<R> {
        let mut socket = self.0.borrow_mut();
        socket.device.as_mut().map(|device| f(&mut **device))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Nothing;

    impl UsbDevice for Nothing {
        fn address(&self) -> u8 {
            7
        }
        fn reset(&mut self) {}
        fn setup(&mut self, _packet: [u8; 8]) {}
        fn out(&mut self, _endpoint: u8, _data: &[u8]) -> Out {
            Out::Ack
        }
        fn in_token(&mut self, _endpoint: u8, _max: usize) -> In {
            In::Nak
        }
    }

    #[test]
    fn a_new_port_has_nothing_attached() {
        let port = UsbPort::new(Clock::new());
        assert!(!port.attached());
        assert_eq!(port.with_device(|d| d.address()), None);
    }

    #[test]
    fn attach_and_detach_change_the_plug_count() {
        let port = UsbPort::new(Clock::new());
        port.attach(Nothing);
        assert!(port.attached());
        assert_eq!(port.plugs(), 1);
        assert_eq!(port.with_device(|d| d.address()), Some(7));
        port.detach();
        assert!(!port.attached());
        assert_eq!(port.plugs(), 2);
    }

    #[test]
    fn detaching_an_empty_port_changes_nothing() {
        let port = UsbPort::new(Clock::new());
        port.detach();
        assert_eq!(port.plugs(), 0);
    }

    #[test]
    fn attaching_over_a_device_counts_as_a_change() {
        let port = UsbPort::new(Clock::new());
        port.attach(Nothing);
        port.attach(Nothing);
        assert_eq!(port.plugs(), 2);
    }

    #[test]
    fn a_plug_asks_the_clock_for_a_tick_at_once() {
        let clock = Clock::new();
        let port = UsbPort::new(clock.clone());
        clock.set(1_000);
        assert!(!clock.take_due());
        port.attach(Nothing);
        assert!(clock.take_due());
        port.detach();
        assert!(clock.take_due());
    }

    #[test]
    fn clones_share_the_socket() {
        let port = UsbPort::new(Clock::new());
        let other = port.clone();
        other.attach(Nothing);
        assert!(port.attached());
    }
}
