use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

/// The text channel between the guest's console USART and the host. A cheap handle: clones
/// share the queues, so the USART model and the front end each keep one.
#[derive(Clone, Debug, Default)]
pub struct Console(Rc<RefCell<Queues>>);

#[derive(Debug, Default)]
struct Queues {
    output: Vec<u8>,
    input: VecDeque<u8>,
}

impl Console {
    pub fn new() -> Console {
        Console::default()
    }

    /// The bytes the guest has sent since the last call, in order.
    pub fn take_output(&self) -> Vec<u8> {
        std::mem::take(&mut self.0.borrow_mut().output)
    }

    /// Queues bytes for the guest to receive.
    pub fn push_input(&self, bytes: &[u8]) {
        self.0.borrow_mut().input.extend(bytes);
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
    fn clones_share_state() {
        let console = Console::new();
        let other = console.clone();
        other.write(b'a');
        console.push_input(b"b");
        assert_eq!(console.take_output(), b"a");
        assert_eq!(other.read(), Some(b'b'));
    }
}
