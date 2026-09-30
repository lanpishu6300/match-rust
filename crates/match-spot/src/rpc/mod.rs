//! HTTP clients for startup restore.

pub mod market;
pub mod order;
mod response;

pub use market::{MarketClient, SpotCoinMarket};
pub use order::{build_mq_order_spot, EntrustListRow, OrderClient};
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_response_api_error() {
        let resp = ResponseData::<()> {
            code: 0,
            message: Some("fail".into()),
            data: None,
        };
        assert!(matches!(
            check_response(resp),
            Err(RpcError::Api { code: 0, .. })
        ));
    }

    #[test]
    fn check_response_missing_data() {
        let resp = ResponseData::<()> {
            code: 1,
            message: None,
            data: None,
        };
        assert!(matches!(check_response(resp), Err(RpcError::MissingData)));
    }

    #[test]
    fn check_response_ok() {
        let resp = ResponseData {
            code: 1,
            message: None,
            data: Some(42),
        };
        assert_eq!(check_response(resp).unwrap(), 42);
    }
}
