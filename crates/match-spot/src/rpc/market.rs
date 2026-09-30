use super::{check_response, ResponseData, RpcError};
use serde::Deserialize;

const MARKET_LIST_PATH: &str = "/market/coinMarkets";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotCoinMarket {
    pub coin_market: Option<String>,
    pub symbol_key: Option<String>,
    pub main_stream: Option<i32>,
}

pub fn list_url(base: &str) -> String {
    join_base_path(base, MARKET_LIST_PATH)
}

fn join_base_path(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    format!("{base}{path}")
}

pub struct MarketClient {
    http: reqwest::Client,
    base_url: String,
}

impl MarketClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
        }
    }

    pub async fn fetch_markets(&self) -> Result<Vec<SpotCoinMarket>, RpcError> {
        let url = list_url(&self.base_url);
        let resp = self.http.post(url).send().await?;
        let body: ResponseData<Vec<SpotCoinMarket>> = resp.json().await?;
        check_response(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_url_joins_base() {
        assert_eq!(
            list_url("http://market"),
            "http://market/market/coinMarkets"
        );
    }
}
