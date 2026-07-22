//! Tiered workloads: resting depth × stream length × fill intensity.

use match_core_hp::{HpCommand, Side};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillBand {
    /// Mostly rest; sparse 1:1 crosses → fill_rate ≈ 0.10.
    Low,
    /// fair_cross-style pairs → fill_rate ≈ 0.50.
    Mid,
    /// Aggressive walks → fill_rate ≥ 1.5 (fills per stream order).
    High,
}

impl FillBand {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Mid => "mid",
            Self::High => "high",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "low" => Some(Self::Low),
            "mid" => Some(Self::Mid),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

/// One cell in the matrix.
#[derive(Clone, Copy, Debug)]
pub struct TierCell {
    pub rest: usize,
    pub stream: usize,
    pub tier: FillBand,
}

/// Default preset: 3×3×3 = 27 cells.
pub fn preset_default() -> Vec<TierCell> {
    let rests = [1_000, 10_000, 100_000];
    let streams = [10_000, 50_000, 200_000];
    let tiers = [FillBand::Low, FillBand::Mid, FillBand::High];
    let mut out = Vec::with_capacity(27);
    for &rest in &rests {
        for &stream in &streams {
            for &tier in &tiers {
                out.push(TierCell { rest, stream, tier });
            }
        }
    }
    out
}

/// Smaller subset for smoke / quick iteration.
pub fn preset_quick() -> Vec<TierCell> {
    vec![
        TierCell {
            rest: 1_000,
            stream: 10_000,
            tier: FillBand::Low,
        },
        TierCell {
            rest: 10_000,
            stream: 50_000,
            tier: FillBand::Mid,
        },
        TierCell {
            rest: 10_000,
            stream: 50_000,
            tier: FillBand::High,
        },
        TierCell {
            rest: 100_000,
            stream: 50_000,
            tier: FillBand::Mid,
        },
    ]
}

/// Non-crossing buys far below any mid/high ask band (ticks 1_000..).
pub fn warm_rest(rest: usize, id_base: u64) -> Vec<HpCommand> {
    let mut v = Vec::with_capacity(rest);
    for i in 0..rest {
        let price_tick = 1_000 + (i % 500) as i64;
        v.push(HpCommand::Limit {
            side: Side::Buy,
            price_tick,
            qty_lot: 1_000_000,
            ts: i as u64,
            client_id: id_base + i as u64,
        });
    }
    v
}

/// Seed thin asks for high-tier walks (ticks 20_000+).
fn seed_asks_for_walk(n: usize, id_base: u64) -> Vec<HpCommand> {
    let mut v = Vec::with_capacity(n);
    for i in 0..n {
        v.push(HpCommand::Limit {
            side: Side::Sell,
            price_tick: 20_000 + (i % 2_000) as i64,
            qty_lot: 1_000_000,
            ts: i as u64,
            client_id: id_base + i as u64,
        });
    }
    v
}

/// Timed stream for a cell. `id_base` must not collide with warm ids.
pub fn stream_cmds(cell: TierCell, id_base: u64) -> Vec<HpCommand> {
    match cell.tier {
        FillBand::Low => stream_low(cell.stream, id_base),
        FillBand::Mid => stream_mid(cell.stream, id_base),
        FillBand::High => stream_high(cell.stream, id_base),
    }
}

/// Extra warm asks needed before a high-tier stream (so walks have liquidity).
pub fn high_tier_ask_seed(stream: usize, id_base: u64) -> Vec<HpCommand> {
    // Each taker walks ~2 lots on average → need ~2× stream asks.
    seed_asks_for_walk(stream.saturating_mul(2).max(stream), id_base)
}

/// fill_rate ≈ 0.10: R = 8C, stream = 2C + R ⇒ C = stream/10.
fn stream_low(stream: usize, id_base: u64) -> Vec<HpCommand> {
    let crosses = (stream / 10).max(1);
    let rest = stream.saturating_sub(crosses * 2);
    let mut v = Vec::with_capacity(stream);
    let mut id = id_base;
    let mut ts = 0u64;

    for i in 0..rest {
        v.push(HpCommand::Limit {
            side: Side::Buy,
            price_tick: 2_000 + (i % 300) as i64,
            qty_lot: 1_000_000,
            ts,
            client_id: id,
        });
        id += 1;
        ts += 1;
    }
    for i in 0..crosses {
        v.push(HpCommand::Limit {
            side: Side::Sell,
            price_tick: 50_000,
            qty_lot: 1_000_000,
            ts,
            client_id: id,
        });
        id += 1;
        ts += 1;
        v.push(HpCommand::Limit {
            side: Side::Buy,
            price_tick: 50_000,
            qty_lot: 1_000_000,
            ts,
            client_id: id,
        });
        id += 1;
        ts += 1;
        let _ = i;
    }
    v.truncate(stream);
    v
}

fn stream_mid(stream: usize, id_base: u64) -> Vec<HpCommand> {
    let half = stream / 2;
    let mut v = Vec::with_capacity(stream);
    let price_tick = 50_000i64;
    for i in 0..half {
        v.push(HpCommand::Limit {
            side: Side::Sell,
            price_tick,
            qty_lot: 1_000_000,
            ts: i as u64,
            client_id: id_base + i as u64,
        });
    }
    for i in 0..half {
        let t = half + i;
        v.push(HpCommand::Limit {
            side: Side::Buy,
            price_tick,
            qty_lot: 1_000_000,
            ts: t as u64,
            client_id: id_base + t as u64,
        });
    }
    v
}

/// Aggressive buys that walk seeded asks (qty 2 lots each → ~2 fills/order).
fn stream_high(stream: usize, id_base: u64) -> Vec<HpCommand> {
    let mut v = Vec::with_capacity(stream);
    for i in 0..stream {
        v.push(HpCommand::Limit {
            side: Side::Buy,
            price_tick: 30_000,
            qty_lot: 2_000_000,
            ts: i as u64,
            client_id: id_base + i as u64,
        });
    }
    v
}

/// Warm commands for a cell (rest depth + optional ask seed for high).
pub fn warm_cmds(cell: TierCell) -> Vec<HpCommand> {
    let mut w = warm_rest(cell.rest, 1);
    if cell.tier == FillBand::High {
        let ask_base = 1 + cell.rest as u64 + 1_000_000;
        w.extend(high_tier_ask_seed(cell.stream, ask_base));
    }
    w
}

/// Stream id base after warm (leave gap so client ids never collide).
pub fn stream_id_base(cell: TierCell) -> u64 {
    let warm_n = cell.rest
        + if cell.tier == FillBand::High {
            cell.stream.saturating_mul(2).max(cell.stream)
        } else {
            0
        };
    warm_n as u64 + 10_000_000
}
