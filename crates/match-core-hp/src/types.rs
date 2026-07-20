/// Fixed-point scales for one trading symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymbolScale {
    pub price_scale: u32,
    pub qty_scale: u32,
}

/// Bid or ask side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Buy,
    Sell,
}

/// Resting / working order in tick/lot space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HpOrder {
    pub id: u64,
    pub side: Side,
    pub price_tick: i64,
    pub qty_lot: i64,
    pub open_lot: i64,
    pub ts: u64,
    pub client_id: u64,
}

impl HpOrder {
    pub fn limit(side: Side, price_tick: i64, qty_lot: i64, client_id: u64) -> Self {
        Self {
            id: 0,
            side,
            price_tick,
            qty_lot,
            open_lot: qty_lot,
            ts: 0,
            client_id,
        }
    }
}

/// Engine output events (hot path; no strings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HpEvent {
    Fill {
        maker_id: u64,
        taker_id: u64,
        maker_client_id: u64,
        taker_client_id: u64,
        price_tick: i64,
        qty_lot: i64,
        /// Maker open lot after this fill.
        maker_open_lot: i64,
        /// Taker open lot after this fill.
        taker_open_lot: i64,
    },
    Rest {
        id: u64,
        client_id: u64,
        side: Side,
        price_tick: i64,
        qty_lot: i64,
    },
    Revoke {
        id: u64,
        client_id: u64,
        /// 0 = user cancel, 1 = market leftover / gear stop.
        reason: u8,
    },
}

/// Inbound commands for [`crate::HpEngine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HpCommand {
    Limit {
        side: Side,
        price_tick: i64,
        qty_lot: i64,
        ts: u64,
        client_id: u64,
    },
    Cancel {
        id: u64,
    },
    Market {
        side: Side,
        qty_lot: i64,
        ts: u64,
        /// Max number of fills (aligned with match-core `gear`), `None` = unlimited.
        max_fills: Option<u32>,
        client_id: u64,
    },
}
