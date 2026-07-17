//! Single-producer single-consumer command ring (power-of-two capacity).

use crate::types::HpCommand;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Ring full — caller should back off / retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Busy;

/// Fixed-capacity SPSC queue of [`HpCommand`].
///
/// Safety contract: at most one producer thread calls [`try_push`], and at most
/// one consumer thread calls [`try_pop`] / [`pop_n`].
pub struct SpscRing {
    buf: Box<[UnsafeCell<HpCommand>]>,
    mask: usize,
    /// Next write index (producer).
    tail: AtomicUsize,
    /// Next read index (consumer).
    head: AtomicUsize,
}

unsafe impl Send for SpscRing {}
unsafe impl Sync for SpscRing {}

impl SpscRing {
    /// Create a ring with capacity `cap` (rounded up to power of two, min 2).
    pub fn with_capacity(cap: usize) -> Self {
        let cap = cap.max(2).next_power_of_two();
        let mut buf = Vec::with_capacity(cap);
        for _ in 0..cap {
            buf.push(UnsafeCell::new(HpCommand::Cancel { id: 0 }));
        }
        Self {
            buf: buf.into_boxed_slice(),
            mask: cap - 1,
            tail: AtomicUsize::new(0),
            head: AtomicUsize::new(0),
        }
    }

    pub fn capacity(&self) -> usize {
        self.mask + 1
    }

    /// Producer: enqueue one command. Returns [`Busy`] if full.
    pub fn try_push(&self, cmd: HpCommand) -> Result<(), Busy> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail.wrapping_sub(head) > self.mask {
            return Err(Busy);
        }
        unsafe {
            *self.buf[tail & self.mask].get() = cmd;
        }
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Consumer: dequeue one command if available.
    pub fn try_pop(&self) -> Option<HpCommand> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head == tail {
            return None;
        }
        let cmd = unsafe { *self.buf[head & self.mask].get() };
        self.head.store(head.wrapping_add(1), Ordering::Release);
        Some(cmd)
    }

    /// Consumer: pop up to `max` commands into `out`. Returns count popped.
    pub fn pop_n(&self, out: &mut Vec<HpCommand>, max: usize) -> usize {
        let mut n = 0;
        while n < max {
            match self.try_pop() {
                Some(c) => {
                    out.push(c);
                    n += 1;
                }
                None => break,
            }
        }
        n
    }
}
