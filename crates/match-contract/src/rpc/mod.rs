//! HTTP clients for startup restore (market list + entrust pagination).

pub mod market;
pub mod order;
mod response;

pub use market::{ContractCoinMarket, MarketClient};
pub use order::{EntrustListRow, OrderClient};
pub use response::ResponseData;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RpcError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("RPC returned code={code}: {message}")]
    Api { code: i32, message: String },
    #[error("RPC response missing data field")]
    MissingData,
}

pub(crate) fn check_response<T>(resp: ResponseData<T>) -> Result<T, RpcError> {
    if !resp.is_success() {
        return Err(RpcError::Api {
            code: resp.code,
            message: resp.message.unwrap_or_default(),
        });
    }
    resp.data.ok_or(RpcError::MissingData)
}
