use super::{check_response, ResponseData, RpcError};
use serde::Deserialize;

const MARKET_LIST_PATH: &str = "/contract-market/contractcoinMarketList";

/// Contract coin market row from restore RPC (subset of Java `USDTContractCoinMarketVO`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractCoinMarket {
    pub coin_market: Option<String>,
    pub origin_coin_market: Option<String>,
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

    pub async fn fetch_markets(&self) -> Result<Vec<ContractCoinMarket>, RpcError> {
        let url = list_url(&self.base_url);
        let resp = self.http.post(url).send().await?;
        let body: ResponseData<Vec<ContractCoinMarket>> = resp.json().await?;
        check_response(body)
    }
}
