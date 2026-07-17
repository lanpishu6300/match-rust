use crate::book::Book;
use crate::types::{HpCommand, HpEvent, HpOrder, Side};

/// High-performance matching engine (clean limit/cancel semantics).
pub struct HpEngine {
    pub book: Book,
    events: Vec<HpEvent>,
}

impl HpEngine {
    pub fn new() -> Self {
        Self {
            book: Book::new(),
            events: Vec::with_capacity(64),
        }
    }

    pub fn with_capacity(order_cap: usize, event_cap: usize) -> Self {
        Self {
            book: Book::with_capacity(order_cap),
            events: Vec::with_capacity(event_cap),
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
            HpCommand::Market { .. } => {
                // Implemented in Task 5.
                unimplemented!("HpCommand::Market is not implemented yet");
            }
        }
        &self.events
    }

    fn on_cancel(&mut self, id: u64) {
        if self.book.cancel(id) {
            self.events.push(HpEvent::Revoke { id, reason: 0 });
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

        match side {
            Side::Buy => self.match_buy(taker_id, price_tick),
            Side::Sell => self.match_sell(taker_id, price_tick),
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
        }
    }

    fn match_buy(&mut self, taker_id: u64, limit_tick: i64) {
        loop {
            let Some(ask_tick) = self.book.best_ask() else {
                break;
            };
            if ask_tick > limit_tick {
                break;
            }
            let Some(maker_id) = self.book.front_id(Side::Sell, ask_tick) else {
                break;
            };
            let maker_open = self.book.store().get(maker_id).map(|o| o.open_lot).unwrap_or(0);
            let taker_open = self.book.store().get(taker_id).map(|o| o.open_lot).unwrap_or(0);
            if maker_open <= 0 || taker_open <= 0 {
                break;
            }
            let fill_qty = maker_open.min(taker_open);
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
            self.book.fill_order(maker_id, fill_qty);
        }
    }

    fn match_sell(&mut self, taker_id: u64, limit_tick: i64) {
        loop {
            let Some(bid_tick) = self.book.best_bid() else {
                break;
            };
            if bid_tick < limit_tick {
                break;
            }
            let Some(maker_id) = self.book.front_id(Side::Buy, bid_tick) else {
                break;
            };
            let maker_open = self.book.store().get(maker_id).map(|o| o.open_lot).unwrap_or(0);
            let taker_open = self.book.store().get(taker_id).map(|o| o.open_lot).unwrap_or(0);
            if maker_open <= 0 || taker_open <= 0 {
                break;
            }
            let fill_qty = maker_open.min(taker_open);
            self.events.push(HpEvent::Fill {
                maker_id,
                taker_id,
                price_tick: bid_tick,
                qty_lot: fill_qty,
            });
            if let Some(taker) = self.book.store_mut().get_mut(taker_id) {
                taker.open_lot -= fill_qty;
            }
            self.book.fill_order(maker_id, fill_qty);
        }
    }
}

impl Default for HpEngine {
    fn default() -> Self {
        Self::new()
    }
}
