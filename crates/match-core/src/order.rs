use bigdecimal::{BigDecimal, Zero};
use std::ops::{Deref, DerefMut};
use std::str::FromStr;

pub use match_protocol::BbOrder as ProtocolBbOrder;

/// Engine-facing order type (wraps protocol `BbOrder` for book helpers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BbOrder(pub ProtocolBbOrder);

impl Deref for BbOrder {
    type Target = ProtocolBbOrder;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for BbOrder {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Order side aligned with Java `orderType` (1 buy, 2 sell).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Buy = 1,
    Sell = 2,
}

impl Side {
    pub fn order_type(self) -> i8 {
        self as i8
    }

    pub fn from_order_type(order_type: i8) -> Option<Self> {
        match order_type {
            1 => Some(Side::Buy),
            2 => Some(Side::Sell),
            _ => None,
        }
    }
}

impl BbOrder {
    /// Builds a minimal limit order for tests and book ordering checks.
    pub fn test_limit(
        side: Side,
        price: BigDecimal,
        trust_order_no: &str,
        create_time: i64,
        qty: &str,
    ) -> Self {
        let qty = BigDecimal::from_str(qty).expect("valid test qty");
        Self(ProtocolBbOrder {
            user_id: 1,
            uid: 1,
            r#type: 1,
            order_type: side.order_type(),
            market_id: 1,
            coin_id: 1,
            symbol_key: "btcusdt".to_string(),
            coin_market: "BTC/USDT".to_string(),
            trust_order_no: trust_order_no.to_string(),
            order_form: 1,
            gear: None,
            close_position: 1,
            start_deposit: BigDecimal::zero(),
            target_rate: BigDecimal::zero(),
            position_type: 0,
            lever_times: 1,
            order_status: 0,
            consumer_all_number: BigDecimal::zero(),
            current_deal_number: BigDecimal::zero(),
            trust_number: qty.clone(),
            trust_price: price,
            remaining_number: qty,
            create_time,
            face_value: None,
            average_price: BigDecimal::zero(),
        })
    }

    /// Builds a minimal market order (`order_form = 2`) for tests. Default `gear = Some(1)`.
    pub fn test_market(
        side: Side,
        trust_order_no: &str,
        create_time: i64,
        qty: &str,
    ) -> Self {
        let mut o = Self::test_limit(side, BigDecimal::zero(), trust_order_no, create_time, qty);
        o.order_form = 2;
        o.gear = Some(1);
        o
    }

    /// PostOnly order (`order_form = 3`).
    pub fn test_post_only(
        side: Side,
        price: BigDecimal,
        trust_order_no: &str,
        create_time: i64,
        qty: &str,
    ) -> Self {
        let mut o = Self::test_limit(side, price, trust_order_no, create_time, qty);
        o.order_form = 3;
        o
    }

    /// IOC order (`order_form = 4`).
    pub fn test_ioc(
        side: Side,
        price: BigDecimal,
        trust_order_no: &str,
        create_time: i64,
        qty: &str,
    ) -> Self {
        let mut o = Self::test_limit(side, price, trust_order_no, create_time, qty);
        o.order_form = 4;
        o
    }

    /// FOK order (`order_form = 5`).
    pub fn test_fok(
        side: Side,
        price: BigDecimal,
        trust_order_no: &str,
        create_time: i64,
        qty: &str,
    ) -> Self {
        let mut o = Self::test_limit(side, price, trust_order_no, create_time, qty);
        o.order_form = 5;
        o
    }
}

/// Compare two buy orders for book ordering (mirrors Java `BBOrder.compareTo` for buys).
pub fn compare_buy(a: &BbOrder, b: &BbOrder) -> std::cmp::Ordering {
    match a.trust_price.cmp(&b.trust_price) {
        std::cmp::Ordering::Equal => compare_same_price(a, b),
        ord => ord.reverse(),
    }
}

/// Compare two sell orders for book ordering (mirrors Java `BBOrder.compareTo` for sells).
pub fn compare_sell(a: &BbOrder, b: &BbOrder) -> std::cmp::Ordering {
    match a.trust_price.cmp(&b.trust_price) {
        std::cmp::Ordering::Equal => compare_same_price(a, b),
        ord => ord,
    }
}

fn compare_same_price(a: &BbOrder, b: &BbOrder) -> std::cmp::Ordering {
    if a.trust_order_no == b.trust_order_no {
        std::cmp::Ordering::Equal
    } else {
        match a.create_time.cmp(&b.create_time) {
            std::cmp::Ordering::Equal => a.trust_order_no.cmp(&b.trust_order_no),
            ord => ord,
        }
    }
}
