//! High-performance matching core (fixed-point, price-level book).
//! Not Java-equivalent; not used by match-contract by default.

mod book;
mod engine;
mod order_store;
mod scale;
mod types;

pub use book::Book;
pub use engine::HpEngine;
pub use order_store::OrderStore;
pub use scale::{from_lot, from_tick, to_lot, to_tick, ScaleError};
pub use types::{HpCommand, HpEvent, HpOrder, Side, SymbolScale};
