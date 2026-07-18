use bigdecimal::BigDecimal;
use match_core::{BbOrder, Engine, MatchEvent, Side};
use std::str::FromStr;

fn dec(s: &str) -> BigDecimal {
    BigDecimal::from_str(s).unwrap()
}

#[test]
fn limit_buy_rests_on_empty_book() {
    let mut eng = Engine::new();
    let events = eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "1", 1, "2"));
    assert!(events.iter().all(|e| !matches!(e, MatchEvent::Fill { .. })));
    let depth = eng.depth_levels("btcusdt", Side::Buy, 20);
    assert_eq!(depth.len(), 1);
    assert_eq!(depth[0].1, dec("2"));
}
