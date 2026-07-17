//! FOK sell path ported from Java `FokSellHandler`.

use bigdecimal::{BigDecimal, Zero};
use match_protocol::{ORDER_STATUS_SUCCESS, ORDER_STATUS_SUCCESS_PART};

use crate::book::OrderBook;
use crate::event::MatchEvent;
use crate::match_limit::{dec_str, fill_event, remaining, revoke_order_with_reason};
use crate::order::{BbOrder, Side};
use crate::price_utils::get_average_price;

enum FokWalk {
    Done(Vec<MatchEvent>),
    Fail(MatchEvent),
}

/// Java `FokSellHandler.sellHandle`.
pub(super) fn fok_sell_handle(book: &mut OrderBook) -> Vec<MatchEvent> {
    let buy = match book.first(Side::Buy) {
        Some(o) => o.clone(),
        None => return Vec::new(),
    };
    let sell = match book.first(Side::Sell) {
        Some(o) => o.clone(),
        None => return Vec::new(),
    };

    let last_sell = remaining(&sell);
    let last_buy = remaining(&buy);
    // Java: `lastSellNumberByMarketCoin = lastBuyNumber` (buy remaining).
    let last_buy_for_cmp = last_buy.clone();

    if last_sell > last_buy_for_cmp {
        let first_bb = sell.clone();
        match fok_sell_walk(book, sell, first_bb, Vec::new(), Vec::new()) {
            FokWalk::Done(events) => events,
            FokWalk::Fail(revoke) => vec![revoke],
        }
    } else if last_sell == last_buy_for_cmp {
        book.remove_by_order_no(Side::Sell, &sell.trust_order_no);
        book.remove_by_order_no(Side::Buy, &buy.trust_order_no);
        let mut sell = sell;
        sell.average_price = get_average_price(
            &sell.consumer_all_number,
            &sell.average_price,
            &last_sell,
            &buy.trust_price,
        );
        sell.order_status = ORDER_STATUS_SUCCESS;
        sell.current_deal_number = last_sell.clone();
        sell.remaining_number = BigDecimal::zero();
        // Java parity quirk: setConsumerAllNumber(buyOrder.getTrustNumber()).
        sell.consumer_all_number = buy.trust_number.clone();
        let deal_price = buy.trust_price.clone();
        vec![fill_event(
            &sell.symbol_key,
            &sell,
            &buy.trust_order_no,
            &deal_price,
            &last_sell,
            &BigDecimal::zero(),
            &BigDecimal::zero(),
            ORDER_STATUS_SUCCESS,
            ORDER_STATUS_SUCCESS,
        )]
    } else {
        // last_sell < last_buy: fully fill sell, leave partial buy.
        book.remove_by_order_no(Side::Sell, &sell.trust_order_no);
        let mut sell = sell;
        let mut buy = book
            .remove_by_order_no(Side::Buy, &buy.trust_order_no)
            .unwrap_or(buy);
        sell.average_price = get_average_price(
            &sell.consumer_all_number,
            &sell.average_price,
            &last_sell,
            &buy.trust_price,
        );
        let average_price = get_average_price(
            &buy.consumer_all_number,
            &buy.average_price,
            &last_sell,
            &buy.trust_price,
        );
        buy.consumer_all_number += &last_sell;
        buy.remaining_number = &buy.trust_number - &buy.consumer_all_number;
        buy.average_price = average_price;
        book.insert(buy.clone());

        sell.order_status = ORDER_STATUS_SUCCESS;
        sell.current_deal_number = last_sell.clone();
        sell.remaining_number = BigDecimal::zero();
        sell.consumer_all_number = sell.trust_number.clone();
        let deal_price = buy.trust_price.clone();
        let maker_rem = buy.remaining_number.clone();
        vec![fill_event(
            &sell.symbol_key,
            &sell,
            &buy.trust_order_no,
            &deal_price,
            &last_sell,
            &BigDecimal::zero(),
            &maker_rem,
            ORDER_STATUS_SUCCESS,
            ORDER_STATUS_SUCCESS_PART,
        )]
    }
}

