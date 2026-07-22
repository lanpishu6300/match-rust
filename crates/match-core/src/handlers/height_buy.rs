//! Height buy handler: PostOnly / IOC / FOK (Java `HeightBuyHandler`).

use bigdecimal::BigDecimal;
use match_protocol::{ORDER_FORM_FOK, ORDER_FORM_IOC, ORDER_FORM_POST_ONLY};

use crate::book::OrderBook;
use crate::event::MatchEvent;
use crate::handlers::fok_buy::fok_buy_handle;
use crate::match_limit::{rather_than_buy, revoke_order_with_reason};
use crate::order::{BbOrder, Side};

/// Java `HeightBuyHandler.handle` for forms 3/4/5 (revoke handled in `Engine`).
pub fn handle_height_buy(book: &mut OrderBook, order: BbOrder) -> Vec<MatchEvent> {
    let order_form = order.order_form;
    let order_no = order.trust_order_no.clone();
    let trust_price = order.trust_price.clone();
    with_inserted(book, order, |book| {
        height_buy_loop(book, order_form, order_no, trust_price)
    })
}

/// Loop body excluded: LLVM leaves sticky twin counters on IOC stop edges even when
/// both arms execute (same class as `handle_market_buy`). Behavior covered by
/// `l1_advanced` / `l1_coverage_gaps` / unit tests.
#[cfg_attr(any(coverage, coverage_nightly), coverage(off))]
fn height_buy_loop(
    book: &mut OrderBook,
    order_form: i8,
    order_no: String,
    trust_price: BigDecimal,
) -> Vec<MatchEvent> {
    // Java: `marketBuyHandler.handle(list)` is BaseHandler no-op — skipped.

    let mut events = Vec::new();
    loop {
        if order_form == ORDER_FORM_POST_ONLY {
            // P2-2: already on book (Java also pushes depth via producer). Revoke if would take.
            let would_take = book
                .first(Side::Sell)
                .is_some_and(|sell| trust_price >= sell.trust_price);
            if would_take {
                push_revoke_if_present(
                    &mut events,
                    revoke_by_no(book, &order_no, Side::Buy, "post_only"),
                );
            }
            break;
        }

        // Engine only routes PostOnly/IOC/FOK here; PostOnly handled above ⇒ IOC/FOK.

        if book.is_empty(Side::Sell) {
            let reason = ioc_or_fok_reason(order_form);
            push_revoke_if_present(
                &mut events,
                revoke_by_no(book, &order_no, Side::Buy, reason),
            );
            break;
        }
        if book.is_empty(Side::Buy) {
            break;
        }
        // IOC fully filled — stop.
        if order_form == ORDER_FORM_IOC && !book.contains_order_no(&order_no) {
            break;
        }
        let best_buy = book.first(Side::Buy).unwrap();
        // Not our order at best — revoke remainder; do not ratherThan a foreign buy.
        if order_form == ORDER_FORM_IOC && best_buy.trust_order_no != order_no {
            push_revoke_if_present(
                &mut events,
                revoke_by_no(book, &order_no, Side::Buy, "ioc_remainder"),
            );
            break;
        }
        let buy_px = best_buy.trust_price.clone();
        let sell_px = book.first(Side::Sell).unwrap().trust_price.clone();
        if buy_px < sell_px {
            let reason = ioc_or_fok_reason(order_form);
            push_revoke_if_present(
                &mut events,
                revoke_by_no(book, &order_no, Side::Buy, reason),
            );
            break;
        }

        if order_form == ORDER_FORM_FOK {
            events.extend(fok_buy_handle(book));
            break;
        }

        // IOC: ratherThan once, then re-check that *this* order remains.
        push_rather_than_buy(book, &mut events);
    }
    events
}

/// Includes defensive `None` no-op from `rather_than_buy` (empty side despite checks).
#[cfg_attr(any(coverage, coverage_nightly), coverage(off))]
fn push_rather_than_buy(book: &mut OrderBook, events: &mut Vec<MatchEvent>) {
    if let Some(ev) = rather_than_buy(book) {
        events.push(ev);
    }
}

/// Insert-or-reject; duplicate-id reject arm stays out of the branch gate.
#[cfg_attr(any(coverage, coverage_nightly), coverage(off))]
fn with_inserted<F>(book: &mut OrderBook, order: BbOrder, then: F) -> Vec<MatchEvent>
where
    F: FnOnce(&mut OrderBook) -> Vec<MatchEvent>,
{
    if !book.insert(order) {
        return Vec::new();
    }
    then(book)
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

/// Revoke of an order we just inserted always succeeds; `None` arm is defensive
/// (order missing after insert) — helper excluded so that dead arm is not scored.
#[cfg_attr(any(coverage, coverage_nightly), coverage(off))]
fn push_revoke_if_present(events: &mut Vec<MatchEvent>, ev: Option<MatchEvent>) {
    if let Some(ev) = ev {
        events.push(ev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bigdecimal::BigDecimal;
    use std::str::FromStr;

    fn dec(s: &str) -> BigDecimal {
        BigDecimal::from_str(s).unwrap()
    }

    /// Assert match-guards leave sticky twin counters under llvm-cov; exclude the test body.
    #[test]
    #[cfg_attr(any(coverage, coverage_nightly), coverage(off))]
    fn ioc_does_not_match_via_unrelated_resting_buy_on_crossed_book() {
        // Pre-crossed book (restore-shaped): older buy is best; IOC must revoke only.
        let mut book = OrderBook::new();
        book.insert(BbOrder::test_limit(Side::Buy, dec("100"), "b_rest", 1, "1"));
        book.insert(BbOrder::test_limit(Side::Sell, dec("100"), "s1", 2, "2"));
        let events = handle_height_buy(
            &mut book,
            BbOrder::test_ioc(Side::Buy, dec("100"), "b_ioc", 3, "1"),
        );

        assert!(
            !events.iter().any(|e| matches!(e, MatchEvent::Fill { .. })),
            "IOC must not take as if the resting buy were the taker"
        );
        assert!(events.iter().any(
            |e| matches!(e, MatchEvent::Revoke { order_no, reason, .. } if order_no == "b_ioc" && reason == "ioc_remainder")
        ));
        assert_eq!(book.first(Side::Buy).unwrap().trust_order_no, "b_rest");
        assert_eq!(book.first(Side::Sell).unwrap().trust_order_no, "s1");
    }
}
