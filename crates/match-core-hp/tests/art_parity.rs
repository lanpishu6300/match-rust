//! Same logical sequence must produce identical fills/depth under default and `--features art`.
//! Run both:
//!   cargo test -p match-core-hp --test art_parity
//!   cargo test -p match-core-hp --features art --test art_parity

use match_core_hp::{HpCommand, HpEngine, HpEvent, Side};

fn count_fills(eng: &mut HpEngine, cmds: &[HpCommand]) -> u64 {
    let mut n = 0u64;
    for c in cmds {
        for e in eng.on_order(*c) {
            if matches!(e, HpEvent::Fill { .. }) {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn fair_cross_fill_count() {
    let n = 200usize;
    let mut cmds = Vec::with_capacity(n);
    for i in 0..n / 2 {
        cmds.push(HpCommand::Limit {
            side: Side::Sell,
            price_tick: 10_000,
            qty_lot: 1_000_000,
            ts: i as u64,
            client_id: i as u64,
        });
    }
    for i in n / 2..n {
        cmds.push(HpCommand::Limit {
            side: Side::Buy,
            price_tick: 10_000,
            qty_lot: 1_000_000,
            ts: i as u64,
            client_id: i as u64,
        });
    }
    let mut eng = HpEngine::with_capacity(n + 8, 64);
    let fills = count_fills(&mut eng, &cmds);
    assert_eq!(fills, (n / 2) as u64);
    assert_eq!(eng.book.best_ask(), None);
    assert_eq!(eng.book.best_bid(), None);
}

#[test]
fn depth_order_after_mixed_rest() {
    let mut eng = HpEngine::new();
    for (tick, side) in [
        (100i64, Side::Buy),
        (110, Side::Buy),
        (90, Side::Buy),
        (200, Side::Sell),
        (190, Side::Sell),
        (210, Side::Sell),
    ] {
        eng.on_order(HpCommand::Limit {
            side,
            price_tick: tick,
            qty_lot: 1,
            ts: tick as u64,
            client_id: tick as u64,
        });
    }
    assert_eq!(
        eng.book.depth(Side::Buy, 3),
        vec![(110, 1), (100, 1), (90, 1)]
    );
    assert_eq!(
        eng.book.depth(Side::Sell, 3),
        vec![(190, 1), (200, 1), (210, 1)]
    );
}
