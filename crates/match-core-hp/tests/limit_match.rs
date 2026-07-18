use match_core_hp::{HpCommand, HpEngine, HpEvent, Side};

#[test]
fn limit_buy_fills_resting_sell() {
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
        qty_lot: 5,
        ts: 2,
        client_id: 2,
    });
    assert!(matches!(
        ev[0],
        HpEvent::Fill {
            qty_lot: 5,
            price_tick: 100,
            ..
        }
    ));
    assert!(e.book.best_ask().is_none());
}

#[test]
fn price_time_older_maker_first() {
    let mut e = HpEngine::new();
    let rest1 = e.on_order(HpCommand::Limit {
        side: Side::Sell,
        price_tick: 100,
        qty_lot: 1,
        ts: 1,
        client_id: 1,
    });
    let maker_older = match rest1[0] {
        HpEvent::Rest { id, .. } => id,
        _ => panic!("expected Rest"),
    };
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
        qty_lot: 1,
        ts: 3,
        client_id: 3,
    });
    match ev[0] {
        HpEvent::Fill {
            maker_id,
            qty_lot,
            price_tick,
            ..
        } => {
            assert_eq!(maker_id, maker_older);
            assert_eq!(qty_lot, 1);
            assert_eq!(price_tick, 100);
        }
        other => panic!("expected Fill, got {other:?}"),
    }
    // Younger sell still resting.
    assert_eq!(e.book.best_ask(), Some(100));
    assert_eq!(e.book.front_id(Side::Sell, 100).is_some(), true);
}

#[test]
fn partial_fill_leaves_remainder() {
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
        price_tick: 100,
        qty_lot: 3,
        ts: 2,
        client_id: 2,
    });
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
        HpEvent::Rest {
            side: Side::Buy,
            price_tick: 100,
            qty_lot: 2,
            ..
        }
    ));
    assert_eq!(e.book.best_bid(), Some(100));
    assert_eq!(e.book.best_ask(), None);
    let depth = e.book.depth(Side::Buy, 1);
    assert_eq!(depth, vec![(100, 2)]);
}

#[test]
fn cancel_removes_resting() {
    let mut e = HpEngine::new();
    let ev = e.on_order(HpCommand::Limit {
        side: Side::Buy,
        price_tick: 100,
        qty_lot: 5,
        ts: 1,
        client_id: 1,
    });
    let id = match ev[0] {
        HpEvent::Rest { id, .. } => id,
        _ => panic!("expected Rest"),
    };
    let ev = e.on_order(HpCommand::Cancel { id });
    assert!(matches!(ev[0], HpEvent::Revoke { id: rid, .. } if rid == id));
    assert!(e.book.best_bid().is_none());
}

#[test]
fn fill_uses_maker_price() {
    let mut e = HpEngine::new();
    e.on_order(HpCommand::Limit {
        side: Side::Sell,
        price_tick: 100,
        qty_lot: 2,
        ts: 1,
        client_id: 1,
    });
    // Aggressive buy at 105 should fill at maker 100.
    let ev = e.on_order(HpCommand::Limit {
        side: Side::Buy,
        price_tick: 105,
        qty_lot: 2,
        ts: 2,
        client_id: 2,
    });
    assert!(matches!(
        ev[0],
        HpEvent::Fill {
            price_tick: 100,
            qty_lot: 2,
            ..
        }
    ));
}

#[test]
fn walks_multiple_ask_levels() {
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
    assert!(matches!(
        ev[0],
        HpEvent::Fill {
            price_tick: 100,
            qty_lot: 1,
            ..
        }
    ));
    assert!(matches!(
        ev[1],
        HpEvent::Fill {
            price_tick: 101,
            qty_lot: 1,
            ..
        }
    ));
    assert!(e.book.best_ask().is_none());
}
