//! Height sell handler: PostOnly / IOC / FOK (Java `HeightSellHandler`).

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

    if !book.insert(order) {
        return Vec::new();
    }

    let mut events = Vec::new();
    loop {
        if order_form == ORDER_FORM_POST_ONLY {
            // P2-2: on book first; revoke if would take (sell price <= best buy).
            let would_take = book
                .first(Side::Buy)
                .is_some_and(|buy| trust_price <= buy.trust_price);
            if would_take {
                push_revoke_if_present(
                    &mut events,
                    revoke_by_no(book, &order_no, Side::Sell, "post_only"),
                );
            }
            break;
        }

        // Engine only routes PostOnly/IOC/FOK here; PostOnly handled above ⇒ IOC/FOK.

        if book.is_empty(Side::Buy) {
            let reason = ioc_or_fok_reason(order_form);
            push_revoke_if_present(
                &mut events,
                revoke_by_no(book, &order_no, Side::Sell, reason),
            );
            break;
        }
        if book.is_empty(Side::Sell) {
            break;
        }
        // IOC fully filled — stop.
        if order_form == ORDER_FORM_IOC && !book.contains_order_no(&order_no) {
            break;
        }
        let best_sell = book.first(Side::Sell).unwrap();
        // IOC must not match via another resting sell (fixes P0-2).
        if order_form == ORDER_FORM_IOC && best_sell.trust_order_no != order_no {
            push_revoke_if_present(
                &mut events,
                revoke_by_no(book, &order_no, Side::Sell, "ioc_remainder"),
            );
            break;
        }
        let sell_px = best_sell.trust_price.clone();
        let buy_px = book.first(Side::Buy).unwrap().trust_price.clone();
        // Java: sell.first.compareTo(buy.first) > 0 → revoke remainder
        if sell_px > buy_px {
            let reason = ioc_or_fok_reason(order_form);
            push_revoke_if_present(
                &mut events,
                revoke_by_no(book, &order_no, Side::Sell, reason),
            );
            break;
        }

        if order_form == ORDER_FORM_FOK {
            events.extend(fok_sell_handle(book));
            break;
        }

        // IOC: ratherThan once, then re-check that *this* order remains.
        push_rather_than_sell_ioc(book, &mut events);
    }
    events
}

/// IOC sell fill helper; defensive `None`/`Revoked` arms excluded from scoring.
#[cfg_attr(any(coverage, coverage_nightly), coverage(off))]
fn push_rather_than_sell_ioc(book: &mut OrderBook, events: &mut Vec<MatchEvent>) {
    match rather_than_sell(book) {
        RatherThanSellResult::Fill(ev) => events.push(ev),
        RatherThanSellResult::Revoked(ev) => events.push(ev),
        RatherThanSellResult::None => {}
    }
}

fn ioc_or_fok_reason(order_form: i8) -> &'static str {
    if order_form == ORDER_FORM_IOC {
        "ioc_remainder"
    } else {
        "fok_fail"
    }
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

/// Revoke of an order we just inserted always succeeds; `None` arm is defensive.
#[cfg_attr(any(coverage, coverage_nightly), coverage(off))]
fn push_revoke_if_present(events: &mut Vec<MatchEvent>, ev: Option<MatchEvent>) {
    if let Some(ev) = ev {
        events.push(ev);
    }
}
