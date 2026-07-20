#![cfg(feature = "art")]

//! ART index insert/remove/split paths (requires `--features art`).

use match_core_hp::{Book, HpOrder, Side};

#[test]
fn art_insert_overwrite_and_remove() {
    let mut b = Book::new();
    let id = b.insert_limit(HpOrder::limit(Side::Sell, 100, 5, 1));
    assert_eq!(b.best_ask(), Some(100));
    assert_eq!(b.depth(Side::Sell, 1), vec![(100, 5)]);

    b.insert_limit(HpOrder::limit(Side::Sell, 90, 2, 2));
    assert_eq!(b.best_ask(), Some(90));
    assert!(b.cancel(id));
    assert_eq!(b.best_ask(), Some(90));
    assert_eq!(b.depth(Side::Sell, 2), vec![(90, 2)]);
}

#[test]
fn art_split_on_shared_prefix() {
    let mut b = Book::new();
    b.insert_limit(HpOrder::limit(Side::Buy, 0xAB00, 1, 1));
    b.insert_limit(HpOrder::limit(Side::Buy, 0xABFF, 1, 2));
    assert_eq!(b.best_bid(), Some(0xABFF));
    assert_eq!(b.depth(Side::Buy, 2).len(), 2);
}

#[test]
fn art_get_mut_and_remove_missing() {
    let mut b = Book::new();
    b.insert_limit(HpOrder::limit(Side::Sell, 50, 1, 1));
    assert!(b.front_id(Side::Sell, 51).is_none());
    assert!(!b.cancel(999));
}

#[test]
fn art_many_inserts_trigger_inner_splits() {
    let mut b = Book::new();
    for i in 0..32 {
        b.insert_limit(HpOrder::limit(
            Side::Buy,
            1_000 + i as i64 * 257,
            1,
            i as u64,
        ));
    }
    assert_eq!(b.depth(Side::Buy, 5).len(), 5);
    assert!(b.best_bid().is_some());
}
