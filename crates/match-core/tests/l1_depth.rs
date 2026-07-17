use bigdecimal::BigDecimal;
use match_core::{BbOrder, Engine, Side};
use match_protocol::NO_DEAL_NUMBER;
use std::str::FromStr;

fn dec(s: &str) -> BigDecimal {
    BigDecimal::from_str(s).unwrap()
}

#[test]
fn depth_aggregates_same_price() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "1", 1, "1"));
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "2", 2, "2"));
    let levels = eng.depth_levels("btcusdt", Side::Buy, NO_DEAL_NUMBER as usize);
    assert_eq!(levels.len(), 1);
    assert_eq!(levels[0].1, dec("3"));
}

#[test]
fn depth_best_prices_first_and_limits_levels() {
    let mut buy_eng = Engine::new();
    for (i, price) in ["98", "100", "99", "101", "97"].iter().enumerate() {
        buy_eng.on_order(BbOrder::test_limit(
            Side::Buy,
            dec(price),
            &format!("b{i}"),
            i as i64 + 1,
            "1",
        ));
    }
    let bids = buy_eng.depth_levels("btcusdt", Side::Buy, 3);
    assert_eq!(bids.len(), 3);
    assert_eq!(bids[0].0, dec("101"));
    assert_eq!(bids[1].0, dec("100"));
    assert_eq!(bids[2].0, dec("99"));

    let mut sell_eng = Engine::new();
    for (i, price) in ["102", "100", "101", "99", "103"].iter().enumerate() {
        sell_eng.on_order(BbOrder::test_limit(
            Side::Sell,
            dec(price),
            &format!("s{i}"),
            i as i64 + 1,
            "1",
        ));
    }
    let asks = sell_eng.depth_levels("btcusdt", Side::Sell, 3);
    assert_eq!(asks.len(), 3);
    assert_eq!(asks[0].0, dec("99"));
    assert_eq!(asks[1].0, dec("100"));
    assert_eq!(asks[2].0, dec("101"));
}

#[test]
fn depth_after_rather_than_buy_partial_reflects_survivor_remaining() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s1", 1, "1"));
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b1", 2, "3"));
    let levels = eng.depth_levels("btcusdt", Side::Buy, NO_DEAL_NUMBER as usize);
    assert_eq!(levels.len(), 1);
    assert_eq!(levels[0].0, dec("100"));
    assert_eq!(levels[0].1, dec("2"));
    assert!(eng.depth_levels("btcusdt", Side::Sell, NO_DEAL_NUMBER as usize).is_empty());
}

#[test]
fn depth_after_rather_than_sell_partial_reflects_survivor_remaining() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b1", 1, "1"));
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s1", 2, "3"));
    let levels = eng.depth_levels("btcusdt", Side::Sell, NO_DEAL_NUMBER as usize);
    assert_eq!(levels.len(), 1);
    assert_eq!(levels[0].0, dec("100"));
    assert_eq!(levels[0].1, dec("2"));
    assert!(eng.depth_levels("btcusdt", Side::Buy, NO_DEAL_NUMBER as usize).is_empty());
}