/// Java `FokSellHandler.fokSell` recursive walk.
fn fok_sell_walk(
    book: &mut OrderBook,
    mut sell: BbOrder,
    first_bb: BbOrder,
    mut events: Vec<MatchEvent>,
    mut buy_order_list: Vec<BbOrder>,
) -> FokWalk {
    let Some(buy_ref) = book.first(Side::Buy) else {
        return rollback_fok_sell(book, &first_bb, buy_order_list);
    };
    let buy = buy_ref.clone();

    // Java variable names are swapped vs intuition:
    // lastBuyNumber = sell remaining; lastSellNumber = buy remaining.
    let last_sell_rem = remaining(&sell);
    let last_buy_rem = remaining(&buy);
    let deal_qty = if last_sell_rem >= last_buy_rem {
        last_buy_rem.clone()
    } else {
        last_sell_rem.clone()
    };

    if sell.trust_price > buy.trust_price {
        if sell.remaining_number > BigDecimal::zero() && !buy_order_list.is_empty() {
            return rollback_fok_sell(book, &first_bb, buy_order_list);
        }
        return FokWalk::Done(events);
    }

    buy_order_list.push(buy.clone());

    if last_sell_rem < last_buy_rem {
        // Fully fill sell against partial buy.
        let mut buy = book
            .remove_by_order_no(Side::Buy, &buy.trust_order_no)
            .unwrap_or(buy);
        let _ = book.remove_by_order_no(Side::Sell, &sell.trust_order_no);

        sell.average_price = get_average_price(
            &sell.consumer_all_number,
            &sell.average_price,
            &deal_qty,
            &buy.trust_price,
        );
        let average_price = get_average_price(
            &buy.consumer_all_number,
            &buy.average_price,
            &deal_qty,
            &buy.trust_price,
        );
        buy.consumer_all_number += &deal_qty;
        buy.remaining_number = &buy.trust_number - &buy.consumer_all_number;
        buy.average_price = average_price;
        book.insert(buy.clone());

        sell.order_status = ORDER_STATUS_SUCCESS;
        sell.current_deal_number = deal_qty.clone();
        sell.remaining_number = BigDecimal::zero();
        // Java: setConsumerAllNumber(buyOrder.getTrustNumber()) in this branch.
        sell.consumer_all_number = buy.trust_number.clone();

        let deal_price = buy.trust_price.clone();
        let maker_rem = buy.remaining_number.clone();
        events.push(fill_event(
            &sell.symbol_key,
            &sell,
            &buy.trust_order_no,
            &deal_price,
            &deal_qty,
            &BigDecimal::zero(),
            &maker_rem,
            ORDER_STATUS_SUCCESS,
            ORDER_STATUS_SUCCESS_PART,
        ));
        FokWalk::Done(events)
    } else {
        book.remove_by_order_no(Side::Buy, &buy.trust_order_no);
        let _ = book.remove_by_order_no(Side::Sell, &sell.trust_order_no);

        sell.average_price = get_average_price(
            &sell.consumer_all_number,
            &sell.average_price,
            &deal_qty,
            &buy.trust_price,
        );
        sell.consumer_all_number += &deal_qty;
        sell.remaining_number = &sell.trust_number - &sell.consumer_all_number;
        sell.order_status = ORDER_STATUS_SUCCESS_PART;
        sell.current_deal_number = deal_qty.clone();
        book.insert(sell.clone());

        let deal_price = buy.trust_price.clone();
        let taker_rem = sell.remaining_number.clone();
        events.push(fill_event(
            &sell.symbol_key,
            &sell,
            &buy.trust_order_no,
            &deal_price,
            &deal_qty,
            &taker_rem,
            &BigDecimal::zero(),
            ORDER_STATUS_SUCCESS_PART,
            ORDER_STATUS_SUCCESS,
        ));

        if last_sell_rem > deal_qty {
            if book.is_empty(Side::Buy) {
                return rollback_fok_sell(book, &first_bb, buy_order_list);
            }
            return fok_sell_walk(book, sell, first_bb, events, buy_order_list);
        }

        book.remove_by_order_no(Side::Sell, &sell.trust_order_no);
        FokWalk::Done(events)
    }
}

fn rollback_fok_sell(
    book: &mut OrderBook,
    first_bb: &BbOrder,
    buy_order_list: Vec<BbOrder>,
) -> FokWalk {
    for buy in buy_order_list {
        let _ = book.remove_by_order_no(Side::Buy, &buy.trust_order_no);
        book.insert(buy);
    }
    let revoke = revoke_order_with_reason(book, first_bb, "fok_fail").unwrap_or_else(|| {
        MatchEvent::Revoke {
            order_no: first_bb.trust_order_no.clone(),
            symbol: first_bb.symbol_key.clone(),
            remaining: dec_str(&first_bb.remaining_number),
            reason: "fok_fail".to_string(),
        }
    });
    FokWalk::Fail(revoke)
}
