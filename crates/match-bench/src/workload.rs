//! Parallel logical sequences for core (`BbOrder`) and hp (`HpCommand`).
//!
//! Core cloning of `BbOrder` is part of its measured cost (Java-shaped DTO).
//! HP commands are `Copy` — no clone noise favoring the hot path unfairly.

use bigdecimal::BigDecimal;
use match_core::{BbOrder, Side as CoreSide};
use match_core_hp::{HpCommand, Side as HpSide};
use match_protocol::ORDER_STATUS_REVOKE;
use std::str::FromStr;

fn dec(s: &str) -> BigDecimal {
    BigDecimal::from_str(s).expect("valid decimal")
}

fn core_side(buy: bool) -> CoreSide {
    if buy {
        CoreSide::Buy
    } else {
        CoreSide::Sell
    }
}

fn hp_side(buy: bool) -> HpSide {
    if buy {
        HpSide::Buy
    } else {
        HpSide::Sell
    }
}

/// Rest-only: `n` non-crossing buy limits.
pub fn rest_only(n: usize) -> (Vec<BbOrder>, Vec<HpCommand>) {
    let mut core = Vec::with_capacity(n);
    let mut hp = Vec::with_capacity(n);
    for i in 0..n {
        let price_tick = 10_000 + (i % 100) as i64;
        let price = format!("{}.{:02}", price_tick / 100, price_tick % 100);
        let no = format!("r{i}");
        core.push(BbOrder::test_limit(
            core_side(true),
            dec(&price),
            &no,
            i as i64,
            "1",
        ));
        hp.push(HpCommand::Limit {
            side: hp_side(true),
            price_tick,
            qty_lot: 1_000_000, // qty scale 6 → "1"
            ts: i as u64,
            client_id: i as u64,
        });
    }
    (core, hp)
}

/// Cross-full: rest `n/2` sells then `n/2` buys that fully fill (1:1).
pub fn cross_full(n: usize) -> (Vec<BbOrder>, Vec<HpCommand>) {
    let half = n / 2;
    let mut core = Vec::with_capacity(n);
    let mut hp = Vec::with_capacity(n);
    let price_tick = 10_000i64;
    let price = "100.00";

    for i in 0..half {
        let no = format!("s{i}");
        core.push(BbOrder::test_limit(
            core_side(false),
            dec(price),
            &no,
            i as i64,
            "1",
        ));
        hp.push(HpCommand::Limit {
            side: hp_side(false),
            price_tick,
            qty_lot: 1_000_000,
            ts: i as u64,
            client_id: i as u64,
        });
    }
    for i in 0..half {
        let t = (half + i) as i64;
        let no = format!("b{i}");
        core.push(BbOrder::test_limit(
            core_side(true),
            dec(price),
            &no,
            t,
            "1",
        ));
        hp.push(HpCommand::Limit {
            side: hp_side(true),
            price_tick,
            qty_lot: 1_000_000,
            ts: t as u64,
            client_id: (half + i) as u64,
        });
    }
    (core, hp)
}

/// Partial walk: many thin ask levels, then aggressive buys that walk several levels.
pub fn partial_walk(n: usize) -> (Vec<BbOrder>, Vec<HpCommand>) {
    // ~80% resting asks across many ticks; ~20% aggressive buys.
    let makers = (n * 4) / 5;
    let takers = n - makers;
    let mut core = Vec::with_capacity(n);
    let mut hp = Vec::with_capacity(n);

    for i in 0..makers {
        let price_tick = 10_000 + (i % 200) as i64;
        let price = format!("{}.{:02}", price_tick / 100, price_tick % 100);
        let no = format!("pw_s{i}");
        core.push(BbOrder::test_limit(
            core_side(false),
            dec(&price),
            &no,
            i as i64,
            "1",
        ));
        hp.push(HpCommand::Limit {
            side: hp_side(false),
            price_tick,
            qty_lot: 1_000_000,
            ts: i as u64,
            client_id: i as u64,
        });
    }

    for i in 0..takers {
        let t = (makers + i) as i64;
        // Aggressive buy walks from best ask upward.
        let no = format!("pw_b{i}");
        core.push(BbOrder::test_limit(
            core_side(true),
            dec("102.00"),
            &no,
            t,
            "5",
        ));
        hp.push(HpCommand::Limit {
            side: hp_side(true),
            price_tick: 10_200,
            qty_lot: 5_000_000,
            ts: t as u64,
            client_id: (makers + i) as u64,
        });
    }
    (core, hp)
}

