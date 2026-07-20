use bigdecimal::BigDecimal;
use match_core::{BbOrder, Engine, MatchEvent, Side};
use std::str::FromStr;

fn dec(s: &str) -> BigDecimal {
    BigDecimal::from_str(s).unwrap()
}

#[test]
fn limit_sell_rests_when_not_crossing() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("99"), "b1", 1, "1"));
    let events = eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s1", 2, "1"));
    assert!(events.is_empty());
    assert_eq!(eng.depth_levels("btcusdt", Side::Sell, 20).len(), 1);
}

#[test]
fn sell_smaller_than_resting_buy_leaves_partial_buy() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b1", 1, "3"));
    let events = eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s1", 2, "1"));

    let fills: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, MatchEvent::Fill { .. }))
        .collect();
    assert_eq!(fills.len(), 1);
    if let MatchEvent::Fill {
        qty,
        taker_remaining,
        maker_remaining,
        ..
    } = &fills[0]
    {
        assert_eq!(qty, "1");
        assert_eq!(taker_remaining, "0");
        assert_eq!(maker_remaining, "2");
    }
    let buy_depth = eng.depth_levels("btcusdt", Side::Buy, 20);
    assert_eq!(buy_depth.len(), 1);
    assert_eq!(buy_depth[0].1, dec("2"));
}

#[test]
fn sell_larger_than_resting_buy_leaves_partial_sell() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b1", 1, "1"));
    let events = eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s1", 2, "3"));

    let fills: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, MatchEvent::Fill { .. }))
        .collect();
    assert_eq!(fills.len(), 1);
    if let MatchEvent::Fill {
        qty,
        taker_remaining,
        maker_remaining,
        ..
    } = &fills[0]
    {
        assert_eq!(qty, "1");
        assert_eq!(taker_remaining, "2");
        assert_eq!(maker_remaining, "0");
    }
}

#[test]
fn ioc_buy_fully_filled_against_larger_sell_stops_without_revoke() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s1", 1, "2"));
    let events = eng.on_order(BbOrder::test_ioc(Side::Buy, dec("100"), "b_ioc", 2, "1"));

    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, MatchEvent::Fill { .. }))
            .count(),
        1
    );
    assert!(!events
        .iter()
        .any(|e| matches!(e, MatchEvent::Revoke { .. })));
}

#[test]
fn ioc_sell_fully_filled_against_larger_buy_stops_without_revoke() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b1", 1, "2"));
    let events = eng.on_order(BbOrder::test_ioc(Side::Sell, dec("100"), "s_ioc", 2, "1"));

    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, MatchEvent::Fill { .. }))
            .count(),
        1
    );
    assert!(!events
        .iter()
        .any(|e| matches!(e, MatchEvent::Revoke { .. })));
}

#[test]
fn ioc_buy_fully_filled_does_not_revoke() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s1", 1, "1"));
    let events = eng.on_order(BbOrder::test_ioc(Side::Buy, dec("100"), "b_ioc", 2, "1"));

    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, MatchEvent::Fill { .. }))
            .count(),
        1
    );
    assert!(!events
        .iter()
        .any(|e| matches!(e, MatchEvent::Revoke { .. })));
}

#[test]
fn ioc_buy_full_fill_with_leftover_sell_hits_empty_buy_break() {
    // After IOC fully fills, sell liquidity remains so the loop hits is_empty(Buy).
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s1", 1, "1"));
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("101"), "s2", 2, "1"));
    let events = eng.on_order(BbOrder::test_ioc(Side::Buy, dec("100"), "b_ioc", 3, "1"));

    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, MatchEvent::Fill { .. }))
            .count(),
        1
    );
    assert!(!events
        .iter()
        .any(|e| matches!(e, MatchEvent::Revoke { .. })));
    assert_eq!(eng.depth_levels("btcusdt", Side::Sell, 20).len(), 1);
}

#[test]
fn ioc_sell_full_fill_with_leftover_buy_hits_empty_sell_break() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b1", 1, "1"));
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("99"), "b2", 2, "1"));
    let events = eng.on_order(BbOrder::test_ioc(Side::Sell, dec("100"), "s_ioc", 3, "1"));

    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, MatchEvent::Fill { .. }))
            .count(),
        1
    );
    assert!(!events
        .iter()
        .any(|e| matches!(e, MatchEvent::Revoke { .. })));
    assert_eq!(eng.depth_levels("btcusdt", Side::Buy, 20).len(), 1);
}

#[test]
fn ioc_sell_fully_filled_does_not_revoke() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b1", 1, "1"));
    let events = eng.on_order(BbOrder::test_ioc(Side::Sell, dec("100"), "s_ioc", 2, "1"));

    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, MatchEvent::Fill { .. }))
            .count(),
        1
    );
    assert!(!events
        .iter()
        .any(|e| matches!(e, MatchEvent::Revoke { .. })));
}

#[test]
fn ioc_empty_sell_book_revokes_buy() {
    let mut eng = Engine::new();
    let events = eng.on_order(BbOrder::test_ioc(Side::Buy, dec("100"), "b_ioc", 1, "1"));
    assert!(events.iter().any(|e| matches!(
        e,
        MatchEvent::Revoke {
            order_no,
            reason,
            ..
        } if order_no == "b_ioc" && reason == "ioc_remainder"
    )));
}

