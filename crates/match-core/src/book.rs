use std::collections::BTreeSet;

use bigdecimal::BigDecimal;

use crate::depth::depth_levels_from_orders;
use crate::order::{compare_buy, compare_sell, BbOrder, Side};

#[derive(Debug, Clone)]
struct BuyEntry(BbOrder);

impl PartialEq for BuyEntry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for BuyEntry {}

impl PartialOrd for BuyEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BuyEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        compare_buy(&self.0, &other.0)
    }
}

#[derive(Debug, Clone)]
struct SellEntry(BbOrder);

impl PartialEq for SellEntry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for SellEntry {}

impl PartialOrd for SellEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SellEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        compare_sell(&self.0, &other.0)
    }
}

/// Price-time priority order book with separate buy and sell sides.
#[derive(Debug, Default)]
pub struct OrderBook {
    buys: BTreeSet<BuyEntry>,
    sells: BTreeSet<SellEntry>,
}

impl OrderBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts `order`. Returns `false` if the side is invalid or an order with the
    /// same `trust_order_no` at the same price already exists (BTreeSet Equal).
    pub fn insert(&mut self, order: BbOrder) -> bool {
        match Side::from_order_type(order.order_type) {
            Some(Side::Buy) => self.buys.insert(BuyEntry(order)),
            Some(Side::Sell) => self.sells.insert(SellEntry(order)),
            None => false,
        }
    }

    /// True if `order_no` is already resting on either side.
    pub fn contains_order_no(&self, order_no: &str) -> bool {
        self.buys.iter().any(|e| e.0.trust_order_no == order_no)
            || self.sells.iter().any(|e| e.0.trust_order_no == order_no)
    }

    pub fn remove(&mut self, order: &BbOrder) -> bool {
        let key = order.removal_key();
        match Side::from_order_type(order.order_type) {
            Some(Side::Buy) => self.buys.take(&BuyEntry(key)).is_some(),
            Some(Side::Sell) => self.sells.take(&SellEntry(key)).is_some(),
            None => false,
        }
    }

    pub fn best(&self, side: Side) -> Option<&BbOrder> {
        match side {
            Side::Buy => self.buys.first().map(|entry| &entry.0),
            Side::Sell => self.sells.first().map(|entry| &entry.0),
        }
    }

    pub fn first(&self, side: Side) -> Option<&BbOrder> {
        self.best(side)
    }

    /// Remove and return the best order on `side` (price-time first).
    pub fn pop_first(&mut self, side: Side) -> Option<BbOrder> {
        match side {
            Side::Buy => self.buys.pop_first().map(|entry| entry.0),
            Side::Sell => self.sells.pop_first().map(|entry| entry.0),
        }
    }

    /// Find and remove an order by `trust_order_no` (Java revoke lookup).
    ///
    /// Only the sort fields (`price`/`time`/`order_no`) are cloned into the removal
    /// key — the full BigDecimal payload of the resting order is moved out, not copied.
    pub fn remove_by_order_no(&mut self, side: Side, order_no: &str) -> Option<BbOrder> {
        match side {
            Side::Buy => {
                let found = self
                    .buys
                    .iter()
                    .find(|entry| entry.0.trust_order_no == order_no)?;
                let key = found.0.removal_key();
                // Ord-equal key ⇒ BTreeSet::take finds the same entry (O(log n)); the
                // entry's order is moved out. `None` is unreachable under Ord/Eq consistency.
                self.buys.take(&BuyEntry(key)).map(|entry| entry.0)
            }
            Side::Sell => {
                let found = self
                    .sells
                    .iter()
                    .find(|entry| entry.0.trust_order_no == order_no)?;
                let key = found.0.removal_key();
                self.sells.take(&SellEntry(key)).map(|entry| entry.0)
            }
        }
    }

    pub fn is_empty(&self, side: Side) -> bool {
        match side {
            Side::Buy => self.buys.is_empty(),
            Side::Sell => self.sells.is_empty(),
        }
    }

    /// Depth snapshot: up to `limit` price levels with qty aggregated per level.
    /// Borrows resting orders — no per-level deep copy of the order payload.
    pub fn depth_levels(&self, side: Side, limit: usize) -> Vec<(BigDecimal, BigDecimal)> {
        match side {
            Side::Buy => depth_levels_from_orders(self.buys.iter().map(|e| &e.0), limit),
            Side::Sell => depth_levels_from_orders(self.sells.iter().map(|e| &e.0), limit),
        }
    }

    /// Resting orders in book sort order (price-time priority).
    pub fn resting_orders(&self, side: Side) -> Vec<BbOrder> {
        match side {
            Side::Buy => self.buys.iter().map(|e| e.0.clone()).collect(),
            Side::Sell => self.sells.iter().map(|e| e.0.clone()).collect(),
        }
    }
}