/// Cancel-hot: rest `n/2` orders then cancel them all.
///
/// HP cancel ids assume sequential slot assignment with no prior frees (ids `1..=half`).
pub fn cancel_hot(n: usize) -> (Vec<BbOrder>, Vec<HpCommand>) {
    let half = n / 2;
    let mut core = Vec::with_capacity(n);
    let mut hp = Vec::with_capacity(n);

    for i in 0..half {
        let price_tick = 10_000 + (i % 50) as i64;
        let price = format!("{}.{:02}", price_tick / 100, price_tick % 100);
        let no = format!("{i}");
        core.push(BbOrder::test_limit(
            core_side(true),
            dec(&price),
            &no,
            i as i64,
            "1",
        ));
        hp.push(HpCommand::Limit {
            side: hp_side(true),
            price_tick,
            qty_lot: 1_000_000,
            ts: i as u64,
            client_id: i as u64,
        });
    }

    for i in 0..half {
        let t = (half + i) as i64;
        let no = format!("{i}");
        let mut rev = BbOrder::test_limit(core_side(true), dec("100.00"), &no, t, "1");
        rev.order_status = ORDER_STATUS_REVOKE;
        core.push(rev);
        // Engine ids are 1-based insertion order with no free-list reuse.
        hp.push(HpCommand::Cancel { id: (i as u64) + 1 });
    }
    (core, hp)
}

/// Mixed: rest, cross fills, cancels interleaved in blocks.
pub fn mixed(n: usize) -> (Vec<BbOrder>, Vec<HpCommand>) {
    let mut core = Vec::with_capacity(n);
    let mut hp = Vec::with_capacity(n);
    let mut i = 0usize;
    let mut next_cancel_id = 1u64;
    let mut resting_core_nos: Vec<String> = Vec::new();

    while i < n {
        let phase = (i / 64) % 4;
        match phase {
            0 => {
                // Rest a sell.
                let no = format!("m_s{i}");
                core.push(BbOrder::test_limit(
                    core_side(false),
                    dec("100.00"),
                    &no,
                    i as i64,
                    "1",
                ));
                hp.push(HpCommand::Limit {
                    side: hp_side(false),
                    price_tick: 10_000,
                    qty_lot: 1_000_000,
                    ts: i as u64,
                    client_id: i as u64,
                });
                resting_core_nos.push(no);
                next_cancel_id += 1; // hp id advanced by insert
                i += 1;
            }
            1 => {
                // Cross with a buy.
                let no = format!("m_b{i}");
                core.push(BbOrder::test_limit(
                    core_side(true),
                    dec("100.00"),
                    &no,
                    i as i64,
                    "1",
                ));
                hp.push(HpCommand::Limit {
                    side: hp_side(true),
                    price_tick: 10_000,
                    qty_lot: 1_000_000,
                    ts: i as u64,
                    client_id: i as u64,
                });
                // Taker may fully fill (no rest) or rest; maker may be gone.
                // Keep cancel bookkeeping conservative: drop one resting no if any.
                if !resting_core_nos.is_empty() {
                    resting_core_nos.pop();
                }
                i += 1;
            }
            2 => {
                // Rest a buy away from market.
                let no = format!("m_r{i}");
                core.push(BbOrder::test_limit(
                    core_side(true),
                    dec("90.00"),
                    &no,
                    i as i64,
                    "1",
                ));
                hp.push(HpCommand::Limit {
                    side: hp_side(true),
                    price_tick: 9_000,
                    qty_lot: 1_000_000,
                    ts: i as u64,
                    client_id: i as u64,
                });
                resting_core_nos.push(no);
                i += 1;
            }
            _ => {
                // Cancel last resting if possible; else rest.
                if let Some(no) = resting_core_nos.pop() {
                    let mut rev =
                        BbOrder::test_limit(core_side(true), dec("90.00"), &no, i as i64, "1");
                    rev.order_status = ORDER_STATUS_REVOKE;
                    core.push(rev);
                    // Best-effort: cancel a low id that may still exist.
                    let id = next_cancel_id.saturating_sub(1).max(1);
                    hp.push(HpCommand::Cancel { id });
                } else {
                    let no = format!("m_f{i}");
                    core.push(BbOrder::test_limit(
                        core_side(true),
                        dec("89.00"),
                        &no,
                        i as i64,
                        "1",
                    ));
                    hp.push(HpCommand::Limit {
                        side: hp_side(true),
                        price_tick: 8_900,
                        qty_lot: 1_000_000,
                        ts: i as u64,
                        client_id: i as u64,
                    });
                }
                i += 1;
            }
        }
    }
    (core, hp)
}
