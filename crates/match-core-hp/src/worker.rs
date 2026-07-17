//! Single-writer worker: SPSC ingress + exclusive [`HpEngine`].

use crate::engine::HpEngine;
use crate::spsc::{Busy, SpscRing};
use crate::types::{HpCommand, HpEvent};

/// Symbol worker: producer submits via [`try_submit`]; consumer runs [`run_once`].
pub struct HpWorker {
    ring: SpscRing,
    engine: HpEngine,
    /// Scratch batch drained from the ring (reused).
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

    /// Producer side: enqueue a command. [`Busy`] if the ring is full.
    pub fn try_submit(&self, cmd: HpCommand) -> Result<(), Busy> {
        self.ring.try_push(cmd)
    }

    /// Consumer side: drain available commands into the engine.
    /// Returns the number of [`HpEvent::Fill`] events produced this call.
    pub fn run_once(&mut self) -> usize {
        self.batch.clear();
        self.ring.pop_n(&mut self.batch, self.ring.capacity());
        let mut fills = 0usize;
        for cmd in self.batch.drain(..) {
            let events = self.engine.on_order(cmd);
            fills += events.iter().filter(|e| matches!(e, HpEvent::Fill { .. })).count();
        }
        fills
    }
}
