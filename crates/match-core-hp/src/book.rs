use crate::order_store::OrderStore;
use crate::types::{HpOrder, Side};
use std::collections::{BTreeMap, VecDeque};
use std::cmp::Reverse;

#[derive(Debug, Default)]
struct Level {
    total_lot: i64,
    /// FIFO order ids at this price.
    ids: VecDeque<u64>,
}

/// Price-level order book with FIFO within each tick.
#[derive(Debug)]
pub struct Book {
    bids: BTreeMap<Reverse<i64>, Level>,
    asks: BTreeMap<i64, Level>,
    store: OrderStore,
}

impl Book {
    pub fn new() -> Self {
        Self {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            store: OrderStore::new(),
        }
    }

    pub fn with_capacity(order_cap: usize) -> Self {
        Self {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            store: OrderStore::with_capacity(order_cap),
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
            // Should not happen under correct matching; clamp for safety.
            order.open_lot = 0;
        }
        let level = self.level_mut(side, tick);
        level.total_lot -= qty_lot;
        if level.total_lot < 0 {
            level.total_lot = 0;
        }
        if remaining <= 0 {
            // Pop front if it is this id (expected during match walk).
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
        self.asks.keys().next().copied()
    }

    pub fn best_bid(&self) -> Option<i64> {
        self.bids.keys().next().map(|Reverse(t)| *t)
    }

    pub fn front_id(&self, side: Side, tick: i64) -> Option<u64> {
        self.level(side, tick)?.ids.front().copied()
    }

    pub fn depth(&self, side: Side, n: usize) -> Vec<(i64, i64)> {
        match side {
            Side::Buy => self
                .bids
                .iter()
                .take(n)
                .map(|(Reverse(t), lvl)| (*t, lvl.total_lot))
                .collect(),
            Side::Sell => self
                .asks
                .iter()
                .take(n)
                .map(|(t, lvl)| (*t, lvl.total_lot))
                .collect(),
        }
    }

    fn level(&self, side: Side, tick: i64) -> Option<&Level> {
        match side {
            Side::Buy => self.bids.get(&Reverse(tick)),
            Side::Sell => self.asks.get(&tick),
        }
    }

    fn level_mut(&mut self, side: Side, tick: i64) -> &mut Level {
        match side {
            Side::Buy => self.bids.entry(Reverse(tick)).or_default(),
            Side::Sell => self.asks.entry(tick).or_default(),
        }
    }

    fn remove_from_level(&mut self, side: Side, tick: i64, id: u64, open_lot: i64) {
        let empty = {
            let Some(level) = (match side {
                Side::Buy => self.bids.get_mut(&Reverse(tick)),
                Side::Sell => self.asks.get_mut(&tick),
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
        match side {
            Side::Buy => {
                self.bids.remove(&Reverse(tick));
            }
            Side::Sell => {
                self.asks.remove(&tick);
            }
        }
    }
}

impl Level {
    fn push(&mut self, id: u64, lot: i64) {
        self.ids.push_back(id);
        self.total_lot += lot;
    }
}

impl Default for Book {
    fn default() -> Self {
        Self::new()
    }
}
