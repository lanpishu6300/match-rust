//! Advanced order handlers (PostOnly / IOC / FOK) ported from Java Height*/Fok*.
//!
//! IOC stops once the original order is fully filled or is no longer best on its
//! side, so the loop cannot continue matching unrelated resting orders.

mod fok_buy;
mod fok_sell;
mod height_buy;
mod height_sell;

pub use height_buy::handle_height_buy;
pub use height_sell::handle_height_sell;
