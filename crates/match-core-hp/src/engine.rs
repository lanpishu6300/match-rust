use crate::book::Book;
use crate::types::{HpCommand, HpEvent, HpOrder, Side};
use std::collections::HashMap;

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
                max_levels,
                client_id,
            } => self.on_market(side, qty_lot, ts, max_levels, client_id),
        }
        &self.events
    }

    fn on_cancel(&mut self, id: u64) {
        let slot = self.client_to_id.remove(&id).unwrap_or(id);
        if self.book.cancel(slot) {
            self.client_to_id.retain(|_, v| *v != slot);
            self.events.push(HpEvent::Revoke {
                id: slot,
                reason: 0,
            });
        }
    }

    fn on_limit(&mut self, side: Side, price_tick: i64, qty_lot: i64, ts: u64, client_id: u64) {
        if qty_lot <= 0 {
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

        let remaining = self
            .book
            .store()
            .get(taker_id)
            .map(|o| o.open_lot)
            .unwrap_or(0);

        if remaining > 0 {
            self.book.rest(taker_id);
            self.events.push(HpEvent::Rest {
                id: taker_id,
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

    fn on_market(
        &mut self,
        side: Side,
        qty_lot: i64,
        ts: u64,
        max_levels: Option<u32>,
        client_id: u64,
    ) {
        if qty_lot <= 0 {
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
            Side::Buy => self.match_buy(taker_id, None, max_levels),
            Side::Sell => self.match_sell(taker_id, None, max_levels),
        }

        // Market never rests; drop leftover taker slot.
        self.book.store_mut().remove(taker_id);
        self.client_to_id.remove(&client_id);
    }

    /// Match a buy taker. `limit_tick = None` means market (no price cap).
    fn match_buy(&mut self, taker_id: u64, limit_tick: Option<i64>, max_levels: Option<u32>) {
        let mut levels_seen = 0u32;
        let mut current_level: Option<i64> = None;
        loop {
            let Some(ask_tick) = self.book.best_ask() else {
                break;
            };
            if let Some(lim) = limit_tick {
                if ask_tick > lim {
                    break;
                }
            }
            if current_level != Some(ask_tick) {
                if let Some(max) = max_levels {
                    if levels_seen >= max {
                        break;
                    }
                }
                current_level = Some(ask_tick);
                levels_seen += 1;
            }
            let Some(maker_id) = self.book.front_id(Side::Sell, ask_tick) else {
                break;
            };
            let maker_open = self
                .book
                .store()
                .get(maker_id)
                .map(|o| o.open_lot)
                .unwrap_or(0);
            let taker_open = self
                .book
                .store()
                .get(taker_id)
                .map(|o| o.open_lot)
                .unwrap_or(0);
            if maker_open <= 0 || taker_open <= 0 {
                break;
            }
            let fill_qty = maker_open.min(taker_open);
            let maker_client = self
                .book
                .store()
                .get(maker_id)
                .map(|o| o.client_id)
                .unwrap_or(0);
            // Maker price.
            self.events.push(HpEvent::Fill {
                maker_id,
                taker_id,
                price_tick: ask_tick,
                qty_lot: fill_qty,
            });
            if let Some(taker) = self.book.store_mut().get_mut(taker_id) {
                taker.open_lot -= fill_qty;
            }
            if self.book.fill_order(maker_id, fill_qty).is_none() && maker_client != 0 {
                self.client_to_id.remove(&maker_client);
            }
        }
    }

    /// Match a sell taker. `limit_tick = None` means market (no price floor).
    fn match_sell(&mut self, taker_id: u64, limit_tick: Option<i64>, max_levels: Option<u32>) {
        let mut levels_seen = 0u32;
        let mut current_level: Option<i64> = None;
        loop {
            let Some(bid_tick) = self.book.best_bid() else {
                break;
            };
            if let Some(lim) = limit_tick {
                if bid_tick < lim {
                    break;
                }
            }
            if current_level != Some(bid_tick) {
                if let Some(max) = max_levels {
                    if levels_seen >= max {
                        break;
                    }
                }
                current_level = Some(bid_tick);
                levels_seen += 1;
            }
            let Some(maker_id) = self.book.front_id(Side::Buy, bid_tick) else {
                break;
            };
            let maker_open = self
                .book
                .store()
                .get(maker_id)
                .map(|o| o.open_lot)
                .unwrap_or(0);
            let taker_open = self
                .book
                .store()
                .get(taker_id)
                .map(|o| o.open_lot)
                .unwrap_or(0);
            if maker_open <= 0 || taker_open <= 0 {
                break;
            }
            let fill_qty = maker_open.min(taker_open);
            let maker_client = self
                .book
                .store()
                .get(maker_id)
                .map(|o| o.client_id)
                .unwrap_or(0);
            self.events.push(HpEvent::Fill {
                maker_id,
                taker_id,
                price_tick: bid_tick,
                qty_lot: fill_qty,
            });
            if let Some(taker) = self.book.store_mut().get_mut(taker_id) {
                taker.open_lot -= fill_qty;
            }
            if self.book.fill_order(maker_id, fill_qty).is_none() && maker_client != 0 {
                self.client_to_id.remove(&maker_client);
            }
        }
    }
}

impl Default for HpEngine {
    fn default() -> Self {
        Self::new()
    }
}
