//! Branch-coverage tests for engine, book, order_store, and worker edges.
//!
//! Defensive branches that are unreachable through the public API without corrupt
//! internal state (e.g. `Book::remove_from_level` when the level index is missing)
//! are called out inline where we intentionally do not add tests.

use match_core_hp::{Book, HpCommand, HpEngine, HpEvent, HpOrder, HpWorker, OrderStore, Side, WaitStrategy};

#[test]
fn engine_default_constructible() {
    assert!(HpEngine::default().book.best_bid().is_none());
    assert!(Book::default().best_ask().is_none());
}

#[test]
fn match_buy_stops_when_taker_qty_exhausted_with_book_left() {
    let mut e = HpEngine::new();
    e.on_order(HpCommand::Limit {
        side: Side::Sell,
        price_tick: 100,
        qty_lot: 5,
        ts: 1,
        client_id: 1,
    });
    let ev = e.on_order(HpCommand::Market {
        side: Side::Buy,
        qty_lot: 2,
        ts: 2,
        max_levels: None,
        client_id: 2,
    });
    assert_eq!(ev.len(), 1);
    assert_eq!(e.book.best_ask(), Some(100));
}

#[test]
fn match_sell_stops_when_maker_open_non_positive() {
    let mut e = HpEngine::new();
    e.on_order(HpCommand::Limit {
        side: Side::Buy,
        price_tick: 100,
        qty_lot: 1,
        ts: 1,
        client_id: 1,
    });
    let maker = e.book.front_id(Side::Buy, 100).unwrap();
    e.book.store_mut().get_mut(maker).unwrap().open_lot = 0;
    let ev = e.on_order(HpCommand::Limit {
        side: Side::Sell,
        price_tick: 100,
        qty_lot: 1,
        ts: 2,
        client_id: 2,
    });
    assert!(matches!(ev[0], HpEvent::Rest { .. }));
}

#[test]
fn limit_sell_crosses_when_bid_at_or_above_limit() {
    let mut e = HpEngine::new();
    e.on_order(HpCommand::Limit {
        side: Side::Buy,
        price_tick: 100,
        qty_lot: 1,
        ts: 1,
        client_id: 1,
    });
    let ev = e.on_order(HpCommand::Limit {
        side: Side::Sell,
        price_tick: 100,
        qty_lot: 1,
        ts: 2,
        client_id: 2,
    });
    assert!(matches!(ev[0], HpEvent::Fill { price_tick: 100, .. }));
}

#[test]
fn cancel_non_best_ask_keeps_best_cache() {
    let mut b = Book::new();
    let _best = b.insert_limit(HpOrder::limit(Side::Sell, 50, 1, 1));
    let worse = b.insert_limit(HpOrder::limit(Side::Sell, 60, 1, 2));
    assert_eq!(b.best_ask(), Some(50));
    assert!(b.cancel(worse));
    assert_eq!(b.best_ask(), Some(50));
}

#[test]
fn cancel_with_inflated_open_lot_clamps_level_total() {
    let mut b = Book::new();
    let id = b.insert_limit(HpOrder::limit(Side::Buy, 10, 2, 1));
    // Inflate stored open_lot above level aggregate to hit total_lot < 0 clamp on cancel.
    b.store_mut().get_mut(id).unwrap().open_lot = 99;
    assert!(b.cancel(id));
    assert_eq!(b.best_bid(), None);
}

#[test]
fn zero_qty_limit_and_market_are_no_ops() {
    let mut e = HpEngine::new();
    assert!(e
        .on_order(HpCommand::Limit {
            side: Side::Buy,
            price_tick: 100,
            qty_lot: 0,
            ts: 1,
            client_id: 1,
        })
        .is_empty());
    assert!(e
        .on_order(HpCommand::Market {
            side: Side::Buy,
            qty_lot: -1,
            ts: 2,
            max_levels: None,
            client_id: 2,
        })
        .is_empty());
    assert!(e.book.best_bid().is_none());
}

#[test]
fn limit_buy_below_best_ask_does_not_cross() {
    let mut e = HpEngine::new();
    e.on_order(HpCommand::Limit {
        side: Side::Sell,
        price_tick: 100,
        qty_lot: 1,
        ts: 1,
        client_id: 1,
    });
    let ev = e.on_order(HpCommand::Limit {
        side: Side::Buy,
        price_tick: 99,
        qty_lot: 1,
        ts: 2,
        client_id: 2,
    });
    assert!(matches!(ev[0], HpEvent::Rest { price_tick: 99, .. }));
    assert_eq!(e.book.best_ask(), Some(100));
    assert_eq!(e.book.best_bid(), Some(99));
}

