//! Limit-order matching ported from Java Buy/Sell/RatherThan/Equals/LessThan handlers.

use bigdecimal::{BigDecimal, Zero};
use match_protocol::{
    ORDER_FORM_FOK, ORDER_STATUS_REVOKE, ORDER_STATUS_SUCCESS, ORDER_STATUS_SUCCESS_PART,
};

use crate::book::OrderBook;
use crate::event::MatchEvent;
use crate::order::{BbOrder, Side};
use crate::price_utils::get_average_price;

pub(crate) fn remaining(order: &BbOrder) -> BigDecimal {
    &order.trust_number - &order.consumer_all_number
}

pub(crate) fn dec_str(d: &BigDecimal) -> String {
    d.normalized().to_string()
}

pub(crate) fn fill_event(
    symbol: &str,
    taker: &BbOrder,
    maker_order_no: &str,
    price: &BigDecimal,
    qty: &BigDecimal,
    taker_remaining: &BigDecimal,
    maker_remaining: &BigDecimal,
    taker_status: i8,
    maker_status: i8,
) -> MatchEvent {
    MatchEvent::Fill {
        symbol: symbol.to_string(),
        taker_order_no: taker.trust_order_no.clone(),
        maker_order_no: maker_order_no.to_string(),
        price: dec_str(price),
        qty: dec_str(qty),
        taker_remaining: dec_str(taker_remaining),
        maker_remaining: dec_str(maker_remaining),
        taker_status: taker_status as u8,
        maker_status: maker_status as u8,
    }
}

pub fn revoke_order(book: &mut OrderBook, order: &BbOrder) -> Option<MatchEvent> {
    revoke_order_with_reason(book, order, "user")
}

pub fn revoke_order_with_reason(
    book: &mut OrderBook,
    order: &BbOrder,
    reason: &str,
) -> Option<MatchEvent> {
    let side = Side::from_order_type(order.order_type)?;
    let removed = book.remove_by_order_no(side, &order.trust_order_no)?;
    Some(MatchEvent::Revoke {
        order_no: removed.trust_order_no.clone(),
        symbol: removed.symbol_key.clone(),
        remaining: dec_str(&removed.remaining_number),
        reason: reason.to_string(),
    })
}

/// Java `BuyHandler` limit path: add to buy book, then match while buy.first >= sell.first.
pub fn handle_limit_buy(book: &mut OrderBook, order: BbOrder) -> Vec<MatchEvent> {
    book.insert(order);
    let mut events = Vec::new();
    loop {
        if book.is_empty(Side::Buy) || book.is_empty(Side::Sell) {
            break;
        }
        let buy_px = book.first(Side::Buy).unwrap().trust_price.clone();
        let sell_px = book.first(Side::Sell).unwrap().trust_price.clone();
        if buy_px < sell_px {
            break;
        }
        if let Some(ev) = rather_than_buy(book) {
            events.push(ev);
        } else {
            break;
        }
    }
    events
}

/// Java `SellHandler` limit path: add to sell book, then match while sell.first <= buy.first.
pub fn handle_limit_sell(book: &mut OrderBook, order: BbOrder) -> Vec<MatchEvent> {
    book.insert(order);
    let mut events = Vec::new();
    loop {
        if book.is_empty(Side::Buy) || book.is_empty(Side::Sell) {
            break;
        }
        let buy_px = book.first(Side::Buy).unwrap().trust_price.clone();
        let sell_px = book.first(Side::Sell).unwrap().trust_price.clone();
        // Java: sell.first.compareTo(buy.first) > 0 → break
        if sell_px > buy_px {
            break;
        }
        match rather_than_sell(book) {
            RatherThanSellResult::Fill(ev) => events.push(ev),
            RatherThanSellResult::Revoked(ev) => {
                events.push(ev);
                break;
            }
            RatherThanSellResult::None => break,
        }
    }
    events
}

pub(crate) enum RatherThanSellResult {
    Fill(MatchEvent),
    Revoked(MatchEvent),
    None,
}

