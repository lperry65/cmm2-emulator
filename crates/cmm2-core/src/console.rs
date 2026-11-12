use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use crate::irq::Irqs;

/// The text channel between the guest's console USART and the host. A cheap handle: clones
/// share the queues, so the USART model and the front end each keep one.
#[derive(Clone, Debug, Default)]
pub struct Console(Rc<RefCell<State>>);

#[derive(Debug, Default)]
struct State {
    output: Vec<u8>,
    input: VecDeque<u8>,
    // The line to raise when input arrives, once the USART has attached it, and whether the
    // USART has the receive interrupt enabled.
    interrupt: Option<(Irqs, u16)>,
    rx_interrupt: bool,
}

impl Console {
    pub fn new() -> Console {
        Console::default()
    }

    /// The bytes the guest has sent since the last call, in order.
    pub fn take_output(&self) -> Vec<u8> {
        std::mem::take(&mut self.0.borrow_mut().output)
    }

    /// Queues bytes for the guest to receive. If that gives a USART with its receive
    /// interrupt enabled something to read, the interrupt is raised.
    pub fn push_input(&self, bytes: &[u8]) {
        let line = {
            let mut state = self.0.borrow_mut();
            let was_empty = state.input.is_empty();
            state.input.extend(bytes);
            if was_empty && !state.input.is_empty() && state.rx_interrupt {
                state.interrupt.clone()
            } else {
                None
            }
        };
        // Raised after the borrow ends, so nothing that reacts can find the console locked.
        if let Some((irqs, line)) = line {
            irqs.raise(line);
        }
    }

    pub(crate) fn attach_interrupt(&self, irqs: Irqs, line: u16) {
        self.0.borrow_mut().interrupt = Some((irqs, line));
    }

    pub(crate) fn set_rx_interrupt(&self, enabled: bool) {
        self.0.borrow_mut().rx_interrupt = enabled;
    }

    pub(crate) fn write(&self, byte: u8) {
        self.0.borrow_mut().output.push(byte);
    }

    pub(crate) fn read(&self) -> Option<u8> {
        self.0.borrow_mut().input.pop_front()
    }

    pub(crate) fn has_input(&self) -> bool {
        !self.0.borrow().input.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_comes_back_in_order_and_take_drains_it() {
        let console = Console::new();
        console.write(b'h');
        console.write(b'i');
        assert_eq!(console.take_output(), b"hi");
        assert!(console.take_output().is_empty());
        console.write(b'!');
        assert_eq!(console.take_output(), b"!");
    }

    #[test]
    fn input_is_first_in_first_out() {
        let console = Console::new();
        console.push_input(b"ab");
        console.push_input(b"c");
        assert_eq!(console.read(), Some(b'a'));
        assert_eq!(console.read(), Some(b'b'));
        assert_eq!(console.read(), Some(b'c'));
        assert_eq!(console.read(), None);
    }

    #[test]
    fn has_input_follows_the_queue() {
        let console = Console::new();
        assert!(!console.has_input());
        console.push_input(b"x");
        assert!(console.has_input());
        console.read();
        assert!(!console.has_input());
    }

    #[test]
    fn pushing_input_with_no_interrupt_attached_is_harmless() {
        let console = Console::new();
        console.set_rx_interrupt(true);
        console.push_input(b"x");
        console.push_input(b"");
        assert!(console.has_input());
    }

    #[test]
    fn clones_share_state() {
        let console = Console::new();
        let other = console.clone();
        other.write(b'a');
        console.push_input(b"b");
        assert_eq!(console.take_output(), b"a");
        assert_eq!(other.read(), Some(b'b'));
    }
}