#[test]
fn limit_sell_above_best_bid_does_not_cross() {
    let mut e = HpEngine::new();
    e.on_order(HpCommand::Limit {
        side: Side::Buy,
        price_tick: 100,
        qty_lot: 1,
        ts: 1,
        client_id: 1,
    });
    let ev = e.on_order(HpCommand::Limit {
        side: Side::Sell,
        price_tick: 101,
        qty_lot: 1,
        ts: 2,
        client_id: 2,
    });
    assert!(matches!(ev[0], HpEvent::Rest { price_tick: 101, .. }));
    assert_eq!(e.book.best_bid(), Some(100));
    assert_eq!(e.book.best_ask(), Some(101));
}

#[test]
fn cancel_unknown_id_is_silent() {
    let mut e = HpEngine::new();
    assert!(e.on_order(HpCommand::Cancel { id: 999 }).is_empty());
}

#[test]
fn cancel_by_client_id() {
    let mut e = HpEngine::new();
    let ev = e.on_order(HpCommand::Limit {
        side: Side::Buy,
        price_tick: 100,
        qty_lot: 1,
        ts: 1,
        client_id: 42,
    });
    let slot = match ev[0] {
        HpEvent::Rest { id, .. } => id,
        _ => panic!("expected Rest"),
    };
    let ev = e.on_order(HpCommand::Cancel { id: 42 });
    assert!(matches!(ev[0], HpEvent::Revoke { id: rid, .. } if rid == slot));
    assert!(e.book.best_bid().is_none());
}

#[test]
fn market_sell_respects_max_levels() {
    let mut e = HpEngine::new();
    for (i, tick) in [(1, 100i64), (2, 99), (3, 98)].into_iter() {
        e.on_order(HpCommand::Limit {
            side: Side::Buy,
            price_tick: tick,
            qty_lot: 1,
            ts: i,
            client_id: i,
        });
    }
    let ev = e.on_order(HpCommand::Market {
        side: Side::Sell,
        qty_lot: 10,
        ts: 4,
        max_levels: Some(2),
        client_id: 4,
    });
    assert_eq!(ev.len(), 2);
    assert_eq!(e.book.best_bid(), Some(98));
}

#[test]
fn fill_non_front_order_uses_retain_path() {
    let mut b = Book::new();
    let id1 = b.insert_limit(HpOrder::limit(Side::Buy, 100, 5, 1));
    let id2 = b.insert_limit(HpOrder::limit(Side::Buy, 100, 3, 2));
    let id3 = b.insert_limit(HpOrder::limit(Side::Buy, 100, 2, 3));
    assert_eq!(b.front_id(Side::Buy, 100), Some(id1));
    assert!(b.fill_order(id2, 3).is_none());
    assert_eq!(b.front_id(Side::Buy, 100), Some(id1));
    assert!(!b.store().contains(id2));
    assert!(b.store().contains(id3));
    let _ = id1;
}

#[test]
fn fill_order_overfill_clamps_defensive_totals() {
    let mut b = Book::new();
    let id = b.insert_limit(HpOrder::limit(Side::Sell, 50, 2, 1));
    assert!(b.fill_order(id, 5).is_none());
    assert_eq!(b.best_ask(), None);
    assert!(!b.store().contains(id));
}

#[test]
fn level_pool_stops_recycling_after_cap() {
    let mut b = Book::new();
    let mut ids = Vec::new();
    // LEVEL_POOL_CAP is 256; a few past cap exercises the discard path.
    for i in 0..260 {
        ids.push(b.insert_limit(HpOrder::limit(Side::Buy, 10_000 + i as i64, 1, i as u64)));
    }
    for id in ids {
        assert!(b.cancel(id));
    }
    assert_eq!(b.best_bid(), None);
    let id = b.insert_limit(HpOrder::limit(Side::Buy, 1, 1, 999));
    assert_eq!(b.best_bid(), Some(1));
    assert_eq!(b.front_id(Side::Buy, 1), Some(id));
}

#[test]
fn order_store_contains_and_reuses_free_list() {
    let mut s = OrderStore::with_capacity(4);
    let id1 = s.insert(HpOrder::limit(Side::Buy, 1, 1, 1));
    assert!(s.contains(id1));
    assert!(!s.contains(999));
    assert!(!s.contains(0));
    assert!(s.get(0).is_none());
    assert!(s.get_mut(0).is_none());
    assert!(s.remove(0).is_none());
    s.remove(id1);
    assert!(!s.contains(id1));
    assert!(s.get(id1).is_none()); // free slot
    let id2 = s.insert(HpOrder::limit(Side::Sell, 2, 2, 2));
    assert_eq!(id2, id1);
    assert!(s.contains(id2));
}

