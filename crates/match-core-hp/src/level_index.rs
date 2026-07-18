//! Price-tick → [`Level`](crate::level::Level) indexes.

use crate::level::Level;
use std::cmp::Reverse;
use std::collections::BTreeMap;

/// Side-agnostic operations over an ordered price-level map.
pub(crate) trait LevelIndex {
    fn get(&self, tick: i64) -> Option<&Level>;
    fn get_mut(&mut self, tick: i64) -> Option<&mut Level>;
    fn contains(&self, tick: i64) -> bool;
    fn insert(&mut self, tick: i64, level: Level);
    fn remove(&mut self, tick: i64) -> Option<Level>;
    /// Best tick for this side (bid: max, ask: min).
    fn best_tick(&self) -> Option<i64>;
    /// Up to `n` levels in best-first order: `(tick, total_lot)`.
    fn depth(&self, n: usize) -> Vec<(i64, i64)>;
}

/// Ask side: ascending tick order (best = lowest).
#[derive(Debug, Default)]
#[cfg_attr(feature = "art", allow(dead_code))]
pub(crate) struct BTreeAskIndex {
    map: BTreeMap<i64, Level>,
}

impl LevelIndex for BTreeAskIndex {
    fn get(&self, tick: i64) -> Option<&Level> {
        self.map.get(&tick)
    }

    fn get_mut(&mut self, tick: i64) -> Option<&mut Level> {
        self.map.get_mut(&tick)
    }

    fn contains(&self, tick: i64) -> bool {
        self.map.contains_key(&tick)
    }

    fn insert(&mut self, tick: i64, level: Level) {
        self.map.insert(tick, level);
    }

    fn remove(&mut self, tick: i64) -> Option<Level> {
        self.map.remove(&tick)
    }

    fn best_tick(&self) -> Option<i64> {
        self.map.keys().next().copied()
    }

    fn depth(&self, n: usize) -> Vec<(i64, i64)> {
        self.map
            .iter()
            .take(n)
            .map(|(t, lvl)| (*t, lvl.total_lot))
            .collect()
    }
}

/// Bid side: descending tick order (best = highest).
#[derive(Debug, Default)]
#[cfg_attr(feature = "art", allow(dead_code))]
pub(crate) struct BTreeBidIndex {
    map: BTreeMap<Reverse<i64>, Level>,
}

impl LevelIndex for BTreeBidIndex {
    fn get(&self, tick: i64) -> Option<&Level> {
        self.map.get(&Reverse(tick))
    }

    fn get_mut(&mut self, tick: i64) -> Option<&mut Level> {
        self.map.get_mut(&Reverse(tick))
    }

    fn contains(&self, tick: i64) -> bool {
        self.map.contains_key(&Reverse(tick))
    }

    fn insert(&mut self, tick: i64, level: Level) {
        self.map.insert(Reverse(tick), level);
    }

    fn remove(&mut self, tick: i64) -> Option<Level> {
        self.map.remove(&Reverse(tick))
    }

    fn best_tick(&self) -> Option<i64> {
        self.map.keys().next().map(|Reverse(t)| *t)
    }

    fn depth(&self, n: usize) -> Vec<(i64, i64)> {
        self.map
            .iter()
            .take(n)
            .map(|(Reverse(t), lvl)| (*t, lvl.total_lot))
            .collect()
    }
}
