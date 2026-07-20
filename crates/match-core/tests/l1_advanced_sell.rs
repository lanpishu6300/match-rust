use bigdecimal::BigDecimal;
use match_core::{BbOrder, Engine, MatchEvent, Side};
use std::str::FromStr;

fn dec(s: &str) -> BigDecimal {
    BigDecimal::from_str(s).unwrap()
}

#[test]
fn post_only_sell_that_would_take_revokes() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b1", 1, "1"));
    let events = eng.on_order(BbOrder::test_post_only(
        Side::Sell,
        dec("100"),
        "s_po",
        2,
        "1",
    ));

    assert!(!events.iter().any(|e| matches!(e, MatchEvent::Fill { .. })));
    let revokes: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, MatchEvent::Revoke { .. }))
        .collect();
    assert_eq!(revokes.len(), 1);
    if let MatchEvent::Revoke {
        order_no, reason, ..
    } = &revokes[0]
    {
        assert_eq!(order_no, "s_po");
        assert_eq!(reason, "post_only");
    }
    assert!(eng.depth_levels("btcusdt", Side::Sell, 20).is_empty());
    let buy_depth = eng.depth_levels("btcusdt", Side::Buy, 20);
    assert_eq!(buy_depth.len(), 1);
}

#[test]
fn post_only_sell_rests_when_not_crossing() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("99"), "b1", 1, "1"));
    let events = eng.on_order(BbOrder::test_post_only(
        Side::Sell,
        dec("100"),
        "s_po",
        2,
        "1",
    ));

    assert!(events.is_empty());
    let sell_depth = eng.depth_levels("btcusdt", Side::Sell, 20);
    assert_eq!(sell_depth.len(), 1);
    assert_eq!(sell_depth[0].0, dec("100"));
}

#[test]
fn post_only_buy_rests_when_not_crossing() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("101"), "s1", 1, "1"));
    let events = eng.on_order(BbOrder::test_post_only(
        Side::Buy,
        dec("100"),
        "b_po",
        2,
        "1",
    ));

    assert!(events.is_empty());
    let buy_depth = eng.depth_levels("btcusdt", Side::Buy, 20);
    assert_eq!(buy_depth.len(), 1);
    assert_eq!(buy_depth[0].0, dec("100"));
}

#[test]
fn ioc_sell_partial_fill_revokes_remainder() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b1", 1, "1"));
    let events = eng.on_order(BbOrder::test_ioc(Side::Sell, dec("100"), "s_ioc", 2, "3"));

    let fills: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, MatchEvent::Fill { .. }))
        .collect();
    assert_eq!(fills.len(), 1);
    if let MatchEvent::Fill {
        qty,
        taker_remaining,
        ..
    } = &fills[0]
    {
        assert_eq!(qty, "1");
        assert_eq!(taker_remaining, "2");
    }

    let revokes: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, MatchEvent::Revoke { .. }))
        .collect();
    assert_eq!(revokes.len(), 1);
    if let MatchEvent::Revoke {
        order_no,
        reason,
        remaining,
        ..
    } = &revokes[0]
    {
        assert_eq!(order_no, "s_ioc");
        assert_eq!(reason, "ioc_remainder");
        assert_eq!(remaining, "2");
    }
}

#[test]
fn ioc_buy_no_cross_revokes_without_fill() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("101"), "s1", 1, "1"));
    let events = eng.on_order(BbOrder::test_ioc(Side::Buy, dec("100"), "b_ioc", 2, "1"));

    assert!(!events.iter().any(|e| matches!(e, MatchEvent::Fill { .. })));
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
fn ioc_sell_no_cross_revokes_without_fill() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("99"), "b1", 1, "1"));
    let events = eng.on_order(BbOrder::test_ioc(Side::Sell, dec("100"), "s_ioc", 2, "1"));

    assert!(!events.iter().any(|e| matches!(e, MatchEvent::Fill { .. })));
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
fn fok_sell_success_full_fill_no_remainder() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b1", 1, "2"));
    let events = eng.on_order(BbOrder::test_fok(Side::Sell, dec("100"), "s_fok", 2, "2"));

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
        assert_eq!(qty, "2");
        assert_eq!(taker_remaining, "0");
        assert_eq!(maker_remaining, "0");
    }
    assert!(!events
        .iter()
        .any(|e| matches!(e, MatchEvent::Revoke { .. })));
}

#[test]
fn fok_sell_fail_no_net_book_change_and_revoke() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b1", 1, "1"));
    let before_buy = eng.depth_levels("btcusdt", Side::Buy, 20);
    assert_eq!(before_buy.len(), 1);

    let events = eng.on_order(BbOrder::test_fok(Side::Sell, dec("100"), "s_fok", 2, "2"));

    assert!(!events.iter().any(|e| matches!(e, MatchEvent::Fill { .. })));
    assert!(events.iter().any(|e| matches!(
        e,
        MatchEvent::Revoke {
            order_no,
            reason,
            ..
        } if order_no == "s_fok" && reason == "fok_fail"
    )));
    assert_eq!(eng.depth_levels("btcusdt", Side::Buy, 20), before_buy);
}

#[test]
fn fok_buy_empty_sell_book_revokes() {
    let mut eng = Engine::new();
    let events = eng.on_order(BbOrder::test_fok(Side::Buy, dec("100"), "b_fok", 1, "1"));

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
fn fok_sell_empty_buy_book_revokes() {
    let mut eng = Engine::new();
    let events = eng.on_order(BbOrder::test_fok(Side::Sell, dec("100"), "s_fok", 1, "1"));

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
