//! Single-writer worker: SPSC ingress + exclusive [`HpEngine`].
//!
//! Practices: LMAX single writer; Aeron-style [`WaitStrategy`] for idle loops.

use crate::engine::HpEngine;
use crate::spsc::{Busy, SpscRing};
use crate::types::{HpCommand, HpEvent};
use std::thread;

/// How to wait when the ingress ring is empty (Aeron IdleStrategy analogue).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitStrategy {
    /// Spin in userspace — lowest latency; use on an isolated core.
    BusySpin,
    /// `thread::yield_now` — friendlier when sharing a core.
    Yield,
}

/// Symbol worker: producer submits via [`try_submit`]; consumer runs [`run_once`] / [`poll`].
pub struct HpWorker {
    ring: SpscRing,
    engine: HpEngine,
    /// Scratch batch drained from the ring (reused — Disruptor-style preallocate).
    batch: Vec<HpCommand>,
}

impl HpWorker {
    pub fn new(ring_cap: usize) -> Self {
        Self {
            ring: SpscRing::with_capacity(ring_cap),
            engine: HpEngine::with_capacity(ring_cap, 64),
            batch: Vec::with_capacity(64),
        }
    }

    pub fn engine(&self) -> &HpEngine {
        &self.engine
    }

    pub fn engine_mut(&mut self) -> &mut HpEngine {
        &mut self.engine
    }

    pub fn ring_len_approx(&self) -> usize {
        self.ring.len_approx()
    }

    /// Producer side: enqueue a command. [`Busy`] if the ring is full.
    pub fn try_submit(&self, cmd: HpCommand) -> Result<(), Busy> {
        self.ring.try_push(cmd)
    }

    /// Consumer side: drain available commands into the engine once.
    /// Returns the number of [`HpEvent::Fill`] events produced this call.
    pub fn run_once(&mut self) -> usize {
        self.batch.clear();
        self.ring.pop_n(&mut self.batch, self.ring.capacity());
        self.drain_batch()
    }

    /// Poll until `max_idle_spins` consecutive empty drains.
    /// Returns total fills observed. Pass `None` only for dedicated threads (busy forever).
    pub fn poll(&mut self, wait: WaitStrategy, max_idle_spins: Option<usize>) -> usize {
        let mut total_fills = 0usize;
        let mut idle = 0usize;
        loop {
            self.batch.clear();
            let n = self.ring.pop_n(&mut self.batch, self.ring.capacity());
            if n == 0 {
                idle = idle.saturating_add(1);
                if Self::idle_limit_reached(idle, max_idle_spins) {
                    break;
                }
                match wait {
                    WaitStrategy::BusySpin => std::hint::spin_loop(),
                    WaitStrategy::Yield => thread::yield_now(),
                }
                continue;
            }
            idle = 0;
            total_fills += self.drain_batch();
        }
        total_fills
    }

    /// Whether idle polling should stop.
    ///
    /// `max_idle_spins == None` means a dedicated thread never stops on idle
    /// (caller must only use this when another task will keep feeding work or the
    /// process is shutting down).
    pub(crate) fn idle_limit_reached(idle: usize, max_idle_spins: Option<usize>) -> bool {
        match max_idle_spins {
            Some(max) => idle >= max,
            // Dedicated-thread mode: never treat idle as terminal.
            None => false,
        }
    }

    fn drain_batch(&mut self) -> usize {
        let mut fills = 0usize;
        for cmd in self.batch.drain(..) {
            let events = self.engine.on_order(cmd);
            fills += events
                .iter()
                .filter(|e| matches!(e, HpEvent::Fill { .. }))
                .count();
        }
        fills
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn idle_limit_covers_bounded_and_dedicated() {
        assert!(HpWorker::idle_limit_reached(3, Some(3)));
        assert!(!HpWorker::idle_limit_reached(2, Some(3)));
        assert!(!HpWorker::idle_limit_reached(1_000, None));
    }
}
