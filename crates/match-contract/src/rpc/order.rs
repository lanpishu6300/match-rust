use super::{check_response, ResponseData, RpcError};
use bigdecimal::BigDecimal;
use match_protocol::MqOrder;
use serde::{Deserialize, Serialize};

const ENTRUST_LIST_PATH: &str = "/contract/entrust-list";

/// Entrust row from restore RPC (aligned with Java `USDTContractEntrustListBO`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntrustListRow {
    pub user_id: Option<i32>,
    pub uid: Option<i32>,
    #[serde(default)]
    pub c_type: i8,
    pub r#type: Option<i8>,
    pub order_type: Option<i8>,
    pub market_id: Option<i32>,
    pub coin_id: Option<i32>,
    pub symbol_key: Option<String>,
    pub coin_market: Option<String>,
    pub trust_order_no: Option<String>,
    pub close_position: Option<i8>,
    pub start_deposit: Option<String>,
    pub position_type: Option<i8>,
    pub taker_rate: Option<String>,
    pub order_status: Option<i8>,
    pub order_form: Option<i8>,
    pub gear: Option<i32>,
    pub lever_times: Option<i32>,
    pub trust_number: Option<String>,
    pub trust_price: Option<String>,
    pub create_time: Option<i64>,
    pub face_value: Option<BigDecimal>,
}

/// Paginated entrust list (`Pagination<USDTContractEntrustListBO>`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntrustPagination {
    pub rows: Option<Vec<EntrustListRow>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EntrustListRequest {
    trust_order_no: String,
    main_stream: i32,
}

pub fn entrust_list_url(base: &str) -> String {
    join_base_path(base, ENTRUST_LIST_PATH)
}

fn join_base_path(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    format!("{base}{path}")
}

/// Map entrust row to inbound `MqOrder`, matching Java `InitLoadData.buildMqOrder`.
pub fn build_mq_order(row: &EntrustListRow) -> MqOrder {
    MqOrder {
        user_id: row.user_id,
        uid: row.uid,
        c_type: row.c_type,
        deal_type: None,
        r#type: row.r#type,
        order_type: row.order_type,
        market_id: row.market_id,
        coin_id: row.coin_id,
        symbol_key: row.symbol_key.clone(),
        coin_market: row.coin_market.clone(),
        trust_order_no: row.trust_order_no.clone(),
        close_position: row.close_position,
        start_deposit: row.start_deposit.clone(),
        position_type: row.position_type,
        taker_rate: row.taker_rate.clone(),
        order_status: row.order_status,
        order_form: row.order_form,
        gear: row.gear,
        lever_times: row.lever_times,
        trust_number: row.trust_number.clone(),
        trust_price: row.trust_price.clone(),
        create_time: row.create_time,
        face_value: row.face_value.clone(),
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
        trust_order_no: &str,
        main_stream: i32,
    ) -> Result<Vec<EntrustListRow>, RpcError> {
        let url = entrust_list_url(&self.base_url);
        let body = EntrustListRequest {
            trust_order_no: trust_order_no.to_string(),
            main_stream,
        };
        let resp = self.http.post(url).json(&body).send().await?;
        let page: ResponseData<EntrustPagination> = resp.json().await?;
        let pagination = check_response(page)?;
        Ok(pagination.rows.unwrap_or_default())
    }

    /// Paginate entrust restore until an empty page (cursor = max trust order no seen).
    pub async fn fetch_all_entrusts(
        &self,
        initial_trust_order_no: &str,
        main_stream: i32,
    ) -> Result<Vec<EntrustListRow>, RpcError> {
        let mut cursor = initial_trust_order_no.to_string();
        let mut all = Vec::new();

        loop {
            let rows = self.fetch_entrust_page(&cursor, main_stream).await?;
            if rows.is_empty() {
                break;
            }
            if let Some(last) = rows.last().and_then(|r| r.trust_order_no.clone()) {
                cursor = last;
            }
            all.extend(rows);
        }

        Ok(all)
    }
}