/// Java `RatherThanHandler.buyHandle`: buy remaining > sell → take sell; else LessThan.
pub(crate) fn rather_than_buy(book: &mut OrderBook) -> Option<MatchEvent> {
    let mut buy = book.pop_first(Side::Buy)?;
    let sell = book.pop_first(Side::Sell)?;
    let last_buy = remaining(&buy);
    let last_sell = remaining(&sell);
    let symbol = buy.symbol_key.clone();

    if last_buy > last_sell {
        // Fully fill sell (maker); partially fill buy (taker-side book first).
        buy.average_price = get_average_price(
            &buy.consumer_all_number,
            &buy.average_price,
            &last_sell,
            &sell.trust_price,
        );
        buy.consumer_all_number += &last_sell;
        buy.remaining_number = &buy.trust_number - &buy.consumer_all_number;
        buy.order_status = ORDER_STATUS_SUCCESS_PART;
        buy.current_deal_number = last_sell.clone();

        let deal_price = sell.trust_price.clone();
        let taker_rem = buy.remaining_number.clone();
        book.insert(buy.clone());

        Some(fill_event(
            &symbol,
            &buy,
            &sell.trust_order_no,
            &deal_price,
            &last_sell,
            &taker_rem,
            &BigDecimal::zero(),
            ORDER_STATUS_SUCCESS_PART,
            ORDER_STATUS_SUCCESS,
        ))
    } else if last_buy < last_sell {
        less_than_buy(book, buy, sell, last_buy, last_sell)
    } else {
        equals_buy(buy, sell, last_buy, last_sell)
    }
}

fn less_than_buy(
    book: &mut OrderBook,
    mut buy: BbOrder,
    mut sell: BbOrder,
    last_buy: BigDecimal,
    _last_sell: BigDecimal,
) -> Option<MatchEvent> {
    let symbol = buy.symbol_key.clone();
    buy.average_price = get_average_price(
        &buy.consumer_all_number,
        &buy.average_price,
        &last_buy,
        &sell.trust_price,
    );
    sell.consumer_all_number += &last_buy;
    sell.remaining_number = &sell.trust_number - &sell.consumer_all_number;
    book.insert(sell.clone());

    buy.order_status = ORDER_STATUS_SUCCESS;
    buy.current_deal_number = last_buy.clone();
    buy.consumer_all_number = buy.trust_number.clone();
    // Java LessThan buy path does not set remaining_number=0; emit logical 0 for Fill.
    let deal_price = sell.trust_price.clone();
    let maker_rem = sell.remaining_number.clone();

    Some(fill_event(
        &symbol,
        &buy,
        &sell.trust_order_no,
        &deal_price,
        &last_buy,
        &BigDecimal::zero(),
        &maker_rem,
        ORDER_STATUS_SUCCESS,
        ORDER_STATUS_SUCCESS_PART,
    ))
}

fn equals_buy(
    mut buy: BbOrder,
    sell: BbOrder,
    last_buy: BigDecimal,
    last_sell: BigDecimal,
) -> Option<MatchEvent> {
    let symbol = buy.symbol_key.clone();
    buy.average_price = get_average_price(
        &buy.consumer_all_number,
        &buy.average_price,
        &last_sell,
        &sell.trust_price,
    );
    buy.order_status = ORDER_STATUS_SUCCESS;
    buy.current_deal_number = last_buy.clone();
    buy.consumer_all_number = buy.trust_number.clone();
    buy.remaining_number = BigDecimal::zero();
    let deal_price = sell.trust_price.clone();

    Some(fill_event(
        &symbol,
        &buy,
        &sell.trust_order_no,
        &deal_price,
        &last_buy,
        &BigDecimal::zero(),
        &BigDecimal::zero(),
        ORDER_STATUS_SUCCESS,
        ORDER_STATUS_SUCCESS,
    ))
}

