use bigdecimal::BigDecimal;
use match_core::{BbOrder, Engine, MatchEvent, Side};
use std::str::FromStr;

fn dec(s: &str) -> BigDecimal {
    BigDecimal::from_str(s).unwrap()
}

#[test]
fn market_buy_stops_at_gear_levels() {
    let mut eng = Engine::new();
    for i in 0..5 {
        eng.on_order(BbOrder::test_limit(
            Side::Sell,
            dec(&(100 + i).to_string()),
            &format!("s{i}"),
            i,
            "1",
        ));
    }
    let mut taker = BbOrder::test_market(Side::Buy, "b_mkt", 10, "10");
    taker.gear = Some(2);
    // ensure order_form = 2 market
    let events = eng.on_order(taker);
    let fill_count = events
        .iter()
        .filter(|e| matches!(e, MatchEvent::Fill { .. }))
        .count();
    assert_eq!(fill_count, 2);
    assert!(events
        .iter()
        .any(|e| matches!(e, MatchEvent::Revoke { reason, .. } if reason == "market_gear")));
}

#[test]
fn market_buy_gear_zero_treated_as_one() {
    // gear < 1 is rejected by protocol validate; engine treats missing/invalid gear as 1.
    let mut eng = Engine::new();
    for i in 0..5 {
        eng.on_order(BbOrder::test_limit(
            Side::Sell,
            dec(&(100 + i).to_string()),
            &format!("s{i}"),
            i,
            "1",
        ));
    }
    let mut taker = BbOrder::test_market(Side::Buy, "b_gear0", 10, "10");
    taker.gear = Some(0);
    let events = eng.on_order(taker);
    let fill_count = events
        .iter()
        .filter(|e| matches!(e, MatchEvent::Fill { .. }))
        .count();
    assert_eq!(fill_count, 1);
    assert!(events
        .iter()
        .any(|e| matches!(e, MatchEvent::Revoke { reason, .. } if reason == "market_gear")));
}

#[test]
fn market_sell_stops_at_gear_levels() {
    let mut eng = Engine::new();
    for i in 0..5 {
        eng.on_order(BbOrder::test_limit(
            Side::Buy,
            dec(&(100 - i).to_string()),
            &format!("b{i}"),
            i,
            "1",
        ));
    }
    let mut taker = BbOrder::test_market(Side::Sell, "s_mkt", 10, "10");
    taker.gear = Some(2);
    let events = eng.on_order(taker);
    let fill_count = events
        .iter()
        .filter(|e| matches!(e, MatchEvent::Fill { .. }))
        .count();
    assert_eq!(fill_count, 2);
    assert!(events
        .iter()
        .any(|e| matches!(e, MatchEvent::Revoke { reason, .. } if reason == "market_gear")));
}

#[test]
fn market_buy_empty_book_revokes() {
    let mut eng = Engine::new();
    let mut taker = BbOrder::test_market(Side::Buy, "b_empty", 1, "1");
    taker.gear = Some(5);
    let events = eng.on_order(taker);
    assert!(events.iter().all(|e| !matches!(e, MatchEvent::Fill { .. })));
    assert!(events
        .iter()
        .any(|e| matches!(e, MatchEvent::Revoke { reason, .. } if reason == "market_empty")));
}
