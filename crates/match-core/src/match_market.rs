//! Market-order matching ported from Java BuyHandler/SellHandler market branches
//! and MarketBuyHandler (gear stop; preserve P0-3 gear=0 behavior).

use bigdecimal::BigDecimal;
use match_protocol::ORDER_FORM_MARKET_PRICE;

use crate::book::OrderBook;
use crate::event::MatchEvent;
use crate::match_limit::{
    rather_than_buy, rather_than_sell, revoke_order_with_reason, RatherThanSellResult,
};
use crate::order::{BbOrder, Side};

/// Java `new BigDecimal(Integer.MAX_VALUE)` for market buy trust price.
const MARKET_BUY_TRUST_PRICE: i32 = i32::MAX;

/// Java `MarketBuyHandler.buyHandle`: skip when best sell is also market.
fn market_buy_handle(book: &mut OrderBook) -> Option<MatchEvent> {
    let sell = book.first(Side::Sell)?;
    if sell.order_form == ORDER_FORM_MARKET_PRICE {
        return None;
    }
    rather_than_buy(book)
}

fn gear_of(order: &BbOrder) -> i32 {
    // Java NPE if null; validation requires Some for market. Treat None as 0 (P0-3).
    order.gear.unwrap_or(0)
}

fn revoke_by_no(
    book: &mut OrderBook,
    order_no: &str,
    side: Side,
    reason: &str,
) -> Option<MatchEvent> {
    let mut stub = BbOrder::test_limit(side, BigDecimal::from(0), order_no, 0, "0");
    stub.order_type = side.order_type();
    revoke_order_with_reason(book, &stub, reason)
}

/// Java `BuyHandler` market path: MAX price, rest, match until gear / empty / filled.
pub fn handle_market_buy(book: &mut OrderBook, mut order: BbOrder) -> Vec<MatchEvent> {
    order.trust_price = BigDecimal::from(MARKET_BUY_TRUST_PRICE);
    let gear = gear_of(&order);
    let order_no = order.trust_order_no.clone();
    book.insert(order);

    let mut events = Vec::new();
    let mut fill_count: i32 = 0;
    loop {
        if book.is_empty(Side::Buy) {
            break;
        }
        if book.is_empty(Side::Sell) {
            if let Some(ev) = revoke_by_no(book, &order_no, Side::Buy, "market_empty") {
                events.push(ev);
            }
            break;
        }
        let best_buy = book.first(Side::Buy).unwrap();
        if best_buy.order_form != ORDER_FORM_MARKET_PRICE {
            // Fully filled (best is no longer a market order).
            break;
        }
        if best_buy.trust_order_no != order_no {
            break;
        }

        let made_progress = match market_buy_handle(book) {
            Some(ev) => {
                events.push(ev);
                fill_count += 1;
                true
            }
            None => false,
        };

        // Java: `bbOrders.size() >= bbOrder.getGear()` after each attempt (P0-3: gear=0 ⇒ always).
        if fill_count >= gear {
            if let Some(ev) = revoke_by_no(book, &order_no, Side::Buy, "market_gear") {
                events.push(ev);
            }
            break;
        }

        // Java would `continue` forever when match returns null and gear > size; stop instead.
        if !made_progress {
            break;
        }
    }
    events
}

/// Java `SellHandler` market path: rest, match via ratherThan sell until gear / empty / filled.
pub fn handle_market_sell(book: &mut OrderBook, order: BbOrder) -> Vec<MatchEvent> {
    let gear = gear_of(&order);
    let order_no = order.trust_order_no.clone();
    book.insert(order);

    let mut events = Vec::new();
    let mut fill_count: i32 = 0;
    loop {
        if book.is_empty(Side::Sell) {
            break;
        }
        if book.is_empty(Side::Buy) {
            if let Some(ev) = revoke_by_no(book, &order_no, Side::Sell, "market_empty") {
                events.push(ev);
            }
            break;
        }
        let best_sell = book.first(Side::Sell).unwrap();
        if best_sell.order_form != ORDER_FORM_MARKET_PRICE {
            break;
        }
        if best_sell.trust_order_no != order_no {
            break;
        }

        let made_progress = match rather_than_sell(book) {
            RatherThanSellResult::Fill(ev) => {
                events.push(ev);
                fill_count += 1;
                true
            }
            RatherThanSellResult::Revoked(ev) => {
                events.push(ev);
                return events;
            }
            RatherThanSellResult::None => false,
        };

        if fill_count >= gear {
            if let Some(ev) = revoke_by_no(book, &order_no, Side::Sell, "market_gear") {
                events.push(ev);
            }
            break;
        }

        if !made_progress {
            break;
        }
    }
    events
}
