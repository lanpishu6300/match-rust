use match_core_hp::{HpCommand, HpEngine, HpEvent, Side};

#[test]
fn market_buy_walks_asks_until_qty_done() {
    let mut e = HpEngine::new();
    e.on_order(HpCommand::Limit {
        side: Side::Sell,
        price_tick: 100,
        qty_lot: 2,
        ts: 1,
        client_id: 1,
    });
    e.on_order(HpCommand::Limit {
        side: Side::Sell,
        price_tick: 101,
        qty_lot: 3,
        ts: 2,
        client_id: 2,
    });
    let ev = e.on_order(HpCommand::Market {
        side: Side::Buy,
        qty_lot: 4,
        ts: 3,
        max_fills: None,
        client_id: 3,
    });
    assert_eq!(ev.len(), 2);
    assert!(matches!(
        ev[0],
        HpEvent::Fill {
            price_tick: 100,
            qty_lot: 2,
            ..
        }
    ));
    assert!(matches!(
        ev[1],
        HpEvent::Fill {
            price_tick: 101,
            qty_lot: 2,
            ..
        }
    ));
    // One lot left on ask 101; market never rests.
    assert_eq!(e.book.best_ask(), Some(101));
    assert_eq!(e.book.depth(Side::Sell, 1), vec![(101, 1)]);
    assert!(e.book.best_bid().is_none());
}

#[test]
fn market_buy_stops_when_book_empty() {
    let mut e = HpEngine::new();
    e.on_order(HpCommand::Limit {
        side: Side::Sell,
        price_tick: 100,
        qty_lot: 1,
        ts: 1,
        client_id: 1,
    });
    let ev = e.on_order(HpCommand::Market {
        side: Side::Buy,
        qty_lot: 10,
        ts: 2,
        max_fills: None,
        client_id: 2,
    });
    assert_eq!(ev.len(), 2);
    assert!(matches!(
        ev[0],
        HpEvent::Fill {
            qty_lot: 1,
            price_tick: 100,
            ..
        }
    ));
    assert!(matches!(
        ev[1],
        HpEvent::Revoke {
            client_id: 2,
            reason: 1,
            ..
        }
    ));
    assert!(e.book.best_ask().is_none());
}

#[test]
fn market_respects_max_fills() {
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
    e.on_order(HpCommand::Limit {
        side: Side::Sell,
        price_tick: 102,
        qty_lot: 1,
        ts: 3,
        client_id: 3,
    });
    let ev = e.on_order(HpCommand::Market {
        side: Side::Buy,
        qty_lot: 10,
        ts: 4,
        max_fills: Some(2),
        client_id: 4,
    });
    assert_eq!(ev.len(), 3);
    assert!(matches!(
        ev[0],
        HpEvent::Fill {
            price_tick: 100,
            ..
        }
    ));
    assert!(matches!(
        ev[1],
        HpEvent::Fill {
            price_tick: 101,
            ..
        }
    ));
    assert!(matches!(
        ev[2],
        HpEvent::Revoke {
            client_id: 4,
            reason: 1,
            ..
        }
    ));
    assert_eq!(e.book.best_ask(), Some(102));
}

#[test]
fn market_sell_walks_bids() {
    let mut e = HpEngine::new();
    e.on_order(HpCommand::Limit {
        side: Side::Buy,
        price_tick: 100,
        qty_lot: 2,
        ts: 1,
        client_id: 1,
    });
    e.on_order(HpCommand::Limit {
        side: Side::Buy,
        price_tick: 99,
        qty_lot: 2,
        ts: 2,
        client_id: 2,
    });
    let ev = e.on_order(HpCommand::Market {
        side: Side::Sell,
        qty_lot: 3,
        ts: 3,
        max_fills: None,
        client_id: 3,
    });
    assert_eq!(ev.len(), 2);
    assert!(matches!(
        ev[0],
        HpEvent::Fill {
            price_tick: 100,
            qty_lot: 2,
            ..
        }
    ));
    assert!(matches!(
        ev[1],
        HpEvent::Fill {
            price_tick: 99,
            qty_lot: 1,
            ..
        }
    ));
    assert_eq!(e.book.depth(Side::Buy, 1), vec![(99, 1)]);
}

#[test]
fn depth_two_bids_same_tick_aggregate() {
    let mut e = HpEngine::new();
    e.on_order(HpCommand::Limit {
        side: Side::Buy,
        price_tick: 100,
        qty_lot: 5,
        ts: 1,
        client_id: 1,
    });
    e.on_order(HpCommand::Limit {
        side: Side::Buy,
        price_tick: 100,
        qty_lot: 3,
        ts: 2,
        client_id: 2,
    });
    assert_eq!(e.book.depth(Side::Buy, 5), vec![(100, 8)]);
}
