//! SPSC ring edge cases for coverage of single-pop and batch paths.

use match_core_hp::{HpCommand, Side, SpscRing};

#[test]
fn pop_n_on_empty_returns_zero() {
    let r = SpscRing::with_capacity(4);
    let mut out = Vec::new();
    assert_eq!(r.pop_n(&mut out, 8), 0);
    assert!(out.is_empty());
}

#[test]
fn pop_n_zero_max_is_no_op() {
    let r = SpscRing::with_capacity(4);
    r.try_push(HpCommand::Cancel { id: 1 }).unwrap();
    let mut out = Vec::new();
    assert_eq!(r.pop_n(&mut out, 0), 0);
    assert!(out.is_empty());
    assert_eq!(r.len_approx(), 1);
}

#[test]
fn try_pop_drains_after_batch_push() {
    let r = SpscRing::with_capacity(4);
    for id in 1..=3u64 {
        r.try_push(HpCommand::Cancel { id }).unwrap();
    }
    assert_eq!(r.len_approx(), 3);
    for id in 1..=3u64 {
        assert_eq!(r.try_pop(), Some(HpCommand::Cancel { id }));
    }
    assert!(r.try_pop().is_none());
}

#[test]
fn mixed_command_types_round_trip() {
    let r = SpscRing::with_capacity(8);
    r.try_push(HpCommand::Limit {
        side: Side::Buy,
        price_tick: 1,
        qty_lot: 2,
        ts: 1,
        client_id: 1,
    })
    .unwrap();
    r.try_push(HpCommand::Market {
        side: Side::Sell,
        qty_lot: 3,
        ts: 2,
        max_levels: Some(1),
        client_id: 2,
    })
    .unwrap();
    r.try_push(HpCommand::Cancel { id: 9 }).unwrap();

    let mut out = Vec::new();
    assert_eq!(r.pop_n(&mut out, 2), 2);
    assert!(matches!(out[0], HpCommand::Limit { .. }));
    assert!(matches!(out[1], HpCommand::Market { .. }));
    assert_eq!(r.try_pop(), Some(HpCommand::Cancel { id: 9 }));
}
