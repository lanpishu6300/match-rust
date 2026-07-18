use crate::level::Level;
use crate::level_index::LevelIndex;
use crate::order_store::OrderStore;
use crate::types::{HpOrder, Side};

#[cfg(feature = "art")]
use crate::art_index::{ArtAskIndex, ArtBidIndex};
#[cfg(not(feature = "art"))]
use crate::level_index::{BTreeAskIndex, BTreeBidIndex};

const LEVEL_POOL_CAP: usize = 256;

#[cfg(not(feature = "art"))]
type AskIndex = BTreeAskIndex;
#[cfg(not(feature = "art"))]
type BidIndex = BTreeBidIndex;

#[cfg(feature = "art")]
type AskIndex = ArtAskIndex;
#[cfg(feature = "art")]
type BidIndex = ArtBidIndex;

/// Price-level order book with FIFO within each tick.
#[derive(Debug)]
pub struct Book {
    bids: BidIndex,
    asks: AskIndex,
    store: OrderStore,
    best_bid_tick: Option<i64>,
    best_ask_tick: Option<i64>,
    level_pool: Vec<Level>,
}

impl Book {
    pub fn new() -> Self {
        Self {
            bids: BidIndex::default(),
            asks: AskIndex::default(),
            store: OrderStore::new(),
            best_bid_tick: None,
            best_ask_tick: None,
            level_pool: Vec::new(),
        }
    }

    pub fn with_capacity(order_cap: usize) -> Self {
        Self {
            bids: BidIndex::default(),
            asks: AskIndex::default(),
            store: OrderStore::with_capacity(order_cap),
            best_bid_tick: None,
            best_ask_tick: None,
            level_pool: Vec::new(),
        }
    }

    pub fn store(&self) -> &OrderStore {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut OrderStore {
        &mut self.store
    }

    pub fn insert_limit(&mut self, order: HpOrder) -> u64 {
        let side = order.side;
        let tick = order.price_tick;
        let lot = order.open_lot;
        let id = self.store.insert(order);
        self.level_mut(side, tick).push(id, lot);
        self.note_insert(side, tick);
        id
    }

    /// Place an already-stored order onto its price level (taker remainder).
    pub fn rest(&mut self, id: u64) {
        let (side, tick, lot) = {
            let order = self.store.get(id).expect("rest: order must exist in store");
            (order.side, order.price_tick, order.open_lot)
        };
        debug_assert!(lot > 0);
        self.level_mut(side, tick).push(id, lot);
        self.note_insert(side, tick);
    }

    pub fn cancel(&mut self, id: u64) -> bool {
        let Some(order) = self.store.remove(id) else {
            return false;
        };
        self.remove_from_level(order.side, order.price_tick, id, order.open_lot);
        true
    }

    /// Reduce open qty at the front of a level (after a fill). Removes order if depleted.
    pub fn fill_order(&mut self, id: u64, qty_lot: i64) -> Option<&HpOrder> {
        let order = self.store.get_mut(id)?;
        let side = order.side;
        let tick = order.price_tick;
        order.open_lot -= qty_lot;
        let remaining = order.open_lot;
        if remaining < 0 {
            order.open_lot = 0;
        }
        let level = self.level_mut(side, tick);
        level.total_lot -= qty_lot;
        if level.total_lot < 0 {
            level.total_lot = 0;
        }
        if remaining <= 0 {
            if level.ids.front().copied() == Some(id) {
                level.ids.pop_front();
            } else {
                level.ids.retain(|&x| x != id);
            }
            if level.ids.is_empty() {
                self.remove_empty_level(side, tick);
            }
            self.store.remove(id);
            None
        } else {
            self.store.get(id)
        }
    }

    pub fn best_ask(&self) -> Option<i64> {
        debug_assert_eq!(self.best_ask_tick, self.asks.best_tick());
        self.best_ask_tick
    }

    pub fn best_bid(&self) -> Option<i64> {
        debug_assert_eq!(self.best_bid_tick, self.bids.best_tick());
        self.best_bid_tick
    }

    pub fn front_id(&self, side: Side, tick: i64) -> Option<u64> {
        self.level(side, tick)?.ids.front().copied()
    }

    pub fn depth(&self, side: Side, n: usize) -> Vec<(i64, i64)> {
        match side {
            Side::Buy => self.bids.depth(n),
            Side::Sell => self.asks.depth(n),
        }
    }

    fn level(&self, side: Side, tick: i64) -> Option<&Level> {
        match side {
            Side::Buy => self.bids.get(tick),
            Side::Sell => self.asks.get(tick),
        }
    }

    fn level_mut(&mut self, side: Side, tick: i64) -> &mut Level {
        match side {
            Side::Buy => {
                if !self.bids.contains(tick) {
                    let lvl = self.take_level();
                    self.bids.insert(tick, lvl);
                }
                self.bids.get_mut(tick).expect("just inserted")
            }
            Side::Sell => {
                if !self.asks.contains(tick) {
                    let lvl = self.take_level();
                    self.asks.insert(tick, lvl);
                }
                self.asks.get_mut(tick).expect("just inserted")
            }
        }
    }

    fn take_level(&mut self) -> Level {
        self.level_pool.pop().unwrap_or_default()
    }

    fn recycle_level(&mut self, mut level: Level) {
        level.clear();
        if self.level_pool.len() < LEVEL_POOL_CAP {
            self.level_pool.push(level);
        }
    }

    fn note_insert(&mut self, side: Side, tick: i64) {
        match side {
            Side::Buy => {
                if self.best_bid_tick.map(|b| tick > b).unwrap_or(true) {
                    self.best_bid_tick = Some(tick);
                }
            }
            Side::Sell => {
                if self.best_ask_tick.map(|a| tick < a).unwrap_or(true) {
                    self.best_ask_tick = Some(tick);
                }
            }
        }
    }

    fn note_remove_level(&mut self, side: Side, tick: i64) {
        match side {
            Side::Buy => {
                if self.best_bid_tick == Some(tick) {
                    self.best_bid_tick = self.bids.best_tick();
                }
            }
            Side::Sell => {
                if self.best_ask_tick == Some(tick) {
                    self.best_ask_tick = self.asks.best_tick();
                }
            }
        }
    }

    fn remove_from_level(&mut self, side: Side, tick: i64, id: u64, open_lot: i64) {
        let empty = {
            let Some(level) = (match side {
                Side::Buy => self.bids.get_mut(tick),
                Side::Sell => self.asks.get_mut(tick),
            }) else {
                return;
            };
            level.ids.retain(|&x| x != id);
            level.total_lot -= open_lot;
            if level.total_lot < 0 {
                level.total_lot = 0;
            }
            level.ids.is_empty()
        };
        if empty {
            self.remove_empty_level(side, tick);
        }
    }

    fn remove_empty_level(&mut self, side: Side, tick: i64) {
        let removed = match side {
            Side::Buy => self.bids.remove(tick),
            Side::Sell => self.asks.remove(tick),
        };
        if let Some(level) = removed {
            self.recycle_level(level);
        }
        self.note_remove_level(side, tick);
    }
}

impl Default for Book {
    fn default() -> Self {
        Self::new()
    }
}
