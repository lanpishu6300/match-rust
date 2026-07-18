//! GoldenTrace NDJSON line types and engine replay.
//!
//! # Input order format
//!
//! `op: "input"` uses a **simplified camelCase order shape** (not full `MqOrder`):
//!
//! ```json
//! {
//!   "orderType": 2,
//!   "trustPrice": "100",
//!   "trustNumber": "1",
//!   "trustOrderNo": "s1",
//!   "createTime": 1,
//!   "orderForm": 1,
//!   "symbolKey": "btcusdt",
//!   "orderStatus": 0,
//!   "gear": 1
//! }
//! ```
//!
//! Required: `orderType`, `trustPrice`, `trustNumber`, `trustOrderNo`, `createTime`.
//! Optional: `orderForm` (default 1), `symbolKey` (default `btcusdt`), `orderStatus`
//! (default 0; use 3 for user revoke), `gear` (market orders).
//!
//! Orders are built via `BbOrder::test_*` helpers (limit / market / post-only / IOC / FOK).
//! Full `MqOrder` + `type_convert` is deferred: market orders with `trustPrice=0` fail
//! `type_convert`'s positive-price check, so simplified helpers are the practical path for L1/L2.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::str::FromStr;

use bigdecimal::BigDecimal;
use match_core::{BbOrder, Engine, MatchEvent, Side};
use match_protocol::NO_DEAL_NUMBER;
use serde::{Deserialize, Serialize};

/// One NDJSON GoldenTrace line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op")]
pub enum TraceLine {
    #[serde(rename = "input")]
    Input { seq: u64, order: SimplifiedOrder },
    #[serde(rename = "fill")]
    Fill {
        seq: u64,
        #[serde(rename = "takerOrderNo")]
        taker_order_no: String,
        #[serde(rename = "makerOrderNo")]
        maker_order_no: String,
        price: String,
        qty: String,
        #[serde(rename = "takerRemaining")]
        taker_remaining: String,
        #[serde(rename = "makerRemaining")]
        maker_remaining: String,
        #[serde(rename = "takerStatus")]
        taker_status: u8,
        #[serde(rename = "makerStatus")]
        maker_status: u8,
    },
    #[serde(rename = "depth")]
    Depth {
        seq: u64,
        symbol: String,
        bids: Vec<[String; 2]>,
        asks: Vec<[String; 2]>,
    },
    #[serde(rename = "revoke")]
    Revoke {
        seq: u64,
        #[serde(rename = "orderNo")]
        order_no: String,
        remaining: String,
        reason: String,
    },
}

/// Simplified order used by golden `input` ops (see module docs).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SimplifiedOrder {
    pub order_type: i8,
    pub trust_price: String,
    pub trust_number: String,
    pub trust_order_no: String,
    pub create_time: i64,
    #[serde(default = "default_order_form")]
    pub order_form: i8,
    #[serde(default = "default_symbol_key")]
    pub symbol_key: String,
    #[serde(default)]
    pub order_status: i8,
    #[serde(default)]
    pub gear: Option<i32>,
}

fn default_order_form() -> i8 {
    1
}

fn default_symbol_key() -> String {
    "btcusdt".to_string()
}

impl SimplifiedOrder {
    /// Build an engine `BbOrder` via test helpers.
    pub fn to_bb_order(&self) -> Result<BbOrder, String> {
        let side = Side::from_order_type(self.order_type)
            .ok_or_else(|| format!("invalid orderType {}", self.order_type))?;
        let price =
            BigDecimal::from_str(&self.trust_price).map_err(|e| format!("trustPrice: {e}"))?;

        let mut order = match self.order_form {
            2 => BbOrder::test_market(
                side,
                &self.trust_order_no,
                self.create_time,
                &self.trust_number,
            ),
            3 => BbOrder::test_post_only(
                side,
                price,
                &self.trust_order_no,
                self.create_time,
                &self.trust_number,
            ),
            4 => BbOrder::test_ioc(
                side,
                price,
                &self.trust_order_no,
                self.create_time,
                &self.trust_number,
            ),
            5 => BbOrder::test_fok(
                side,
                price,
                &self.trust_order_no,
                self.create_time,
                &self.trust_number,
            ),
            _ => BbOrder::test_limit(
                side,
                price,
                &self.trust_order_no,
                self.create_time,
                &self.trust_number,
            ),
        };

        order.symbol_key = self.symbol_key.clone();
        order.order_status = self.order_status;
        if let Some(gear) = self.gear {
            order.gear = Some(gear);
        }
        Ok(order)
    }
}