/// Java `RatherThanHandler.sellHandle`.
pub(crate) fn rather_than_sell(book: &mut OrderBook) -> RatherThanSellResult {
    let Some(mut sell) = book.pop_first(Side::Sell) else {
        return RatherThanSellResult::None;
    };
    let Some(buy) = book.pop_first(Side::Buy) else {
        book.insert(sell);
        return RatherThanSellResult::None;
    };
    let last_sell = remaining(&sell);
    let last_buy = remaining(&buy);
    let symbol = sell.symbol_key.clone();

    if last_sell > last_buy {
        if sell.order_form == ORDER_FORM_FOK {
            // Cannot fully fill FOK — revoke sell; restore buy.
            book.insert(buy);
            let rem = sell.remaining_number.clone();
            return RatherThanSellResult::Revoked(MatchEvent::Revoke {
                order_no: sell.trust_order_no.clone(),
                symbol,
                remaining: dec_str(&rem),
                reason: "user".to_string(),
            });
        }
        // Fully fill buy (maker); partially fill sell.
        sell.average_price = get_average_price(
            &sell.consumer_all_number,
            &sell.average_price,
            &last_buy,
            &buy.trust_price,
        );
        sell.consumer_all_number += &last_buy;
        sell.remaining_number = &sell.trust_number - &sell.consumer_all_number;
        sell.order_status = ORDER_STATUS_SUCCESS_PART;
        sell.current_deal_number = last_buy.clone();

        let deal_price = buy.trust_price.clone();
        let taker_rem = sell.remaining_number.clone();
        book.insert(sell.clone());

        RatherThanSellResult::Fill(fill_event(
            &symbol,
            &sell,
            &buy.trust_order_no,
            &deal_price,
            &last_buy,
            &taker_rem,
            &BigDecimal::zero(),
            ORDER_STATUS_SUCCESS_PART,
            ORDER_STATUS_SUCCESS,
        ))
    } else if last_sell < last_buy {
        RatherThanSellResult::Fill(less_than_sell(book, buy, sell, last_buy, last_sell))
    } else {
        RatherThanSellResult::Fill(equals_sell(buy, sell, last_buy, last_sell))
    }
}

fn less_than_sell(
    book: &mut OrderBook,
    mut buy: BbOrder,
    mut sell: BbOrder,
    _last_buy: BigDecimal,
    last_sell: BigDecimal,
) -> MatchEvent {
    let symbol = sell.symbol_key.clone();
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
    sell.consumer_all_number = sell.trust_number.clone();
    let deal_price = buy.trust_price.clone();
    let maker_rem = buy.remaining_number.clone();

    fill_event(
        &symbol,
        &sell,
        &buy.trust_order_no,
        &deal_price,
        &last_sell,
        &BigDecimal::zero(),
        &maker_rem,
        ORDER_STATUS_SUCCESS,
        ORDER_STATUS_SUCCESS_PART,
    )
}

fn equals_sell(
    buy: BbOrder,
    mut sell: BbOrder,
    _last_buy: BigDecimal,
    last_sell: BigDecimal,
) -> MatchEvent {
    let symbol = sell.symbol_key.clone();
    sell.average_price = get_average_price(
        &sell.consumer_all_number,
        &sell.average_price,
        &last_sell,
        &buy.trust_price,
    );
    sell.order_status = ORDER_STATUS_SUCCESS;
    sell.remaining_number = BigDecimal::zero();
    sell.current_deal_number = last_sell.clone();
    sell.consumer_all_number = sell.trust_number.clone();
    let deal_price = buy.trust_price.clone();

    fill_event(
        &symbol,
        &sell,
        &buy.trust_order_no,
        &deal_price,
        &last_sell,
        &BigDecimal::zero(),
        &BigDecimal::zero(),
        ORDER_STATUS_SUCCESS,
        ORDER_STATUS_SUCCESS,
    )
}

/// Rest-only path for unrecognized sides / forms.
pub fn rest_only(book: &mut OrderBook, order: BbOrder) {
    book.insert(order);
}

pub fn is_revoke(order: &BbOrder) -> bool {
    order.order_status == ORDER_STATUS_REVOKE
}
