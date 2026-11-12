use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// A change to the flash that a peripheral asks for. The flash lives in the CPU core, so the
/// flash controller cannot make it itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashOp {
    /// Sets `len` bytes from the address `base` to 0xFF.
    Erase { base: u32, len: u32 },
}

/// The flash operations the controller has queued for the CPU. A cheap handle: clones share
/// the same queue, so the controller keeps one and the CPU another.
#[derive(Clone, Default)]
pub struct FlashOps {
    shared: Rc<Shared>,
}

#[derive(Default)]
struct Shared {
    any: Cell<bool>,
    queue: RefCell<Vec<FlashOp>>,
}

impl FlashOps {
    pub fn new() -> FlashOps {
        FlashOps::default()
    }

    /// Queues the erase of `len` bytes from `base`. The CPU does it after the instruction
    /// that is running.
    pub fn erase(&self, base: u32, len: u32) {
        self.shared
            .queue
            .borrow_mut()
            .push(FlashOp::Erase { base, len });
        self.shared.any.set(true);
    }

    /// Returns and clears the queue, oldest first.
    pub(crate) fn take(&self) -> Option<Vec<FlashOp>> {
        if !self.shared.any.replace(false) {
            return None;
        }
        Some(std::mem::take(&mut *self.shared.queue.borrow_mut()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erase_queues_and_take_clears() {
        let ops = FlashOps::new();
        ops.erase(0x0810_0000, 0x2_0000);
        ops.erase(0x0800_0000, 0x10_0000);
        assert_eq!(
            ops.take(),
            Some(vec![
                FlashOp::Erase {
                    base: 0x0810_0000,
                    len: 0x2_0000
                },
                FlashOp::Erase {
                    base: 0x0800_0000,
                    len: 0x10_0000
                },
            ])
        );
        assert_eq!(ops.take(), None);
    }

    #[test]
    fn nothing_queued_takes_none() {
        assert_eq!(FlashOps::new().take(), None);
    }

    #[test]
    fn clones_share_the_queue() {
        let ops = FlashOps::new();
        ops.clone().erase(0x0800_0000, 4);
        assert_eq!(
            ops.take(),
            Some(vec![FlashOp::Erase {
                base: 0x0800_0000,
                len: 4
            }])
        );
    }
}