/// Fill or revoke event used for ordered diff (seq ignored).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutcomeEvent {
    Fill {
        taker_order_no: String,
        maker_order_no: String,
        price: String,
        qty: String,
        taker_remaining: String,
        maker_remaining: String,
        taker_status: u8,
        maker_status: u8,
    },
    Revoke {
        order_no: String,
        remaining: String,
        reason: String,
    },
}

impl OutcomeEvent {
    pub fn from_match_event(ev: MatchEvent) -> Self {
        match ev {
            MatchEvent::Fill {
                taker_order_no,
                maker_order_no,
                price,
                qty,
                taker_remaining,
                maker_remaining,
                taker_status,
                maker_status,
                ..
            } => Self::Fill {
                taker_order_no,
                maker_order_no,
                price,
                qty,
                taker_remaining,
                maker_remaining,
                taker_status,
                maker_status,
            },
            MatchEvent::Revoke {
                order_no,
                remaining,
                reason,
                ..
            } => Self::Revoke {
                order_no,
                remaining,
                reason,
            },
        }
    }

    pub fn from_trace_line(line: &TraceLine) -> Option<Self> {
        match line {
            TraceLine::Fill {
                taker_order_no,
                maker_order_no,
                price,
                qty,
                taker_remaining,
                maker_remaining,
                taker_status,
                maker_status,
                ..
            } => Some(Self::Fill {
                taker_order_no: taker_order_no.clone(),
                maker_order_no: maker_order_no.clone(),
                price: price.clone(),
                qty: qty.clone(),
                taker_remaining: taker_remaining.clone(),
                maker_remaining: maker_remaining.clone(),
                taker_status: *taker_status,
                maker_status: *maker_status,
            }),
            TraceLine::Revoke {
                order_no,
                remaining,
                reason,
                ..
            } => Some(Self::Revoke {
                order_no: order_no.clone(),
                remaining: remaining.clone(),
                reason: reason.clone(),
            }),
            _ => None,
        }
    }
}

/// Load NDJSON GoldenTrace lines from a file.
pub fn load_ndjson(path: &Path) -> Result<Vec<TraceLine>, String> {
    let file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let reader = BufReader::new(file);
    let mut lines = Vec::new();
    for (i, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| format!("read {}: {e}", path.display()))?;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let op: TraceLine = serde_json::from_str(trimmed)
            .map_err(|e| format!("{}:{}: {e}", path.display(), i + 1))?;
        lines.push(op);
    }
    Ok(lines)
}

fn dec_str(d: &BigDecimal) -> String {
    d.normalized().to_string()
}

/// Snapshot aggregated depth for `symbol` (best-first, up to `NO_DEAL_NUMBER` levels).
pub fn snapshot_depth(eng: &Engine, symbol: &str) -> (Vec<[String; 2]>, Vec<[String; 2]>) {
    let bids = eng
        .depth_levels(symbol, Side::Buy, NO_DEAL_NUMBER as usize)
        .into_iter()
        .map(|(p, q)| [dec_str(&p), dec_str(&q)])
        .collect();
    let asks = eng
        .depth_levels(symbol, Side::Sell, NO_DEAL_NUMBER as usize)
        .into_iter()
        .map(|(p, q)| [dec_str(&p), dec_str(&q)])
        .collect();
    (bids, asks)
}