#[test]
fn worker_ring_len_and_busy_spin_poll() {
    let mut w = HpWorker::new(8);
    assert_eq!(w.ring_len_approx(), 0);
    w.try_submit(HpCommand::Limit {
        side: Side::Sell,
        price_tick: 10,
        qty_lot: 1,
        ts: 1,
        client_id: 1,
    })
    .unwrap();
    assert!(w.ring_len_approx() >= 1);
    w.try_submit(HpCommand::Limit {
        side: Side::Buy,
        price_tick: 10,
        qty_lot: 1,
        ts: 2,
        client_id: 2,
    })
    .unwrap();
    let fills = w.poll(WaitStrategy::BusySpin, Some(4));
    assert_eq!(fills, 1);
    assert_eq!(w.ring_len_approx(), 0);
}

#[test]
fn maker_with_zero_client_id_still_fills() {
    let mut e = HpEngine::new();
    e.on_order(HpCommand::Limit {
        side: Side::Sell,
        price_tick: 100,
        qty_lot: 1,
        ts: 1,
        client_id: 0,
    });
    let ev = e.on_order(HpCommand::Limit {
        side: Side::Buy,
        price_tick: 100,
        qty_lot: 1,
        ts: 2,
        client_id: 2,
    });
    assert!(matches!(ev[0], HpEvent::Fill { .. }));
}

#[test]
fn limit_sell_stops_matching_when_bid_below_limit() {
    let mut e = HpEngine::new();
    e.on_order(HpCommand::Limit {
        side: Side::Buy,
        price_tick: 100,
        qty_lot: 1,
        ts: 1,
        client_id: 1,
    });
    let ev = e.on_order(HpCommand::Limit {
        side: Side::Sell,
        price_tick: 102,
        qty_lot: 1,
        ts: 2,
        client_id: 2,
    });
    assert!(matches!(ev[0], HpEvent::Rest { price_tick: 102, .. }));
    assert_eq!(e.book.best_bid(), Some(100));
}

#[test]
fn poll_exits_immediately_on_zero_idle_budget() {
    let mut w = HpWorker::new(8);
    assert_eq!(w.poll(WaitStrategy::Yield, Some(0)), 0);
}

#[test]
fn partial_maker_fill_keeps_resting_order() {
    let mut e = HpEngine::new();
    e.on_order(HpCommand::Limit {
        side: Side::Sell,
        price_tick: 100,
        qty_lot: 5,
        ts: 1,
        client_id: 1,
    });
    let ev = e.on_order(HpCommand::Limit {
        side: Side::Buy,
        price_tick: 100,
        qty_lot: 3,
        ts: 2,
        client_id: 2,
    });
    assert_eq!(ev.len(), 1);
    assert_eq!(e.book.best_ask(), Some(100));
    assert_eq!(e.book.depth(Side::Sell, 1), vec![(100, 2)]);
}

#[test]
fn two_makers_same_level_single_taker() {
    let mut e = HpEngine::new();
    e.on_order(HpCommand::Limit {
        side: Side::Sell,
        price_tick: 100,
        qty_lot: 1,
        ts: 1,
        client_id: 1,
    });
    e.on_order(HpCommand::Limit {
        side: Side::Sell,
        price_tick: 100,
        qty_lot: 1,
        ts: 2,
        client_id: 2,
    });
    let ev = e.on_order(HpCommand::Limit {
        side: Side::Buy,
        price_tick: 100,
        qty_lot: 2,
        ts: 3,
        client_id: 3,
    });
    assert_eq!(ev.len(), 2);
    assert!(matches!(ev[0], HpEvent::Fill { .. }));
    assert!(matches!(ev[1], HpEvent::Fill { .. }));
}

#[test]
fn taker_exhausted_stops_match_loop() {
    let mut e = HpEngine::new();
    e.on_order(HpCommand::Limit {
        side: Side::Sell,
        price_tick: 100,
        qty_lot: 1,
        ts: 1,
        client_id: 1,
    });
    e.on_order(HpCommand::Limit {
        side: Side::Sell,
        price_tick: 101,
        qty_lot: 1,
        ts: 2,
        client_id: 2,
    });
    let ev = e.on_order(HpCommand::Limit {
        side: Side::Buy,
        price_tick: 101,
        qty_lot: 2,
        ts: 3,
        client_id: 3,
    });
    assert_eq!(ev.len(), 2);
    assert!(e.book.best_ask().is_none());
}

/// Empty FIFO while `best_ask` is set — must run as an integration test so the same
/// crate instantiation used by other integration binaries also covers the `front_id`
/// `None` arm (unit-test coverage alone leaves that arm missed in llvm-cov merge).
#[cfg(coverage)]
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
        max_levels: None,
        client_id: 2,
    });
    assert!(ev.is_empty());
}

#[cfg(coverage)]
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
        max_levels: None,
        client_id: 2,
    });
    assert!(ev.is_empty());
}

#[cfg(coverage)]
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
        max_levels: None,
        client_id: 2,
    });
    assert!(ev.is_empty());
}

// Still unreachable without further hooks: `Book::remove_from_level` when the level
// index entry is missing. `HpWorker::poll(..., None)` never breaks on idle.
