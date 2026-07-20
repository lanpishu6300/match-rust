use bigdecimal::BigDecimal;
use match_core::{BbOrder, Engine, MatchEvent, OrderBook, Side};
use match_protocol::ORDER_STATUS_REVOKE;
use std::str::FromStr;

fn dec(s: &str) -> BigDecimal {
    BigDecimal::from_str(s).unwrap()
}

fn order(side: Side, price: &str, no: &str, t: i64, qty: &str) -> BbOrder {
    BbOrder::test_limit(side, BigDecimal::from_str(price).unwrap(), no, t, qty)
}

#[test]
fn book_remove_and_remove_by_order_no() {
    let mut book = OrderBook::new();
    let buy = order(Side::Buy, "100", "b1", 1, "1");
    let sell = order(Side::Sell, "101", "s1", 1, "1");
    book.insert(buy.clone());
    book.insert(sell.clone());

    assert!(book.remove(&buy));
    assert!(book.is_empty(Side::Buy));
    assert_eq!(
        book.remove_by_order_no(Side::Sell, "s1")
            .unwrap()
            .trust_order_no,
        "s1"
    );
    assert!(book.is_empty(Side::Sell));
    assert!(book.remove_by_order_no(Side::Buy, "missing").is_none());
}

#[test]
fn book_pop_first_and_invalid_side_insert() {
    let mut book = OrderBook::new();
    book.insert(order(Side::Buy, "100", "b1", 1, "1"));
    book.insert(order(Side::Buy, "101", "b2", 2, "1"));

    let popped = book.pop_first(Side::Buy).unwrap();
    assert_eq!(popped.trust_order_no, "b2");

    let mut invalid = order(Side::Buy, "99", "x", 3, "1");
    invalid.order_type = 99;
    book.insert(invalid);
    assert_eq!(book.depth_levels(Side::Buy, 10).len(), 1);
}

#[test]
fn engine_user_revoke_emits_revoke_event() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b1", 1, "1"));

    let mut revoke = BbOrder::test_limit(Side::Buy, dec("100"), "b1", 2, "1");
    revoke.order_status = ORDER_STATUS_REVOKE;
    let events = eng.on_order(revoke);

    assert_eq!(events.len(), 1);
    if let MatchEvent::Revoke {
        order_no,
        reason,
        remaining,
        ..
    } = &events[0]
    {
        assert_eq!(order_no, "b1");
        assert_eq!(reason, "user");
        assert_eq!(remaining, "1");
    }
    assert!(eng.depth_levels("btcusdt", Side::Buy, 20).is_empty());
}

#[test]
fn engine_unknown_symbol_depth_is_empty() {
    let eng = Engine::new();
    assert!(eng.depth_levels("unknown", Side::Buy, 20).is_empty());
}

#[test]
fn engine_invalid_order_type_rest_only_no_events() {
    let mut eng = Engine::new();
    let mut o = BbOrder::test_limit(Side::Buy, dec("100"), "b1", 1, "1");
    o.order_type = 0;
    let events = eng.on_order(o);
    assert!(events.is_empty());
    assert!(eng.depth_levels("btcusdt", Side::Buy, 20).is_empty());
}

#[test]
fn compare_same_order_no_is_equal_for_book_ordering() {
    let mut book = OrderBook::new();
    assert!(book.insert(order(Side::Buy, "100", "same", 1, "1")));
    assert!(!book.insert(order(Side::Buy, "100", "same", 2, "1")));
    assert_eq!(book.depth_levels(Side::Buy, 10).len(), 1);
}

#[test]
fn engine_rejects_duplicate_trust_order_no() {
    let mut eng = Engine::new();
    let events1 = eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "dup", 1, "1"));
    assert!(events1.is_empty());
    let events2 = eng.on_order(BbOrder::test_limit(Side::Buy, dec("101"), "dup", 2, "1"));
    assert!(events2.is_empty());
    assert_eq!(eng.depth_levels("btcusdt", Side::Buy, 20).len(), 1);
}
