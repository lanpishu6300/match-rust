//! Boundary conversion between protocol/`BbOrder` decimals and HP tick/lot commands.
//! Not used on the hot path.

use crate::scale::{to_lot, to_tick, ScaleError};
use crate::types::{HpCommand, Side, SymbolScale};
use bigdecimal::BigDecimal;
use match_protocol::{
    BbOrder, ORDER_FORM_LIMIT, ORDER_FORM_MARKET_PRICE, ORDER_STATUS_REVOKE, ORDER_TYPE_BUY,
    ORDER_TYPE_SELL,
};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AdapterError {
    #[error("scale conversion: {0}")]
    Scale(#[from] ScaleError),
    #[error("unsupported order side")]
    UnsupportedSide,
    #[error("unsupported order form")]
    UnsupportedForm,
    #[error("invalid trust_order_no as id")]
    InvalidOrderId,
    #[error("create_time must be non-negative")]
    InvalidTimestamp,
    #[error("market gear must be >= 1")]
    InvalidGear,
}

/// Convert a protocol [`BbOrder`] into an [`HpCommand`] using symbol scales.
///
/// Supports limit, market, and cancel (`order_status == REVOKE`).
pub fn from_bb_order(o: &BbOrder, scale: &SymbolScale) -> Result<HpCommand, AdapterError> {
    if o.order_status == ORDER_STATUS_REVOKE {
        let id = parse_id(&o.trust_order_no)?;
        return Ok(HpCommand::Cancel { id });
    }

    let side = match o.order_type {
        ORDER_TYPE_BUY => Side::Buy,
        ORDER_TYPE_SELL => Side::Sell,
        _ => return Err(AdapterError::UnsupportedSide),
    };
    let ts = u64::try_from(o.create_time).map_err(|_| AdapterError::InvalidTimestamp)?;
    let client_id = parse_id(&o.trust_order_no)?;
    let qty_lot = decimal_to_lot(scale, &o.remaining_number)?;

    match o.order_form {
        ORDER_FORM_LIMIT => {
            let price_tick = decimal_to_tick(scale, &o.trust_price)?;
            Ok(HpCommand::Limit {
                side,
                price_tick,
                qty_lot,
                ts,
                client_id,
            })
        }
        ORDER_FORM_MARKET_PRICE => {
            let gear = o
                .gear
                .filter(|g| *g >= 1)
                .ok_or(AdapterError::InvalidGear)?;
            Ok(HpCommand::Market {
                side,
                qty_lot,
                ts,
                max_fills: Some(gear as u32),
                client_id,
            })
        }
        _ => Err(AdapterError::UnsupportedForm),
    }
}

fn parse_id(s: &str) -> Result<u64, AdapterError> {
    s.parse().map_err(|_| AdapterError::InvalidOrderId)
}

fn decimal_to_tick(scale: &SymbolScale, d: &BigDecimal) -> Result<i64, AdapterError> {
    Ok(to_tick(scale, &d.normalized().to_string())?)
}

fn decimal_to_lot(scale: &SymbolScale, d: &BigDecimal) -> Result<i64, AdapterError> {
    Ok(to_lot(scale, &d.normalized().to_string())?)
}
