//! Advanced order handlers (PostOnly / IOC / FOK) ported from Java Height*/Fok*.
//!
//! IOC loops break once the original IOC order is fully filled or no longer the
//! best on its side (fixes former Java quirk P0-2 that could match unrelated
//! resting orders after the IOC completed).

mod fok_buy;
mod fok_sell;
mod height_buy;
mod height_sell;

pub use height_buy::handle_height_buy;
pub use height_sell::handle_height_sell;
