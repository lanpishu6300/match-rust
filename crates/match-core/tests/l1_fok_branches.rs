use bigdecimal::BigDecimal;
use match_core::{BbOrder, Engine, MatchEvent, Side};
use std::str::FromStr;

fn dec(s: &str) -> BigDecimal {
    BigDecimal::from_str(s).unwrap()
}

#[test]
fn fok_buy_multi_level_success() {
    let mut eng = Engine::new();
    for (i, no) in ["s1", "s2", "s3"].iter().enumerate() {
        eng.on_order(BbOrder::test_limit(
            Side::Sell,
            dec("100"),
            no,
            i as i64 + 1,
            "1",
        ));
    }
    let events = eng.on_order(BbOrder::test_fok(Side::Buy, dec("100"), "b_fok", 4, "3"));

    let fills: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, MatchEvent::Fill { .. }))
        .collect();
    assert_eq!(fills.len(), 3);
    assert!(!events
        .iter()
        .any(|e| matches!(e, MatchEvent::Revoke { .. })));
    assert!(eng.depth_levels("btcusdt", Side::Buy, 20).is_empty());
    assert!(eng.depth_levels("btcusdt", Side::Sell, 20).is_empty());
}

#[test]
fn fok_buy_multi_level_fail_rolls_back() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s1", 1, "1"));
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s2", 2, "1"));
    let before = eng.depth_levels("btcusdt", Side::Sell, 20);

    let events = eng.on_order(BbOrder::test_fok(Side::Buy, dec("100"), "b_fok", 3, "3"));

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
fn fok_buy_smaller_than_resting_sell_partial_maker() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s1", 1, "3"));
    let events = eng.on_order(BbOrder::test_fok(Side::Buy, dec("100"), "b_fok", 2, "1"));

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
    assert!(eng.depth_levels("btcusdt", Side::Buy, 20).is_empty());
    let sell_depth = eng.depth_levels("btcusdt", Side::Sell, 20);
    assert_eq!(sell_depth.len(), 1);
    assert_eq!(sell_depth[0].1, dec("2"));
}

#[test]
fn fok_sell_multi_level_success() {
    let mut eng = Engine::new();
    for (i, no) in ["b1", "b2", "b3"].iter().enumerate() {
        eng.on_order(BbOrder::test_limit(
            Side::Buy,
            dec("100"),
            no,
            i as i64 + 1,
            "1",
        ));
    }
    let events = eng.on_order(BbOrder::test_fok(Side::Sell, dec("100"), "s_fok", 4, "3"));

    let fills: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, MatchEvent::Fill { .. }))
        .collect();
    assert_eq!(fills.len(), 3);
    assert!(!events
        .iter()
        .any(|e| matches!(e, MatchEvent::Revoke { .. })));
}

#[test]
fn fok_sell_multi_level_fail_rolls_back() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b1", 1, "1"));
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b2", 2, "1"));
    let before = eng.depth_levels("btcusdt", Side::Buy, 20);

    let events = eng.on_order(BbOrder::test_fok(Side::Sell, dec("100"), "s_fok", 3, "3"));

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
fn fok_sell_smaller_than_resting_buy_partial_maker() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b1", 1, "3"));
    let events = eng.on_order(BbOrder::test_fok(Side::Sell, dec("100"), "s_fok", 2, "1"));

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
fn fok_buy_no_cross_revokes() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("101"), "s1", 1, "1"));
    let events = eng.on_order(BbOrder::test_fok(Side::Buy, dec("100"), "b_fok", 2, "1"));

    assert!(!events.iter().any(|e| matches!(e, MatchEvent::Fill { .. })));
    assert!(events.iter().any(|e| matches!(
        e,
        MatchEvent::Revoke {
            order_no,
            reason,
            ..
        } if order_no == "b_fok" && reason == "fok_fail"
    )));
}

#[test]
fn fok_sell_no_cross_revokes() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("99"), "b1", 1, "1"));
    let events = eng.on_order(BbOrder::test_fok(Side::Sell, dec("100"), "s_fok", 2, "1"));

    assert!(!events.iter().any(|e| matches!(e, MatchEvent::Fill { .. })));
    assert!(events.iter().any(|e| matches!(
        e,
        MatchEvent::Revoke {
            order_no,
            reason,
            ..
        } if order_no == "s_fok" && reason == "fok_fail"
    )));
}
