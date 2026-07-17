use match_core_hp::{HpCommand, HpEvent, HpWorker, Side};

#[test]
fn submit_then_run_once_produces_fills() {
    let mut w = HpWorker::new(64);

    // Rest a sell.
    w.try_submit(HpCommand::Limit {
        side: Side::Sell,
        price_tick: 100,
        qty_lot: 5,
        ts: 1,
        client_id: 1,
    })
    .unwrap();
    assert_eq!(w.run_once(), 0);
    assert_eq!(w.engine().book.best_ask(), Some(100));

    // Aggressive buy fully fills.
    w.try_submit(HpCommand::Limit {
        side: Side::Buy,
        price_tick: 100,
        qty_lot: 5,
        ts: 2,
        client_id: 2,
    })
    .unwrap();
    assert_eq!(w.run_once(), 1);
    assert!(w.engine().book.best_ask().is_none());
}

#[test]
fn try_submit_busy_when_full() {
    let w = HpWorker::new(2); // rounds to 2
    // Capacity 2: can push until (tail - head) > mask, i.e. 2 slots usable
    // with our full check `tail - head > mask` ⇒ full when depth == cap.
    w.try_submit(HpCommand::Cancel { id: 1 }).unwrap();
    w.try_submit(HpCommand::Cancel { id: 2 }).unwrap();
    assert!(w.try_submit(HpCommand::Cancel { id: 3 }).is_err());
}

#[test]
fn engine_reuses_event_buffer_across_calls() {
    let mut w = HpWorker::new(32);
    for i in 0..10 {
        w.try_submit(HpCommand::Limit {
            side: Side::Sell,
            price_tick: 100 + i,
            qty_lot: 1,
            ts: i as u64,
            client_id: i as u64,
        })
        .unwrap();
    }
    w.run_once();
    w.try_submit(HpCommand::Market {
        side: Side::Buy,
        qty_lot: 10,
        ts: 100,
        max_levels: None,
        client_id: 99,
    })
    .unwrap();
    let fills = w.run_once();
    assert_eq!(fills, 10);
    // Spot-check last events via a direct engine call (buffer cleared each time).
    let ev = w.engine_mut().on_order(HpCommand::Cancel { id: 999_999 });
    assert!(ev.iter().all(|e| !matches!(e, HpEvent::Fill { .. })));
}
