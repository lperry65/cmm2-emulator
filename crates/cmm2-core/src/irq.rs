use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// External interrupts the CPU core supports.
const LINES: u16 = 240;

/// Interrupt lines from peripherals to the CPU. A cheap handle: clones share the same
/// pending set, so a peripheral keeps one and the CPU another.
#[derive(Clone, Debug, Default)]
pub struct Irqs {
    shared: Rc<Shared>,
}

#[derive(Debug, Default)]
struct Shared {
    any: Cell<bool>,
    pending: RefCell<[u32; 8]>,
}

impl Irqs {
    pub fn new() -> Irqs {
        Irqs::default()
    }

    /// Pends external interrupt `n` (0 to 239). It runs once the CPU has it enabled.
    /// Lines are edge triggered: raising a pending line again changes nothing.
    pub fn raise(&self, n: u16) {
        assert!(n < LINES, "interrupt {n} is not one of the {LINES} lines");
        self.shared.pending.borrow_mut()[usize::from(n / 32)] |= 1 << (n % 32);
        self.shared.any.set(true);
    }

    /// Drops every pending bit.
    pub(crate) fn clear(&self) {
        self.shared.any.set(false);
        *self.shared.pending.borrow_mut() = [0; 8];
    }

    /// Returns and clears the pending bits, one word per 32 lines.
    pub(crate) fn take(&self) -> Option<[u32; 8]> {
        if !self.shared.any.replace(false) {
            return None;
        }
        Some(std::mem::take(&mut *self.shared.pending.borrow_mut()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raise_sets_bits_and_take_clears_them() {
        let irqs = Irqs::new();
        irqs.raise(0);
        irqs.raise(5);
        irqs.raise(33);
        assert_eq!(irqs.take(), Some([0b100001, 0b10, 0, 0, 0, 0, 0, 0]));
        assert_eq!(irqs.take(), None);
    }

    #[test]
    fn nothing_pending_takes_none() {
        assert_eq!(Irqs::new().take(), None);
    }

    #[test]
    fn highest_line_is_239() {
        let irqs = Irqs::new();
        irqs.raise(239);
        assert_eq!(irqs.take().unwrap()[7], 1 << 15);
    }

    #[test]
    #[should_panic]
    fn line_240_is_rejected() {
        Irqs::new().raise(240);
    }

    #[test]
    fn clones_share_state() {
        let irqs = Irqs::new();
        irqs.clone().raise(7);
        assert_eq!(irqs.take(), Some([1 << 7, 0, 0, 0, 0, 0, 0, 0]));
    }

    #[test]
    fn clear_drops_pending_bits() {
        let irqs = Irqs::new();
        irqs.raise(3);
        irqs.raise(200);
        irqs.clear();
        assert_eq!(irqs.take(), None);
        irqs.raise(4);
        assert_eq!(irqs.take(), Some([1 << 4, 0, 0, 0, 0, 0, 0, 0]));
    }
}
