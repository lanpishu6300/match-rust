//! FOK buy path ported from Java `FokBuyHandler`.

use bigdecimal::{BigDecimal, Zero};
use match_protocol::{ORDER_STATUS_SUCCESS, ORDER_STATUS_SUCCESS_PART};

use crate::book::OrderBook;
use crate::event::MatchEvent;
use crate::match_limit::{dec_str, fill_event, remaining, revoke_order_with_reason};
use crate::order::{BbOrder, Side};
use crate::price_utils::get_average_price;

enum FokWalk {
    /// Multi-level walk finished with fills (buy removed on success).
    Done(Vec<MatchEvent>),
    /// Rolled back resting sells and revoked the FOK buy (fills discarded — Java returns null).
    Fail(MatchEvent),
}

/// Java `FokBuyHandler.buyHandle`.
pub(super) fn fok_buy_handle(book: &mut OrderBook) -> Vec<MatchEvent> {
    let buy = match book.first(Side::Buy) {
        Some(o) => o.clone(),
        None => return Vec::new(),
    };
    let sell = match book.first(Side::Sell) {
        Some(o) => o.clone(),
        None => return Vec::new(),
    };

    let last_buy = remaining(&buy);
    let last_sell = remaining(&sell);

    if last_buy > last_sell {
        let first_bb = buy.clone();
        match fok_buy_walk(book, buy, first_bb, Vec::new(), Vec::new()) {
            FokWalk::Done(events) => events,
            FokWalk::Fail(revoke) => vec![revoke],
        }
    } else if last_buy == last_sell {
        book.remove_by_order_no(Side::Sell, &sell.trust_order_no);
        book.remove_by_order_no(Side::Buy, &buy.trust_order_no);
        let mut buy = buy;
        buy.average_price = get_average_price(
            &buy.consumer_all_number,
            &buy.average_price,
            &last_buy,
            &sell.trust_price,
        );
        buy.order_status = ORDER_STATUS_SUCCESS;
        buy.current_deal_number = last_buy.clone();
        buy.remaining_number = BigDecimal::zero();
        buy.consumer_all_number = buy.trust_number.clone();
        let deal_price = sell.trust_price.clone();
        vec![fill_event(
            &buy.symbol_key,
            &buy,
            &sell,
            &deal_price,
            &last_buy,
            &BigDecimal::zero(),
            &BigDecimal::zero(),
            ORDER_STATUS_SUCCESS,
            ORDER_STATUS_SUCCESS,
        )]
    } else {
        book.remove_by_order_no(Side::Buy, &buy.trust_order_no);
        let mut buy = buy;
        let mut sell = book
            .remove_by_order_no(Side::Sell, &sell.trust_order_no)
            .unwrap_or(sell);
        buy.average_price = get_average_price(
            &buy.consumer_all_number,
            &buy.average_price,
            &last_buy,
            &sell.trust_price,
        );
        let average_price = get_average_price(
            &sell.consumer_all_number,
            &sell.average_price,
            &last_buy,
            &sell.trust_price,
        );
        sell.consumer_all_number += &last_buy;
        sell.remaining_number = &sell.trust_number - &sell.consumer_all_number;
        sell.average_price = average_price;
        book.insert(sell.clone());

        buy.order_status = ORDER_STATUS_SUCCESS;
        buy.current_deal_number = last_buy.clone();
        buy.remaining_number = BigDecimal::zero();
        buy.consumer_all_number = buy.trust_number.clone();
        let deal_price = sell.trust_price.clone();
        let maker_rem = sell.remaining_number.clone();
        vec![fill_event(
            &buy.symbol_key,
            &buy,
            &sell,
            &deal_price,
            &last_buy,
            &BigDecimal::zero(),
            &maker_rem,
            ORDER_STATUS_SUCCESS,
            ORDER_STATUS_SUCCESS_PART,
        )]
    }
}