/// Result of replaying inputs and collecting expected outcomes / depth checks.
#[derive(Debug, Default)]
pub struct ReplayCollected {
    pub actual_outcomes: Vec<OutcomeEvent>,
    pub expected_outcomes: Vec<OutcomeEvent>,
    /// Expected depth lines paired with the book snapshot taken when the line is reached.
    pub depth_checks: Vec<DepthCheck>,
}

#[derive(Debug)]
pub struct DepthCheck {
    pub seq: u64,
    pub symbol: String,
    pub expected_bids: Vec<[String; 2]>,
    pub expected_asks: Vec<[String; 2]>,
    pub actual_bids: Vec<[String; 2]>,
    pub actual_asks: Vec<[String; 2]>,
}

/// Walk `ops` in order: apply inputs to the engine; collect fill/revoke expected lines;
/// when a depth line appears, snapshot the book at that point.
pub fn collect_replay(ops: &[TraceLine]) -> Result<ReplayCollected, String> {
    let mut eng = Engine::new();
    let mut out = ReplayCollected::default();

    for line in ops {
        match line {
            TraceLine::Input { order, seq } => {
                let bb = order
                    .to_bb_order()
                    .map_err(|e| format!("seq {seq} input: {e}"))?;
                for ev in eng.on_order(bb) {
                    out.actual_outcomes.push(OutcomeEvent::from_match_event(ev));
                }
            }
            TraceLine::Fill { .. } | TraceLine::Revoke { .. } => {
                if let Some(ev) = OutcomeEvent::from_trace_line(line) {
                    out.expected_outcomes.push(ev);
                }
            }
            TraceLine::Depth {
                seq,
                symbol,
                bids,
                asks,
            } => {
                let (actual_bids, actual_asks) = snapshot_depth(&eng, symbol);
                out.depth_checks.push(DepthCheck {
                    seq: *seq,
                    symbol: symbol.clone(),
                    expected_bids: bids.clone(),
                    expected_asks: asks.clone(),
                    actual_bids,
                    actual_asks,
                });
            }
        }
    }

    Ok(out)
}

/// Replay a golden file (inputs + expected in the same NDJSON).
pub fn replay_path(path: &Path) -> Result<ReplayCollected, String> {
    let ops = load_ndjson(path)?;
    collect_replay(&ops)
}

/// Replay inputs from `input_path`; expected fill/depth/revoke from `expected_path`
/// (may be the same file). Depth lines are applied against the engine state after all
/// inputs that appear *before* that depth line in `expected_path` when files differ —
/// when `input_path == expected_path`, use [`replay_path`] instead.
///
/// For a separate expected file, inputs are applied first in order, then depth checks
/// use the final book (depth lines after all inputs). Prefer interleaved same-file goldens.
pub fn replay_paths(input_path: &Path, expected_path: &Path) -> Result<ReplayCollected, String> {
    if input_path == expected_path {
        return replay_path(input_path);
    }

    let inputs = load_ndjson(input_path)?;
    let expected = load_ndjson(expected_path)?;

    let mut eng = Engine::new();
    let mut out = ReplayCollected::default();

    for line in &inputs {
        if let TraceLine::Input { order, seq } = line {
            let bb = order
                .to_bb_order()
                .map_err(|e| format!("seq {seq} input: {e}"))?;
            for ev in eng.on_order(bb) {
                out.actual_outcomes.push(OutcomeEvent::from_match_event(ev));
            }
        }
    }

    for line in &expected {
        match line {
            TraceLine::Fill { .. } | TraceLine::Revoke { .. } => {
                if let Some(ev) = OutcomeEvent::from_trace_line(line) {
                    out.expected_outcomes.push(ev);
                }
            }
            TraceLine::Depth {
                seq,
                symbol,
                bids,
                asks,
            } => {
                let (actual_bids, actual_asks) = snapshot_depth(&eng, symbol);
                out.depth_checks.push(DepthCheck {
                    seq: *seq,
                    symbol: symbol.clone(),
                    expected_bids: bids.clone(),
                    expected_asks: asks.clone(),
                    actual_bids,
                    actual_asks,
                });
            }
            TraceLine::Input { .. } => {}
        }
    }

    Ok(out)
}
