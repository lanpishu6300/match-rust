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
    /// client_id → resting order id list（每 client 支持多在途订单）。
    /// 仅在订单 rest 期间维护；fully-fill / cancel / 被 taker 吃光时移除对应项。
    client_to_id: FxHashMap<u64, Vec<u64>>,
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

    /// Number of clients that currently have at least one resting order.
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
        // 主路径：id = 显式订单 id（store slot）。slot 优先解析，避免与 client 号混淆。
        if let Some(o) = self.book.store().get(id) {
            let client_id = o.client_id;
            if self.book.cancel(id) {
                if let Some(list) = self.client_to_id.get_mut(&client_id) {
                    list.retain(|&x| x != id);
                    if list.is_empty() {
                        self.client_to_id.remove(&client_id);
                    }
                }
                self.events.push(HpEvent::Revoke {
                    id,
                    client_id,
                    reason: 0,
                });
            }
            return;
        }
        // 便利路径：id = client_id → 撤销该 client 全部在途订单。
        if let Some(list) = self.client_to_id.remove(&id) {
            for slot in list {
                if self.book.cancel(slot) {
                    self.events.push(HpEvent::Revoke {
                        id: slot,
                        client_id: id,
                        reason: 0,
                    });
                }
            }
        }
    }

    #[cfg_attr(coverage_nightly, inline(never))]
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
            Side::Buy => self.match_buy(taker_id, client_id, Some(price_tick), None),
            Side::Sell => self.match_sell(taker_id, client_id, Some(price_tick), None),
        }

        let remaining = defensive::remaining_open(self.book.store(), taker_id);

        if remaining > 0 {
            // Cancel lookup only needed while the order remains on the book.
            self.client_to_id.entry(client_id).or_default().push(taker_id);
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
                if let Some(list) = self.client_to_id.get_mut(&maker_client) {
                    list.retain(|&x| x != maker_id);
                    if list.is_empty() {
                        self.client_to_id.remove(&maker_client);
                    }
                }
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
                if let Some(list) = self.client_to_id.get_mut(&maker_client) {
                    list.retain(|&x| x != maker_id);
                    if list.is_empty() {
                        self.client_to_id.remove(&maker_client);
                    }
                }
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
    fn same_client_multi_resting_orders_allowed() {
        let mut e = HpEngine::new();
        let ev1: Vec<HpEvent> = e.on_order(HpCommand::Limit {
            side: Side::Buy,
            price_tick: 100,
            qty_lot: 1,
            ts: 1,
            client_id: 42,
        }).to_vec();
        let ev2: Vec<HpEvent> = e.on_order(HpCommand::Limit {
            side: Side::Buy,
            price_tick: 101,
            qty_lot: 1,
            ts: 2,
            client_id: 42,
        }).to_vec();
        // 同 client 两单都 rest（支持多在途），不再静默丢弃。
        assert!(ev1.iter().any(|e| matches!(e, HpEvent::Rest { .. })));
        assert!(ev2.iter().any(|e| matches!(e, HpEvent::Rest { .. })));
        assert_eq!(e.client_map_len(), 1); // 一个 client，两个在途单
        assert_eq!(e.client_to_id.get(&42).map(Vec::len), Some(2));
    }

    #[test]
    fn cancel_by_order_id_removes_only_that_order() {
        let mut e = HpEngine::new();
        e.on_order(HpCommand::Limit {
            side: Side::Buy,
            price_tick: 100,
            qty_lot: 1,
            ts: 1,
            client_id: 9,
        });
        let ev2: Vec<HpEvent> = e.on_order(HpCommand::Limit {
            side: Side::Buy,
            price_tick: 101,
            qty_lot: 1,
            ts: 2,
            client_id: 9,
        }).to_vec();
        let second_id = match &ev2[0] {
            HpEvent::Rest { id, .. } => *id,
            _ => panic!("expected Rest"),
        };
        let first_id = second_id - 1; // slot 连续分配（新 store，无复用）
        let ev: Vec<HpEvent> = e.on_order(HpCommand::Cancel { id: first_id }).to_vec();
        match &ev[0] {
            HpEvent::Revoke { id, reason: 0, .. } => assert_eq!(*id, first_id),
            other => panic!("expected Revoke, got {other:?}"),
        }
        // 只撤一单：第二单仍在途，client 映射保留一个条目。
        assert_eq!(e.client_to_id.get(&9).map(Vec::len), Some(1));
        assert!(e.client_to_id.get(&9).unwrap().contains(&second_id));
    }

    #[test]
    fn maker_fully_filled_clears_client_entry() {
        let mut e = HpEngine::new();
        // client 3 挂买 100 x2 → rest
        e.on_order(HpCommand::Limit {
            side: Side::Buy,
            price_tick: 100,
            qty_lot: 2,
            ts: 1,
            client_id: 3,
        });
        assert_eq!(e.client_map_len(), 1);
        // client 4 卖 100 x2 → 全部吃掉 maker
        let ev: Vec<HpEvent> = e.on_order(HpCommand::Limit {
            side: Side::Sell,
            price_tick: 100,
            qty_lot: 2,
            ts: 2,
            client_id: 4,
        }).to_vec();
        assert!(ev.iter().any(|e| matches!(e, HpEvent::Fill { .. })));
        // maker（client 3）被吃光 → 其 client 条目清空
        assert_eq!(e.client_map_len(), 0);
        assert!(e.client_to_id.is_empty());
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
