use bigdecimal::BigDecimal;
use match_core::{BbOrder, Engine, MatchEvent, Side};
use std::str::FromStr;

fn dec(s: &str) -> BigDecimal {
    BigDecimal::from_str(s).unwrap()
}

#[test]
fn post_only_that_would_take_revokes_no_fill_not_resting() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s1", 1, "1"));
    let events = eng.on_order(BbOrder::test_post_only(
        Side::Buy,
        dec("100"),
        "b_po",
        2,
        "1",
    ));

    let fills: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, MatchEvent::Fill { .. }))
        .collect();
    assert!(fills.is_empty(), "PostOnly must not fill");

    let revokes: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, MatchEvent::Revoke { .. }))
        .collect();
    assert_eq!(revokes.len(), 1);
    if let MatchEvent::Revoke {
        order_no, reason, ..
    } = &revokes[0]
    {
        assert_eq!(order_no, "b_po");
        assert_eq!(reason, "post_only");
    }

    // Java P2-2 flash: briefly on book then revoked — final state not resting.
    assert!(eng.depth_levels("btcusdt", Side::Buy, 20).is_empty());
    let sell_depth = eng.depth_levels("btcusdt", Side::Sell, 20);
    assert_eq!(sell_depth.len(), 1);
    assert_eq!(sell_depth[0].1, dec("1"));
}

#[test]
fn ioc_partial_fill_revokes_remainder() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s1", 1, "1"));
    let events = eng.on_order(BbOrder::test_ioc(Side::Buy, dec("100"), "b_ioc", 2, "3"));

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
        assert_eq!(order_no, "b_ioc");
        assert_eq!(reason, "ioc_remainder");
        assert_eq!(remaining, "2");
    }

    assert!(eng.depth_levels("btcusdt", Side::Buy, 20).is_empty());
    assert!(eng.depth_levels("btcusdt", Side::Sell, 20).is_empty());
}

#[test]
fn fok_success_full_fill_no_remainder() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s1", 1, "2"));
    let events = eng.on_order(BbOrder::test_fok(Side::Buy, dec("100"), "b_fok", 2, "2"));

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

    assert!(
        !events
            .iter()
            .any(|e| matches!(e, MatchEvent::Revoke { .. })),
        "FOK success must not revoke"
    );
    assert!(eng.depth_levels("btcusdt", Side::Buy, 20).is_empty());
    assert!(eng.depth_levels("btcusdt", Side::Sell, 20).is_empty());
}

#[test]
fn fok_fail_no_net_book_change_and_revoke() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s1", 1, "1"));
    let before_sell = eng.depth_levels("btcusdt", Side::Sell, 20);
    assert_eq!(before_sell.len(), 1);
    assert_eq!(before_sell[0].1, dec("1"));

    let events = eng.on_order(BbOrder::test_fok(Side::Buy, dec("100"), "b_fok", 2, "3"));

    assert!(
        !events.iter().any(|e| matches!(e, MatchEvent::Fill { .. })),
        "FOK fail must not emit fills (Java discards walk list on rollback)"
    );

    let revokes: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, MatchEvent::Revoke { .. }))
        .collect();
    assert_eq!(revokes.len(), 1);
    if let MatchEvent::Revoke {
        order_no, reason, ..
    } = &revokes[0]
    {
        assert_eq!(order_no, "b_fok");
        assert_eq!(reason, "fok_fail");
    }

    assert!(eng.depth_levels("btcusdt", Side::Buy, 20).is_empty());
    let after_sell = eng.depth_levels("btcusdt", Side::Sell, 20);
    assert_eq!(after_sell, before_sell);
}
