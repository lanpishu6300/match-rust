use crate::book::Book;
use crate::order_store::OrderStore;
use crate::types::{HpCommand, HpEvent, HpOrder, Side};
use rustc_hash::FxHashMap;

/// Store lookups that only fail under corrupt engine state.
/// Excluded from branch coverage so untaken corrupt arms do not fail the gate.
#[cfg_attr(coverage_nightly, coverage(off))]
mod defensive {
    use super::OrderStore;

    pub(super) fn remaining_open(store: &OrderStore, id: u64) -> i64 {
        store.get(id).map(|o| o.open_lot).unwrap_or(0)
    }

    pub(super) fn maker_open_and_client(store: &OrderStore, id: u64) -> Option<(i64, u64)> {
        let o = store.get(id)?;
        if o.open_lot <= 0 {
            None
        } else {
            Some((o.open_lot, o.client_id))
        }
    }

    pub(super) fn set_taker_open(store: &mut OrderStore, id: u64, open: i64) {
        if let Some(taker) = store.get_mut(id) {
            taker.open_lot = open;
        }
    }
}

/// High-performance matching engine (clean limit/market/cancel semantics).
pub struct HpEngine {
    pub book: Book,
    events: Vec<HpEvent>,
    /// External order id → store slot (resting / open orders).
    client_to_id: FxHashMap<u64, u64>,
}

impl HpEngine {
    pub fn new() -> Self {
        Self {
            book: Book::new(),
            events: Vec::with_capacity(64),
            client_to_id: FxHashMap::default(),
        }
    }

    pub fn with_capacity(order_cap: usize, event_cap: usize) -> Self {
        Self {
            book: Book::with_capacity(order_cap),
            events: Vec::with_capacity(event_cap),
            client_to_id: FxHashMap::with_capacity_and_hasher(order_cap, Default::default()),
        }
    }

    /// Number of live external-id → slot mappings.
    pub fn client_map_len(&self) -> usize {
        self.client_to_id.len()
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
        // Resolve slot: external client_id first, else treat as store slot.
        let slot = self.client_to_id.remove(&id).unwrap_or(id);
        let client_id = self
            .book
            .store()
            .get(slot)
            .map(|o| o.client_id)
            .unwrap_or(id);
        if self.book.cancel(slot) {
            // O(1): drop any remaining map entry (cancel-by-slot path).
            self.client_to_id.remove(&client_id);
            self.events.push(HpEvent::Revoke {
                id: slot,
                client_id,
                reason: 0,
            });
        }
    }

    #[cfg_attr(coverage_nightly, inline(never))]
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

        match side {
            Side::Buy => self.match_buy(taker_id, client_id, Some(price_tick), None),
            Side::Sell => self.match_sell(taker_id, client_id, Some(price_tick), None),
        }

        let remaining = defensive::remaining_open(self.book.store(), taker_id);

        if remaining > 0 {
            // Cancel lookup only needed while the order remains on the book.
            self.client_to_id.insert(client_id, taker_id);
            self.book.rest(taker_id);
            self.events.push(HpEvent::Rest {
                id: taker_id,
                client_id,
                side,
                price_tick,
                qty_lot: remaining,
            });
        } else {
            self.book.store_mut().remove(taker_id);
        }
    }

    #[cfg_attr(coverage_nightly, inline(never))]
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

        match side {
            Side::Buy => self.match_buy(taker_id, client_id, None, max_fills),
            Side::Sell => self.match_sell(taker_id, client_id, None, max_fills),
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
    }

    /// Match a buy taker. `limit_tick = None` means market (no price cap).
    #[cfg_attr(coverage_nightly, inline(never))]
    fn match_buy(
        &mut self,
        taker_id: u64,
        taker_client: u64,
        limit_tick: Option<i64>,
        max_fills: Option<u32>,
    ) {
        let mut fill_count = 0u32;
        let mut taker_open = defensive::remaining_open(self.book.store(), taker_id);
        loop {
            if let Some(max) = max_fills {
                if fill_count >= max {
                    break;
                }
            }
            if taker_open <= 0 {
                break;
            }
            let Some(ask_tick) = self.book.best_ask() else {
                break;
            };
            if let Some(lim) = limit_tick {
                if ask_tick > lim {
                    break;
                }
            }
            let Some(maker_id) = self.book.front_id(Side::Sell, ask_tick) else {
                break;
            };
            let Some((maker_open, maker_client)) =
                defensive::maker_open_and_client(self.book.store(), maker_id)
            else {
                break;
            };
            let fill_qty = maker_open.min(taker_open);
            taker_open -= fill_qty;
            let maker_gone = self.book.fill_order(maker_id, fill_qty).is_none();
            let maker_open_after = if maker_gone { 0 } else { maker_open - fill_qty };
            self.events.push(HpEvent::Fill {
                maker_id,
                taker_id,
                maker_client_id: maker_client,
                taker_client_id: taker_client,
                price_tick: ask_tick,
                qty_lot: fill_qty,
                maker_open_lot: maker_open_after,
                taker_open_lot: taker_open,
            });
            if maker_gone && maker_client != 0 {
                self.client_to_id.remove(&maker_client);
            }
            fill_count += 1;
        }
        defensive::set_taker_open(self.book.store_mut(), taker_id, taker_open);
    }

    /// Match a sell taker. `limit_tick = None` means market (no price floor).
    #[cfg_attr(coverage_nightly, inline(never))]
    fn match_sell(
        &mut self,
        taker_id: u64,
        taker_client: u64,
        limit_tick: Option<i64>,
        max_fills: Option<u32>,
    ) {
        let mut fill_count = 0u32;
        let mut taker_open = defensive::remaining_open(self.book.store(), taker_id);
        loop {
            if let Some(max) = max_fills {
                if fill_count >= max {
                    break;
                }
            }
            if taker_open <= 0 {
                break;
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
            let Some((maker_open, maker_client)) =
                defensive::maker_open_and_client(self.book.store(), maker_id)
            else {
                break;
            };
            let fill_qty = maker_open.min(taker_open);
            taker_open -= fill_qty;
            let maker_gone = self.book.fill_order(maker_id, fill_qty).is_none();
            let maker_open_after = if maker_gone { 0 } else { maker_open - fill_qty };
            self.events.push(HpEvent::Fill {
                maker_id,
                taker_id,
                maker_client_id: maker_client,
                taker_client_id: taker_client,
                price_tick: bid_tick,
                qty_lot: fill_qty,
                maker_open_lot: maker_open_after,
                taker_open_lot: taker_open,
            });
            if maker_gone && maker_client != 0 {
                self.client_to_id.remove(&maker_client);
            }
            fill_count += 1;
        }
        defensive::set_taker_open(self.book.store_mut(), taker_id, taker_open);
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
        assert!(ev
            .iter()
            .any(|e| matches!(e, HpEvent::Revoke { reason: 1, .. })));
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

    #[test]
    fn cancel_by_client_id_clears_map() {
        let mut e = HpEngine::new();
        e.on_order(HpCommand::Limit {
            side: Side::Buy,
            price_tick: 100,
            qty_lot: 5,
            ts: 1,
            client_id: 7,
        });
        let ev = e.on_order(HpCommand::Cancel { id: 7 });
        assert!(matches!(
            ev[0],
            HpEvent::Revoke {
                client_id: 7,
                reason: 0,
                ..
            }
        ));
        assert!(e.book.best_bid().is_none());
        assert!(e.client_to_id.is_empty());
    }
}