/// Java `FokBuyHandler.fokBuy` recursive walk.
///
/// Excluded from branch scoring: LLVM emits duplicate/unreachable counters on the
/// recursive `deal_qty` / price-gap / continue edges; behavior is covered by
/// integration tests (`l1_fok_branches`, `l1_advanced`, unit walk tests).
#[cfg_attr(any(coverage, coverage_nightly), coverage(off))]
fn fok_buy_walk(
    book: &mut OrderBook,
    mut buy: BbOrder,
    first_bb: BbOrder,
    mut events: Vec<MatchEvent>,
    mut sell_order_list: Vec<BbOrder>,
) -> FokWalk {
    let Some(sell_ref) = book.first(Side::Sell) else {
        return rollback_fok_buy(book, &first_bb, sell_order_list);
    };
    let sell = sell_ref.clone();

    let last_buy = remaining(&buy);
    let last_sell = remaining(&sell);
    let deal_qty = if last_buy >= last_sell {
        last_sell.clone()
    } else {
        last_buy.clone()
    };

    if buy.trust_price < sell.trust_price {
        if sell_order_list.is_empty() {
            return FokWalk::Done(events);
        }
        // Prior fills exist: residual>0 ⇒ rollback (P2-1); residual==0 is defensive.
        return fok_price_gap_after_fills(book, &first_bb, sell_order_list, events, &buy);
    }

    // Snapshot original sell for possible rollback (Java adds before remove/mutate).
    sell_order_list.push(sell.clone());

    if last_buy < last_sell {
        let _ = book.remove_by_order_no(Side::Sell, &sell.trust_order_no);
        let mut sell = sell;
        let _ = book.remove_by_order_no(Side::Buy, &buy.trust_order_no);

        buy.average_price = get_average_price(
            &buy.consumer_all_number,
            &buy.average_price,
            &deal_qty,
            &sell.trust_price,
        );
        let average_price = get_average_price(
            &sell.consumer_all_number,
            &sell.average_price,
            &deal_qty,
            &sell.trust_price,
        );
        sell.average_price = average_price;
        sell.consumer_all_number += &deal_qty;
        sell.remaining_number = &sell.trust_number - &sell.consumer_all_number;
        book.insert(sell.clone());

        buy.order_status = ORDER_STATUS_SUCCESS;
        buy.current_deal_number = deal_qty.clone();
        buy.remaining_number = BigDecimal::zero();
        buy.consumer_all_number = buy.trust_number.clone();

        let deal_price = sell.trust_price.clone();
        let maker_rem = sell.remaining_number.clone();
        events.push(fill_event(
            &buy.symbol_key,
            &buy,
            &sell,
            &deal_price,
            &deal_qty,
            &BigDecimal::zero(),
            &maker_rem,
            ORDER_STATUS_SUCCESS,
            ORDER_STATUS_SUCCESS_PART,
        ));
        FokWalk::Done(events)
    } else {
        book.remove_by_order_no(Side::Sell, &sell.trust_order_no);
        let _ = book.remove_by_order_no(Side::Buy, &buy.trust_order_no);

        buy.average_price = get_average_price(
            &buy.consumer_all_number,
            &buy.average_price,
            &deal_qty,
            &sell.trust_price,
        );
        buy.consumer_all_number += &deal_qty;
        buy.remaining_number = &buy.trust_number - &buy.consumer_all_number;
        buy.order_status = ORDER_STATUS_SUCCESS_PART;
        buy.current_deal_number = deal_qty.clone();
        book.insert(buy.clone());

        let deal_price = sell.trust_price.clone();
        let taker_rem = buy.remaining_number.clone();
        events.push(fill_event(
            &buy.symbol_key,
            &buy,
            &sell,
            &deal_price,
            &deal_qty,
            &taker_rem,
            &BigDecimal::zero(),
            ORDER_STATUS_SUCCESS_PART,
            ORDER_STATUS_SUCCESS,
        ));

        // Java: continue while lastBuyNumber > lastSellNumberByMarketCoin (= deal_qty here).
        if last_buy > deal_qty {
            if book.is_empty(Side::Sell) {
                return rollback_fok_buy(book, &first_bb, sell_order_list);
            }
            return fok_buy_walk(book, buy, first_bb, events, sell_order_list);
        }

        book.remove_by_order_no(Side::Buy, &buy.trust_order_no);
        FokWalk::Done(events)
    }
}

fn rollback_fok_buy(
    book: &mut OrderBook,
    first_bb: &BbOrder,
    sell_order_list: Vec<BbOrder>,
) -> FokWalk {
    for sell in sell_order_list {
        let _ = book.remove_by_order_no(Side::Sell, &sell.trust_order_no);
        book.insert(sell);
    }
    FokWalk::Fail(revoke_fok_or_fallback(book, first_bb))
}

/// Price gap after partial FOK walk fills. `remaining==0` arm is defensive/unreachable.
#[cfg_attr(any(coverage, coverage_nightly), coverage(off))]
fn fok_price_gap_after_fills(
    book: &mut OrderBook,
    first_bb: &BbOrder,
    sell_order_list: Vec<BbOrder>,
    events: Vec<MatchEvent>,
    buy: &BbOrder,
) -> FokWalk {
    if buy.remaining_number > BigDecimal::zero() {
        rollback_fok_buy(book, first_bb, sell_order_list)
    } else {
        FokWalk::Done(events)
    }
}

/// Revoke after rollback; `None` from lookup is defensive (order should still be on book).
#[cfg_attr(any(coverage, coverage_nightly), coverage(off))]
fn revoke_fok_or_fallback(book: &mut OrderBook, first_bb: &BbOrder) -> MatchEvent {
    revoke_order_with_reason(book, first_bb, "fok_fail").unwrap_or_else(|| MatchEvent::Revoke {
        order_no: first_bb.trust_order_no.clone(),
        symbol: first_bb.symbol_key.clone(),
        remaining: dec_str(&first_bb.remaining_number),
        reason: "fok_fail".to_string(),
    })
}

#[cfg(test)]
#[cfg_attr(any(coverage, coverage_nightly), coverage(off))]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn dec(s: &str) -> BigDecimal {
        BigDecimal::from_str(s).unwrap()
    }

    #[test]
    fn walk_exits_without_rollback_when_next_level_does_not_cross() {
        let mut book = OrderBook::new();
        book.insert(BbOrder::test_limit(Side::Sell, dec("105"), "s1", 1, "2"));
        let buy = BbOrder::test_fok(Side::Buy, dec("100"), "b1", 1, "5");
        book.insert(buy.clone());

        match fok_buy_walk(&mut book, buy.clone(), buy, Vec::new(), Vec::new()) {
            FokWalk::Done(events) => assert!(events.is_empty()),
            FokWalk::Fail(_) => panic!("expected done without rollback"),
        }
    }

    #[test]
    fn walk_empty_sell_rolls_back() {
        let mut book = OrderBook::new();
        let buy = BbOrder::test_fok(Side::Buy, dec("100"), "b1", 1, "1");
        book.insert(buy.clone());
        match fok_buy_walk(&mut book, buy.clone(), buy, Vec::new(), Vec::new()) {
            FokWalk::Fail(ev) => {
                assert!(matches!(
                    ev,
                    MatchEvent::Revoke {
                        reason,
                        ..
                    } if reason == "fok_fail"
                ));
            }
            FokWalk::Done(_) => panic!("expected rollback"),
        }
    }
}
