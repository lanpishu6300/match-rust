//! Advanced order handlers (PostOnly / IOC / FOK) ported from Java Height*/Fok*.
//!
//! # Intentional Java parity
//!
//! IOC matching loops intentionally retain the Java control-flow quirk documented as
//! **P0-2** in `docs/合约撮合已知问题梳理.md`: after each `ratherThan` fill the loop
//! neither checks whether the original IOC order is still on the book nor breaks on
//! full fill — it continues matching `book.first()` until the book no longer crosses
//! (or a side empties). That can match unrelated resting orders after the IOC is done.
//!
//! See also P2-2 (PostOnly add-then-revoke flash) and P2-1 (FOK multi-level rollback).

mod fok_buy;
mod fok_sell;
mod height_buy;
mod height_sell;

pub use height_buy::handle_height_buy;
pub use height_sell::handle_height_sell;
