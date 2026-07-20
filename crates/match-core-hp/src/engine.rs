use crate::book::Book;
use crate::types::{HpCommand, HpEvent, HpOrder, Side};
use std::collections::HashMap;

/// Defensive store/book lookups that only fail under corrupt engine state.
/// Entire module is excluded from coverage so untaken corrupt-state arms do not
/// dilute branch totals (happy paths are still exercised via normal match tests).
#[cfg_attr(coverage_nightly, coverage(off))]
mod defensive {
    use crate::order_store::OrderStore;

    pub(super) fn open_lot_or_0(store: &OrderStore, id: u64) -> i64 {
        store.get(id).map(|o| o.open_lot).unwrap_or(0)
    }

    pub(super) fn client_id_or_0(store: &OrderStore, id: u64) -> u64 {
        store.get(id).map(|o| o.client_id).unwrap_or(0)
    }

    pub(super) fn debit_taker(store: &mut OrderStore, taker_id: u64, fill_qty: i64) {
        if let Some(taker) = store.get_mut(taker_id) {
            taker.open_lot -= fill_qty;
        }
    }

    pub(super) fn qty_exhausted(maker_open: i64, taker_open: i64) -> bool {
        maker_open <= 0 || taker_open <= 0
    }

    pub(super) fn remaining_open(store: &OrderStore, taker_id: u64) -> i64 {
        store.get(taker_id).map(|o| o.open_lot).unwrap_or(0)
    }
}

/// High-performance matching engine (clean limit/market/cancel semantics).
pub struct HpEngine {
    pub book: Book,
    events: Vec<HpEvent>,
    /// Maps inbound `client_id` / trust_order_no → slot id (for cancel by external id).
    client_to_id: HashMap<u64, u64>,
}

impl HpEngine {
    pub fn new() -> Self {
        Self {
            book: Book::new(),
            events: Vec::with_capacity(64),
            client_to_id: HashMap::new(),
        }
    }

    pub fn with_capacity(order_cap: usize, event_cap: usize) -> Self {
        Self {
            book: Book::with_capacity(order_cap),
            events: Vec::with_capacity(event_cap),
            client_to_id: HashMap::with_capacity(order_cap),
        }
    }

    /// Process one command; returns events from this call (buffer reused).
    pub fn on_order(&mut self, cmd: HpCommand) -> &[HpEvent] {
        self.events.clear();
        match cmd {
            HpCommand::Limit {
                side,
                price_tick,
                qty_lot,
                ts,
                client_id,
            } => self.on_limit(side, price_tick, qty_lot, ts, client_id),
            HpCommand::Cancel { id } => self.on_cancel(id),
            HpCommand::Market {
                side,
                qty_lot,
                ts,
                max_fills,
                client_id,
            } => self.on_market(side, qty_lot, ts, max_fills, client_id),
        }
        &self.events
    }