#[test]
fn ioc_empty_buy_side_revokes_sell() {
    let mut eng = Engine::new();
    let events = eng.on_order(BbOrder::test_ioc(Side::Sell, dec("100"), "s_ioc", 1, "1"));
    assert!(events.iter().any(|e| matches!(
        e,
        MatchEvent::Revoke {
            order_no,
            reason,
            ..
        } if order_no == "s_ioc" && reason == "ioc_remainder"
    )));
}

#[test]
fn fok_buy_walk_rolls_back_on_price_gap() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s1", 1, "1"));
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("105"), "s2", 2, "1"));
    let before = eng.depth_levels("btcusdt", Side::Sell, 20);

    let events = eng.on_order(BbOrder::test_fok(Side::Buy, dec("100"), "b_fok", 3, "2"));
    assert!(!events.iter().any(|e| matches!(e, MatchEvent::Fill { .. })));
    assert!(events.iter().any(|e| matches!(
        e,
        MatchEvent::Revoke {
            order_no,
            reason,
            ..
        } if order_no == "b_fok" && reason == "fok_fail"
    )));
    assert_eq!(eng.depth_levels("btcusdt", Side::Sell, 20), before);
}

#[test]
fn fok_sell_walk_rolls_back_on_price_gap() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b1", 1, "1"));
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("95"), "b2", 2, "1"));
    let before = eng.depth_levels("btcusdt", Side::Buy, 20);

    let events = eng.on_order(BbOrder::test_fok(Side::Sell, dec("100"), "s_fok", 3, "2"));
    assert!(!events.iter().any(|e| matches!(e, MatchEvent::Fill { .. })));
    assert!(events.iter().any(|e| matches!(
        e,
        MatchEvent::Revoke {
            order_no,
            reason,
            ..
        } if order_no == "s_fok" && reason == "fok_fail"
    )));
    assert_eq!(eng.depth_levels("btcusdt", Side::Buy, 20), before);
}

#[test]
fn fok_buy_walk_fills_against_larger_second_sell() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s1", 1, "1"));
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s2", 2, "5"));
    let events = eng.on_order(BbOrder::test_fok(Side::Buy, dec("100"), "b_fok", 3, "4"));

    let fills: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, MatchEvent::Fill { .. }))
        .collect();
    assert_eq!(fills.len(), 2);
    assert!(!events
        .iter()
        .any(|e| matches!(e, MatchEvent::Revoke { .. })));
}

#[test]
fn fok_sell_walk_fills_against_larger_second_buy() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b1", 1, "1"));
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b2", 2, "5"));
    let events = eng.on_order(BbOrder::test_fok(Side::Sell, dec("100"), "s_fok", 3, "4"));

    let fills: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, MatchEvent::Fill { .. }))
        .collect();
    assert_eq!(fills.len(), 2);
    assert!(!events
        .iter()
        .any(|e| matches!(e, MatchEvent::Revoke { .. })));
}

#[test]
fn limit_buy_does_not_cross_resting_sell() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("101"), "s1", 1, "1"));
    let events = eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b1", 2, "1"));
    assert!(events.is_empty());
    assert_eq!(eng.depth_levels("btcusdt", Side::Buy, 20).len(), 1);
    assert_eq!(eng.depth_levels("btcusdt", Side::Sell, 20).len(), 1);
}

#[test]
fn market_buy_fully_filled_empties_buy_book() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s1", 1, "1"));
    let events = eng.on_order(BbOrder::test_market(Side::Buy, "b_mkt", 2, "1"));
    assert_eq!(events.len(), 1);
    assert!(eng.depth_levels("btcusdt", Side::Buy, 20).is_empty());
}

#[test]
fn market_sell_fully_filled_empties_sell_book() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b1", 1, "1"));
    let events = eng.on_order(BbOrder::test_market(Side::Sell, "s_mkt", 2, "1"));
    assert_eq!(events.len(), 1);
    assert!(eng.depth_levels("btcusdt", Side::Sell, 20).is_empty());
}

#[test]
fn market_buy_gear_limit_revokes_after_reaching_gear() {
    let mut eng = Engine::new();
    for i in 0..3 {
        eng.on_order(BbOrder::test_limit(
            Side::Sell,
            dec(&(100 + i).to_string()),
            &format!("s{i}"),
            i,
            "1",
        ));
    }
    let mut taker = BbOrder::test_market(Side::Buy, "b_gear1", 10, "5");
    taker.gear = Some(1);
    let events = eng.on_order(taker);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, MatchEvent::Fill { .. }))
            .count(),
        1
    );
    assert!(events.iter().any(|e| matches!(
        e,
        MatchEvent::Revoke {
            reason,
            ..
        } if reason == "market_gear"
    )));
}

#[test]
fn market_sell_gear_limit_revokes_after_reaching_gear() {
    let mut eng = Engine::new();
    for i in 0..3 {
        eng.on_order(BbOrder::test_limit(
            Side::Buy,
            dec(&(100 - i).to_string()),
            &format!("b{i}"),
            i,
            "1",
        ));
    }
    let mut taker = BbOrder::test_market(Side::Sell, "s_gear1", 10, "5");
    taker.gear = Some(1);
    let events = eng.on_order(taker);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, MatchEvent::Fill { .. }))
            .count(),
        1
    );
    assert!(events.iter().any(|e| matches!(
        e,
        MatchEvent::Revoke {
            reason,
            ..
        } if reason == "market_gear"
    )));
}
