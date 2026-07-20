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
        match Side::from_order_type(order.order_type) {
            Some(Side::Buy) => self.buys.remove(&BuyEntry(order.clone())),
            Some(Side::Sell) => self.sells.remove(&SellEntry(order.clone())),
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
    pub fn remove_by_order_no(&mut self, side: Side, order_no: &str) -> Option<BbOrder> {
        match side {
            Side::Buy => {
                let key = self
                    .buys
                    .iter()
                    .find(|entry| entry.0.trust_order_no == order_no)?
                    .clone();
                // After find+clone of the same Ord key, BTreeSet::remove cannot fail
                // under BuyEntry Ord/Eq consistency — discard the bool.
                let _ = self.buys.remove(&key);
                Some(key.0)
            }
            Side::Sell => {
                let key = self
                    .sells
                    .iter()
                    .find(|entry| entry.0.trust_order_no == order_no)?
                    .clone();
                let _ = self.sells.remove(&key);
                Some(key.0)
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
    pub fn depth_levels(&self, side: Side, limit: usize) -> Vec<(BigDecimal, BigDecimal)> {
        match side {
            Side::Buy => depth_levels_from_orders(self.buys.iter().map(|e| e.0.clone()), limit),
            Side::Sell => depth_levels_from_orders(self.sells.iter().map(|e| e.0.clone()), limit),
        }
    }
}
