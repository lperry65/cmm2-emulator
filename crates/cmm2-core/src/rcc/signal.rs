//! A value the RCC works out and another peripheral runs by, shared between them. A cheap
//! handle: clones share the value, so the RCC keeps one and the peripheral another. Each change
//! is logged with the core cycle it was made at, for the peripheral to take, so that it sees
//! every one however they come (a stop and a restart at the same value too) and when. A change
//! also asks the clock for a tick at once, so the peripheral hears of it without the firmware
//! touching it.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::rc::Rc;

use crate::clock::Clock;

pub(crate) struct Signal<T>(Rc<Shared<T>>);

struct Shared<T> {
    clock: Clock,
    value: Cell<T>,
    changes: RefCell<Vec<(u64, T)>>,
}

impl<T> Clone for Signal<T> {
    fn clone(&self) -> Signal<T> {
        Signal(self.0.clone())
    }
}

impl<T: Copy + fmt::Debug> fmt::Debug for Signal<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Signal").field(&self.0.value.get()).finish()
    }
}

impl<T: Copy + PartialEq> Signal<T> {
    pub(crate) fn new(clock: Clock, value: T) -> Signal<T> {
        Signal(Rc::new(Shared {
            clock,
            value: Cell::new(value),
            changes: RefCell::new(Vec::new()),
        }))
    }

    pub(crate) fn get(&self) -> T {
        self.0.value.get()
    }

    /// The changes since the last call, oldest first, each as the core cycle and the new value.
    pub(crate) fn take_changes(&self) -> Vec<(u64, T)> {
        self.0.changes.take()
    }

    pub(crate) fn set(&self, value: T) {
        if value != self.0.value.get() {
            let now = self.0.clock.now();
            self.0.value.set(value);
            self.0.changes.borrow_mut().push((now, value));
            self.0.clock.wake_at(now);
        }
    }
}
