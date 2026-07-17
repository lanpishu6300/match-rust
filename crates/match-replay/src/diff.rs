//! Compare expected vs actual GoldenTrace outcomes (numeric decimals, ignore key order / seq).

use std::str::FromStr;

use bigdecimal::BigDecimal;

use crate::trace::{DepthCheck, OutcomeEvent, ReplayCollected};

/// Diff two decimal strings numerically (`"100"` == `"100.0"` == `"1E2"`).
pub fn decimals_equal(a: &str, b: &str) -> bool {
    match (BigDecimal::from_str(a.trim()), BigDecimal::from_str(b.trim())) {
        (Ok(da), Ok(db)) => da == db,
        _ => a == b,
    }
}

fn levels_equal(expected: &[[String; 2]], actual: &[[String; 2]]) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    expected.iter().zip(actual.iter()).all(|(e, a)| {
        decimals_equal(&e[0], &a[0]) && decimals_equal(&e[1], &a[1])
    })
}

fn outcome_equal(expected: &OutcomeEvent, actual: &OutcomeEvent) -> Result<(), String> {
    match (expected, actual) {
        (
            OutcomeEvent::Fill {
                taker_order_no: et,
                maker_order_no: em,
                price: ep,
                qty: eq,
                taker_remaining: etr,
                maker_remaining: emr,
                taker_status: ets,
                maker_status: ems,
            },
            OutcomeEvent::Fill {
                taker_order_no: at,
                maker_order_no: am,
                price: ap,
                qty: aq,
                taker_remaining: atr,
                maker_remaining: amr,
                taker_status: ats,
                maker_status: ams,
            },
        ) => {
            let mut errs = Vec::new();
            if et != at {
                errs.push(format!("takerOrderNo expected={et} actual={at}"));
            }
            if em != am {
                errs.push(format!("makerOrderNo expected={em} actual={am}"));
            }
            if !decimals_equal(ep, ap) {
                errs.push(format!("price expected={ep} actual={ap}"));
            }
            if !decimals_equal(eq, aq) {
                errs.push(format!("qty expected={eq} actual={aq}"));
            }
            if !decimals_equal(etr, atr) {
                errs.push(format!("takerRemaining expected={etr} actual={atr}"));
            }
            if !decimals_equal(emr, amr) {
                errs.push(format!("makerRemaining expected={emr} actual={amr}"));
            }
            if ets != ats {
                errs.push(format!("takerStatus expected={ets} actual={ats}"));
            }
            if ems != ams {
                errs.push(format!("makerStatus expected={ems} actual={ams}"));
            }
            if errs.is_empty() {
                Ok(())
            } else {
                Err(errs.join("; "))
            }
        }
        (
            OutcomeEvent::Revoke {
                order_no: eo,
                remaining: er,
                reason: ereason,
            },
            OutcomeEvent::Revoke {
                order_no: ao,
                remaining: ar,
                reason: areason,
            },
        ) => {
            let mut errs = Vec::new();
            if eo != ao {
                errs.push(format!("orderNo expected={eo} actual={ao}"));
            }
            if !decimals_equal(er, ar) {
                errs.push(format!("remaining expected={er} actual={ar}"));
            }
            if ereason != areason {
                errs.push(format!("reason expected={ereason} actual={areason}"));
            }
            if errs.is_empty() {
                Ok(())
            } else {
                Err(errs.join("; "))
            }
        }
        (OutcomeEvent::Fill { .. }, OutcomeEvent::Revoke { .. }) => {
            Err("kind expected=fill actual=revoke".into())
        }
        (OutcomeEvent::Revoke { .. }, OutcomeEvent::Fill { .. }) => {
            Err("kind expected=revoke actual=fill".into())
        }
    }
}

/// Compare ordered fill/revoke lists. Returns human-readable mismatch messages (empty = match).
pub fn diff_outcomes(expected: &[OutcomeEvent], actual: &[OutcomeEvent]) -> Vec<String> {
    let mut diffs = Vec::new();
    let n = expected.len().max(actual.len());
    for i in 0..n {
        match (expected.get(i), actual.get(i)) {
            (Some(e), Some(a)) => {
                if let Err(msg) = outcome_equal(e, a) {
                    diffs.push(format!("outcome[{i}]: {msg}"));
                }
            }
            (Some(e), None) => {
                diffs.push(format!("outcome[{i}]: missing actual for expected {e:?}"));
            }
            (None, Some(a)) => {
                diffs.push(format!("outcome[{i}]: unexpected actual {a:?}"));
            }
            (None, None) => {}
        }
    }
    diffs
}

fn diff_depth_check(check: &DepthCheck) -> Option<String> {
    let bids_ok = levels_equal(&check.expected_bids, &check.actual_bids);
    let asks_ok = levels_equal(&check.expected_asks, &check.actual_asks);
    if bids_ok && asks_ok {
        return None;
    }
    Some(format!(
        "depth seq={} symbol={}: bids expected={:?} actual={:?}; asks expected={:?} actual={:?}",
        check.seq,
        check.symbol,
        check.expected_bids,
        check.actual_bids,
        check.expected_asks,
        check.actual_asks
    ))
}

/// Full replay diff: outcome sequence + depth snapshots.
pub fn diff_replay(collected: &ReplayCollected) -> Vec<String> {
    let mut diffs = diff_outcomes(&collected.expected_outcomes, &collected.actual_outcomes);
    for check in &collected.depth_checks {
        if let Some(msg) = diff_depth_check(check) {
            diffs.push(msg);
        }
    }
    diffs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_equal_ignores_scale() {
        assert!(decimals_equal("100", "100.0"));
        assert!(decimals_equal("1", "1.00"));
        assert!(!decimals_equal("100", "101"));
    }

    #[test]
    fn outcome_fill_decimal_match() {
        let e = OutcomeEvent::Fill {
            taker_order_no: "b1".into(),
            maker_order_no: "s1".into(),
            price: "100".into(),
            qty: "1.0".into(),
            taker_remaining: "0".into(),
            maker_remaining: "0.00".into(),
            taker_status: 1,
            maker_status: 1,
        };
        let a = OutcomeEvent::Fill {
            taker_order_no: "b1".into(),
            maker_order_no: "s1".into(),
            price: "100.0".into(),
            qty: "1".into(),
            taker_remaining: "0.0".into(),
            maker_remaining: "0".into(),
            taker_status: 1,
            maker_status: 1,
        };
        assert!(diff_outcomes(&[e], &[a]).is_empty());
    }
}