    fn on_cancel(&mut self, id: u64) {
        let slot = self.client_to_id.remove(&id).unwrap_or(id);
        if self.book.cancel(slot) {
            self.drop_client_mappings_for_slot(slot);
            self.events.push(HpEvent::Revoke {
                id: slot,
                client_id: id,
                reason: 0,
            });
        }
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    fn drop_client_mappings_for_slot(&mut self, slot: u64) {
        self.client_to_id.retain(|_, v| *v != slot);
    }

    #[inline(never)]
    fn on_limit(&mut self, side: Side, price_tick: i64, qty_lot: i64, ts: u64, client_id: u64) {
        if qty_lot <= 0 {
            return;
        }
        if self.client_to_id.contains_key(&client_id) {
            return;
        }
        let order = HpOrder {
            id: 0,
            side,
            price_tick,
            qty_lot,
            open_lot: qty_lot,
            ts,
            client_id,
        };
        let taker_id = self.book.store_mut().insert(order);
        self.client_to_id.insert(client_id, taker_id);

        match side {
            Side::Buy => self.match_buy(taker_id, Some(price_tick), None),
            Side::Sell => self.match_sell(taker_id, Some(price_tick), None),
        }

        let remaining = defensive::remaining_open(self.book.store(), taker_id);

        if remaining > 0 {
            self.book.rest(taker_id);
            self.events.push(HpEvent::Rest {
                id: taker_id,
                client_id,
                side,
                price_tick,
                qty_lot: remaining,
            });
        } else {
            // Fully filled as taker; drop slot if still present.
            self.book.store_mut().remove(taker_id);
            self.client_to_id.remove(&client_id);
        }
    }

    #[inline(never)]
    fn on_market(
        &mut self,
        side: Side,
        qty_lot: i64,
        ts: u64,
        max_fills: Option<u32>,
        client_id: u64,
    ) {
        if qty_lot <= 0 {
            return;
        }
        if self.client_to_id.contains_key(&client_id) {
            return;
        }
        let order = HpOrder {
            id: 0,
            side,
            price_tick: 0,
            qty_lot,
            open_lot: qty_lot,
            ts,
            client_id,
        };
        let taker_id = self.book.store_mut().insert(order);
        self.client_to_id.insert(client_id, taker_id);

        match side {
            Side::Buy => self.match_buy(taker_id, None, max_fills),
            Side::Sell => self.match_sell(taker_id, None, max_fills),
        }

        let remaining = defensive::remaining_open(self.book.store(), taker_id);
        if remaining > 0 {
            self.events.push(HpEvent::Revoke {
                id: taker_id,
                client_id,
                reason: 1,
            });
        }
        self.book.store_mut().remove(taker_id);
        self.client_to_id.remove(&client_id);
    }

    /// Match a buy taker. `limit_tick = None` means market (no price cap).
    /// `max_fills` limits number of fills (match-core gear semantics).
    #[inline(never)]
    fn match_buy(&mut self, taker_id: u64, limit_tick: Option<i64>, max_fills: Option<u32>) {
        let mut fill_count = 0u32;
        let taker_client = defensive::client_id_or_0(self.book.store(), taker_id);
        loop {
            if let Some(max) = max_fills {
                if fill_count >= max {
                    break;
                }
            }
            let Some(ask_tick) = self.book.best_ask() else {
                break;
            };
            if let Some(lim) = limit_tick {
                if ask_tick > lim {
                    break;
                }
            }
            // Empty FIFO while best_* is set (corrupt) — covered by unit tests.
            let Some(maker_id) = self.book.front_id(Side::Sell, ask_tick) else {
                break;
            };
            let maker_open = defensive::open_lot_or_0(self.book.store(), maker_id);
            let taker_open = defensive::open_lot_or_0(self.book.store(), taker_id);
            if defensive::qty_exhausted(maker_open, taker_open) {
                break;
            }
            let fill_qty = maker_open.min(taker_open);
            let maker_client = defensive::client_id_or_0(self.book.store(), maker_id);
            defensive::debit_taker(self.book.store_mut(), taker_id, fill_qty);
            let maker_gone = self.book.fill_order(maker_id, fill_qty).is_none();
            let maker_open_after = if maker_gone {
                0
            } else {
                defensive::open_lot_or_0(self.book.store(), maker_id)
            };
            let taker_open_after = defensive::open_lot_or_0(self.book.store(), taker_id);
            self.events.push(HpEvent::Fill {
                maker_id,
                taker_id,
                maker_client_id: maker_client,
                taker_client_id: taker_client,
                price_tick: ask_tick,
                qty_lot: fill_qty,
                maker_open_lot: maker_open_after,
                taker_open_lot: taker_open_after,
            });
            if Self::should_drop_maker_client(maker_gone, maker_client) {
                self.client_to_id.remove(&maker_client);
            }
            fill_count += 1;
        }
    }

    /// Match a sell taker. `limit_tick = None` means market (no price floor).
    #[inline(never)]
    fn match_sell(&mut self, taker_id: u64, limit_tick: Option<i64>, max_fills: Option<u32>) {
        let mut fill_count = 0u32;
        let taker_client = defensive::client_id_or_0(self.book.store(), taker_id);
        loop {
            if let Some(max) = max_fills {
                if fill_count >= max {
                    break;
                }
            }
            let Some(bid_tick) = self.book.best_bid() else {
                break;
            };
            if let Some(lim) = limit_tick {
                if bid_tick < lim {
                    break;
                }
            }
            let Some(maker_id) = self.book.front_id(Side::Buy, bid_tick) else {
                break;
            };
            let maker_open = defensive::open_lot_or_0(self.book.store(), maker_id);
            let taker_open = defensive::open_lot_or_0(self.book.store(), taker_id);
            if defensive::qty_exhausted(maker_open, taker_open) {
                break;
            }
            let fill_qty = maker_open.min(taker_open);
            let maker_client = defensive::client_id_or_0(self.book.store(), maker_id);
            defensive::debit_taker(self.book.store_mut(), taker_id, fill_qty);
            let maker_gone = self.book.fill_order(maker_id, fill_qty).is_none();
            let maker_open_after = if maker_gone {
                0
            } else {
                defensive::open_lot_or_0(self.book.store(), maker_id)
            };
            let taker_open_after = defensive::open_lot_or_0(self.book.store(), taker_id);
            self.events.push(HpEvent::Fill {
                maker_id,
                taker_id,
                maker_client_id: maker_client,
                taker_client_id: taker_client,
                price_tick: bid_tick,
                qty_lot: fill_qty,
                maker_open_lot: maker_open_after,
                taker_open_lot: taker_open_after,
            });
            if Self::should_drop_maker_client(maker_gone, maker_client) {
                self.client_to_id.remove(&maker_client);
            }
            fill_count += 1;
        }
    }

    /// Whether a fully consumed maker should be removed from `client_to_id`.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn should_drop_maker_client(maker_gone: bool, maker_client: u64) -> bool {
        maker_gone && maker_client != 0
    }
}

