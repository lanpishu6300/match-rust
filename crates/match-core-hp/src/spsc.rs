//! Single-producer single-consumer command ring (power-of-two capacity).
//!
//! Practices (see `docs/best-practices.md`):
//! - LMAX Disruptor: preallocated slots, single producer / single consumer
//! - Aeron: isolate head/tail on separate cache lines (avoid false sharing)
//! - Disruptor batching: [`pop_n`] uses one Acquire + one Release for a batch

use crate::types::HpCommand;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Ring full — caller should back off / retry (explicit backpressure).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Busy;

/// Pad to a typical x86/ARM cache line so producer/consumer cursors do not share a line.
#[repr(align(64))]
struct CachePadded<T>(T);

/// Fixed-capacity SPSC queue of [`HpCommand`].
///
/// Safety contract: at most one producer thread calls [`try_push`], and at most
/// one consumer thread calls [`try_pop`] / [`pop_n`].
pub struct SpscRing {
    buf: Box<[UnsafeCell<HpCommand>]>,
    mask: usize,
    /// Next write index (producer-only updates).
    tail: CachePadded<AtomicUsize>,
    /// Next read index (consumer-only updates).
    head: CachePadded<AtomicUsize>,
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
            tail: CachePadded(AtomicUsize::new(0)),
            head: CachePadded(AtomicUsize::new(0)),
        }
    }

    pub fn capacity(&self) -> usize {
        self.mask + 1
    }

    /// Approximate depth (relaxed); for metrics only.
    pub fn len_approx(&self) -> usize {
        let tail = self.tail.0.load(Ordering::Relaxed);
        let head = self.head.0.load(Ordering::Relaxed);
        tail.wrapping_sub(head)
    }

    /// Producer: enqueue one command. Returns [`Busy`] if full.
    pub fn try_push(&self, cmd: HpCommand) -> Result<(), Busy> {
        let tail = self.tail.0.load(Ordering::Relaxed);
        let head = self.head.0.load(Ordering::Acquire);
        if tail.wrapping_sub(head) > self.mask {
            return Err(Busy);
        }
        unsafe {
            *self.buf[tail & self.mask].get() = cmd;
        }
        self.tail.0.store(tail.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Consumer: dequeue one command if available.
    pub fn try_pop(&self) -> Option<HpCommand> {
        let head = self.head.0.load(Ordering::Relaxed);
        let tail = self.tail.0.load(Ordering::Acquire);
        if head == tail {
            return None;
        }
        let cmd = unsafe { *self.buf[head & self.mask].get() };
        self.head.0.store(head.wrapping_add(1), Ordering::Release);
        Some(cmd)
    }

    /// Consumer: pop up to `max` commands into `out` in one batch (fewer barriers).
    pub fn pop_n(&self, out: &mut Vec<HpCommand>, max: usize) -> usize {
        if max == 0 {
            return 0;
        }
        let head = self.head.0.load(Ordering::Relaxed);
        let tail = self.tail.0.load(Ordering::Acquire);
        let available = tail.wrapping_sub(head);
        let n = available.min(max);
        if n == 0 {
            return 0;
        }
        out.reserve(n);
        for i in 0..n {
            let idx = (head.wrapping_add(i)) & self.mask;
            let cmd = unsafe { *self.buf[idx].get() };
            out.push(cmd);
        }
        self.head.0.store(head.wrapping_add(n), Ordering::Release);
        n
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::types::Side;

    #[test]
    fn batch_pop_preserves_order() {
        let r = SpscRing::with_capacity(8);
        for i in 0..5u64 {
            r.try_push(HpCommand::Limit {
                side: Side::Buy,
                price_tick: i as i64,
                qty_lot: 1,
                ts: i,
                client_id: i,
            })
            .unwrap();
        }
        let mut out = Vec::new();
        assert_eq!(r.pop_n(&mut out, 10), 5);
        for (i, c) in out.iter().enumerate() {
            if let HpCommand::Limit { client_id, .. } = c {
                assert_eq!(*client_id, i as u64);
            }
        }
    }

    #[test]
    fn try_pop_and_len_approx_edges() {
        let r = SpscRing::with_capacity(4);
        assert_eq!(r.len_approx(), 0);
        assert!(r.try_pop().is_none());

        r.try_push(HpCommand::Cancel { id: 1 }).unwrap();
        r.try_push(HpCommand::Cancel { id: 2 }).unwrap();
        assert_eq!(r.len_approx(), 2);

        assert_eq!(
            r.try_pop(),
            Some(HpCommand::Cancel { id: 1 })
        );
        assert_eq!(
            r.try_pop(),
            Some(HpCommand::Cancel { id: 2 })
        );
        assert!(r.try_pop().is_none());
        assert_eq!(r.len_approx(), 0);
    }

    #[test]
    fn pop_n_zero_max_is_no_op() {
        let r = SpscRing::with_capacity(4);
        r.try_push(HpCommand::Cancel { id: 1 }).unwrap();
        let mut out = Vec::new();
        assert_eq!(r.pop_n(&mut out, 0), 0);
        assert!(out.is_empty());
        assert_eq!(r.len_approx(), 1);
    }
}
