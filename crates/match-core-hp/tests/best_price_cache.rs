use match_core_hp::{Book, HpOrder, Side};

#[test]
fn insert_better_bid_updates_cache() {
    let mut b = Book::new();
    b.insert_limit(HpOrder::limit(Side::Buy, 100, 1, 1));
    assert_eq!(b.best_bid(), Some(100));
    b.insert_limit(HpOrder::limit(Side::Buy, 110, 1, 2));
    assert_eq!(b.best_bid(), Some(110));
    b.insert_limit(HpOrder::limit(Side::Buy, 105, 1, 3));
    assert_eq!(b.best_bid(), Some(110));
}

#[test]
fn cancel_best_bid_falls_back() {
    let mut b = Book::new();
    let hi = b.insert_limit(HpOrder::limit(Side::Buy, 110, 1, 1));
    b.insert_limit(HpOrder::limit(Side::Buy, 100, 1, 2));
    assert_eq!(b.best_bid(), Some(110));
    assert!(b.cancel(hi));
    assert_eq!(b.best_bid(), Some(100));
}

#[test]
fn cancel_last_ask_clears_cache() {
    let mut b = Book::new();
    let id = b.insert_limit(HpOrder::limit(Side::Sell, 50, 1, 1));
    assert_eq!(b.best_ask(), Some(50));
    assert!(b.cancel(id));
    assert_eq!(b.best_ask(), None);
}

#[test]
fn fill_empties_best_ask() {
    let mut b = Book::new();
    let id = b.insert_limit(HpOrder::limit(Side::Sell, 50, 2, 1));
    b.insert_limit(HpOrder::limit(Side::Sell, 60, 1, 2));
    assert_eq!(b.best_ask(), Some(50));
    assert!(b.fill_order(id, 2).is_none());
    assert_eq!(b.best_ask(), Some(60));
}

#[test]
fn level_pool_reuse_after_mass_cancel() {
    let mut b = Book::new();
    let mut ids = Vec::new();
    for i in 0..64 {
        ids.push(b.insert_limit(HpOrder::limit(Side::Buy, 1000 + i, 1, i as u64)));
    }
    assert_eq!(b.best_bid(), Some(1000 + 63));
    for id in ids {
        assert!(b.cancel(id));
    }
    assert_eq!(b.best_bid(), None);

    let id = b.insert_limit(HpOrder::limit(Side::Buy, 42, 3, 99));
    assert_eq!(b.best_bid(), Some(42));
    assert_eq!(b.front_id(Side::Buy, 42), Some(id));
    assert_eq!(b.depth(Side::Buy, 1), vec![(42, 3)]);
}