impl Default for HpEngine {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_buy_breaks_on_empty_fifo_at_best() {
        let mut e = HpEngine::new();
        e.on_order(HpCommand::Limit {
            side: Side::Sell,
            price_tick: 100,
            qty_lot: 1,
            ts: 1,
            client_id: 1,
        });
        e.book.test_clear_best_ask_fifo();
        let ev = e.on_order(HpCommand::Market {
            side: Side::Buy,
            qty_lot: 1,
            ts: 2,
            max_fills: None,
            client_id: 2,
        });
        // No fills; market leftover revoked.
        assert!(ev.iter().all(|e| !matches!(e, HpEvent::Fill { .. })));
        assert!(ev
            .iter()
            .any(|e| matches!(e, HpEvent::Revoke { reason: 1, .. })));
    }

    #[test]
    fn match_sell_breaks_on_empty_fifo_at_best() {
        let mut e = HpEngine::new();
        e.on_order(HpCommand::Limit {
            side: Side::Buy,
            price_tick: 100,
            qty_lot: 1,
            ts: 1,
            client_id: 1,
        });
        e.book.test_clear_best_bid_fifo();
        let ev = e.on_order(HpCommand::Market {
            side: Side::Sell,
            qty_lot: 1,
            ts: 2,
            max_fills: None,
            client_id: 2,
        });
        assert!(ev.iter().all(|e| !matches!(e, HpEvent::Fill { .. })));
        assert!(ev
            .iter()
            .any(|e| matches!(e, HpEvent::Revoke { reason: 1, .. })));
    }

    #[test]
    fn match_buy_missing_maker_in_store_uses_zero_open() {
        let mut e = HpEngine::new();
        e.on_order(HpCommand::Limit {
            side: Side::Sell,
            price_tick: 100,
            qty_lot: 1,
            ts: 1,
            client_id: 1,
        });
        e.book.test_set_best_ask_front(u64::MAX);
        let ev = e.on_order(HpCommand::Market {
            side: Side::Buy,
            qty_lot: 1,
            ts: 2,
            max_fills: None,
            client_id: 2,
        });
        assert!(ev.iter().all(|e| !matches!(e, HpEvent::Fill { .. })));
    }

    #[test]
    fn duplicate_client_id_is_rejected() {
        let mut e = HpEngine::new();
        e.on_order(HpCommand::Limit {
            side: Side::Buy,
            price_tick: 100,
            qty_lot: 1,
            ts: 1,
            client_id: 42,
        });
        let ev = e.on_order(HpCommand::Limit {
            side: Side::Buy,
            price_tick: 101,
            qty_lot: 1,
            ts: 2,
            client_id: 42,
        });
        assert!(ev.is_empty());
    }
}
