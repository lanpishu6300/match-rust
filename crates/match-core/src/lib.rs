//! Pure matching engine (no MQ/Redis/HTTP).
#![cfg_attr(any(coverage, coverage_nightly), feature(coverage_attribute))]

mod book;
mod depth;
mod engine;
mod event;
mod handlers;
mod id;
mod match_limit;
mod match_market;
mod order;
mod price_utils;

pub use book::OrderBook;
pub use engine::Engine;
pub use event::MatchEvent;
pub use id::{AtomicU64IdGenerator, IdGenerator};
pub use order::{compare_buy, compare_sell, BbOrder, Side};
