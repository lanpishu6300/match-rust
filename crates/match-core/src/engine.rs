use std::collections::HashMap;

use bigdecimal::BigDecimal;
use match_protocol::{
    ORDER_FORM_FOK, ORDER_FORM_IOC, ORDER_FORM_MARKET_PRICE, ORDER_FORM_POST_ONLY,
};

use crate::book::OrderBook;
use crate::event::MatchEvent;
use crate::handlers::{handle_height_buy, handle_height_sell};
use crate::match_limit::{handle_limit_buy, handle_limit_sell, is_revoke, rest_only, revoke_order};
use crate::match_market::{handle_market_buy, handle_market_sell};
use crate::order::{BbOrder, Side};

/// Per-symbol matching engine facade.
#[derive(Debug, Default)]
pub struct Engine {
    books: HashMap<String, OrderBook>,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Accept an incoming order: revoke, market, height (PostOnly/IOC/FOK), or limit-match.
    pub fn on_order(&mut self, order: BbOrder) -> Vec<MatchEvent> {
        let symbol = order.symbol_key.clone();
        let book = self.books.entry(symbol).or_insert_with(OrderBook::new);

        if is_revoke(&order) {
            return revoke_order(book, &order).into_iter().collect();
        }

        if is_height_order_form(order.order_form) {
            return match Side::from_order_type(order.order_type) {
                Some(Side::Buy) => handle_height_buy(book, order),
                Some(Side::Sell) => handle_height_sell(book, order),
                None => {
                    rest_only(book, order);
                    Vec::new()
                }
            };
        }

        if order.order_form == ORDER_FORM_MARKET_PRICE {
            return match Side::from_order_type(order.order_type) {
                Some(Side::Buy) => handle_market_buy(book, order),
                Some(Side::Sell) => handle_market_sell(book, order),
                None => {
                    rest_only(book, order);
                    Vec::new()
                }
            };
        }

        match Side::from_order_type(order.order_type) {
            Some(Side::Buy) => handle_limit_buy(book, order),
            Some(Side::Sell) => handle_limit_sell(book, order),
            None => {
                rest_only(book, order);
                Vec::new()
            }
        }
    }

    /// Aggregated depth for `symbol` and `side`: best prices first, qty summed per level.
    pub fn depth_levels(
        &self,
        symbol: &str,
        side: Side,
        limit: usize,
    ) -> Vec<(BigDecimal, BigDecimal)> {
        self.books
            .get(symbol)
            .map(|book| book.depth_levels(side, limit))
            .unwrap_or_default()
    }
}

fn is_height_order_form(order_form: i8) -> bool {
    matches!(
        order_form,
        ORDER_FORM_POST_ONLY | ORDER_FORM_IOC | ORDER_FORM_FOK
    )
}
