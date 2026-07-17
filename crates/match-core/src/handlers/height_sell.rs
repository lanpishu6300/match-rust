//! Height sell handler: PostOnly / IOC / FOK (Java `HeightSellHandler`).
//!
//! Intentional Java parity: see module docs on [`crate::handlers`] for IOC loop quirk P0-2.

use bigdecimal::BigDecimal;
use match_protocol::{ORDER_FORM_FOK, ORDER_FORM_IOC, ORDER_FORM_POST_ONLY};

use crate::book::OrderBook;
use crate::event::MatchEvent;
use crate::handlers::fok_sell::fok_sell_handle;
use crate::match_limit::{rather_than_sell, revoke_order_with_reason, RatherThanSellResult};
use crate::order::{BbOrder, Side};

/// Java `HeightSellHandler.handle` for forms 3/4/5 (revoke handled in `Engine`).
pub fn handle_height_sell(book: &mut OrderBook, order: BbOrder) -> Vec<MatchEvent> {
    let order_form = order.order_form;
    let order_no = order.trust_order_no.clone();
    let trust_price = order.trust_price.clone();

    book.insert(order);

    let mut events = Vec::new();
    loop {
        if order_form == ORDER_FORM_POST_ONLY {
            // P2-2: on book first; revoke if would take (sell price <= best buy).
            let would_take = book
                .first(Side::Buy)
                .is_some_and(|buy| trust_price <= buy.trust_price);
            if would_take {
                if let Some(ev) = revoke_by_no(book, &order_no, Side::Sell, "post_only") {
                    events.push(ev);
                }
            }
            break;
        }

        if order_form == ORDER_FORM_IOC || order_form == ORDER_FORM_FOK {
            if book.is_empty(Side::Buy) {
                let reason = ioc_or_fok_reason(order_form);
                if let Some(ev) = revoke_by_no(book, &order_no, Side::Sell, reason) {
                    events.push(ev);
                }
                break;
            }
            if book.is_empty(Side::Sell) {
                break;
            }
            let sell_px = book.first(Side::Sell).unwrap().trust_price.clone();
            let buy_px = book.first(Side::Buy).unwrap().trust_price.clone();
            // Java: sell.first.compareTo(buy.first) > 0 → revoke remainder
            if sell_px > buy_px {
                let reason = ioc_or_fok_reason(order_form);
                if let Some(ev) = revoke_by_no(book, &order_no, Side::Sell, reason) {
                    events.push(ev);
                }
                break;
            }

            if order_form == ORDER_FORM_FOK {
                events.extend(fok_sell_handle(book));
                break;
            }

            // IOC: ratherThan once, then continue (P0-2).
            match rather_than_sell(book) {
                RatherThanSellResult::Fill(ev) => events.push(ev),
                RatherThanSellResult::Revoked(ev) => {
                    events.push(ev);
                    break;
                }
                RatherThanSellResult::None => break,
            }
            continue;
        }

        break;
    }
    events
}

fn ioc_or_fok_reason(order_form: i8) -> &'static str {
    if order_form == ORDER_FORM_IOC {
        "ioc_remainder"
    } else {
        "fok_fail"
    }
}

fn revoke_by_no(book: &mut OrderBook, order_no: &str, side: Side, reason: &str) -> Option<MatchEvent> {
    let mut stub = BbOrder::test_limit(side, BigDecimal::from(0), order_no, 0, "0");
    stub.order_type = side.order_type();
    revoke_order_with_reason(book, &stub, reason)
}
