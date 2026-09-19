use super::{check_response, ResponseData, RpcError};
use match_protocol::MqOrder;
use serde::{Deserialize, Serialize};

const ENTRUST_LIST_PATH: &str = "/order/entrust-list";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntrustListRow {
    pub user_id: Option<i32>,
    pub uid: Option<i32>,
    pub user_type: Option<i32>,
    pub r#type: Option<i32>,
    pub market_id: Option<i32>,
    pub coin_id: Option<i32>,
    pub symbol_key: Option<String>,
    pub coin_market: Option<String>,
    pub entrust_no: Option<String>,
    pub price_type: Option<i32>,
    pub gear: Option<i32>,
    pub status: Option<i32>,
    pub remain_amount: Option<String>,
    pub price: Option<String>,
    pub create_time: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntrustPagination {
    pub rows: Option<Vec<EntrustListRow>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EntrustListRequest {
    r#type: i32,
    main_stream: i32,
    page: i32,
    page_size: i32,
}

pub fn entrust_list_url(base: &str) -> String {
    join_base_path(base, ENTRUST_LIST_PATH)
}

fn join_base_path(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    format!("{base}{path}")
}

/// Map `EntrustListBO` → `MqOrder` (`InitLoadData.buildMqOrder`).
pub fn build_mq_order_spot(row: &EntrustListRow) -> MqOrder {
    MqOrder {
        user_id: row.user_id,
        uid: row.uid,
        c_type: 1,
        deal_type: None,
        r#type: row.user_type.map(|v| v as i8),
        order_type: row.r#type.map(|v| v as i8),
        market_id: row.market_id,
        coin_id: row.coin_id,
        symbol_key: row.symbol_key.clone(),
        coin_market: row.coin_market.clone(),
        trust_order_no: row.entrust_no.clone(),
        close_position: None,
        start_deposit: None,
        position_type: None,
        taker_rate: None,
        order_status: row.status.map(|v| v as i8),
        order_form: row.price_type.map(|v| v as i8),
        gear: row.gear,
        lever_times: None,
        trust_number: row.remain_amount.clone(),
        trust_price: row.price.clone(),
        create_time: row.create_time,
        face_value: None,
        handicap_type: None,
    }
}

pub struct OrderClient {
    http: reqwest::Client,
    base_url: String,
}

impl OrderClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
        }
    }

    pub async fn fetch_entrust_page(
        &self,
        side: i32,
        main_stream: i32,
        page: i32,
        page_size: i32,
    ) -> Result<Vec<EntrustListRow>, RpcError> {
        let url = entrust_list_url(&self.base_url);
        let body = EntrustListRequest {
            r#type: side,
            main_stream,
            page,
            page_size,
        };
        let resp = self.http.post(url).json(&body).send().await?;
        let page: ResponseData<EntrustPagination> = resp.json().await?;
        let pagination = check_response(page)?;
        Ok(pagination.rows.unwrap_or_default())
    }

    /// Paginate buy (1) and sell (2) sides until empty pages.
    pub async fn fetch_all_entrusts(&self, main_stream: i32) -> Result<Vec<EntrustListRow>, RpcError> {
        let mut all = Vec::new();
        for side in [1_i32, 2] {
            let mut page = 1;
            loop {
                let rows = self.fetch_entrust_page(side, main_stream, page, 100).await?;
                if rows.is_empty() {
                    break;
                }
                all.extend(rows);
                page += 1;
            }
        }
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_mq_order_maps_entrust_fields() {
        let row = EntrustListRow {
            user_id: Some(1),
            uid: Some(2),
            user_type: Some(1),
            r#type: Some(1),
            market_id: Some(10),
            coin_id: Some(20),
            symbol_key: Some("btcusdt".into()),
            coin_market: Some("BTC/USDT".into()),
            entrust_no: Some("E001".into()),
            price_type: Some(1),
            gear: Some(0),
            status: Some(0),
            remain_amount: Some("1.5".into()),
            price: Some("50000".into()),
            create_time: Some(1_700_000_000),
        };
        let mq = build_mq_order_spot(&row);
        assert_eq!(mq.trust_order_no.as_deref(), Some("E001"));
        assert_eq!(mq.order_form, Some(1));
        assert_eq!(mq.r#type, Some(1));
    }
}
