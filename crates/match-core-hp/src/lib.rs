//! High-performance matching core (fixed-point, price-level book).
//!
//! Not Java-equivalent; not used by `match-contract` by default.
//! Design lineage and OSS practice mapping: `docs/best-practices.md`.

pub mod adapter;
pub mod affinity;
#[cfg(feature = "art")]
mod art_index;
mod book;
mod engine;
mod level;
mod level_index;
mod order_store;
mod scale;
mod spsc;
mod types;
mod worker;

pub use affinity::{pin_current_thread, AffinityError};
pub use book::Book;
pub use engine::HpEngine;
pub use order_store::OrderStore;
pub use scale::{from_lot, from_tick, to_lot, to_tick, ScaleError};
pub use spsc::{Busy, SpscRing};
pub use types::{HpCommand, HpEvent, HpOrder, Side, SymbolScale};
pub use worker::{HpWorker, WaitStrategy};
