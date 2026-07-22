//! Matrix pressure test: resting depth × stream length × fill band (HP).
//!
//! ```text
//! cargo run -p match-bench --release --bin tier_sweep -- --preset quick
//! cargo run -p match-bench --release --bin tier_sweep -- --preset default --out docs/bench-results/tier-sweep.csv
//! ```

use clap::Parser;
use match_bench::tier::{self, FillBand, TierCell};
use match_core_hp::{HpEngine, HpEvent};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Parser, Debug)]
#[command(about = "Tier sweep: rest × stream × fill band for match-core-hp")]
struct Args {
    /// `quick` (4 cells) or `default` (27 cells).
    #[arg(long, default_value = "quick")]
    preset: String,

    /// Timed runs per cell; report median.
    #[arg(long, default_value_t = 5)]
    runs: usize,

    /// Warmup timed passes discarded per cell.
    #[arg(long, default_value_t = 1)]
    warmup: usize,

    /// Fail process if any cell misses its fill gate.
    #[arg(long, default_value_t = true)]
    gate: bool,

    /// Allow gate failures (still print rows).
    #[arg(long, default_value_t = false)]
    loose: bool,

    /// Optional CSV path (same columns as stdout).
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Clone, Debug)]
struct CellStats {
    rest: usize,
    stream: usize,
    tier: &'static str,
    n_fills: u64,
    fill_rate: f64,
    elapsed_ns: u128,
    ns_per_order: f64,
    orders_per_sec: f64,
    fills_per_sec: f64,
    peak_mapped: usize,
}

fn run_once(cell: TierCell) -> (u128, u64, usize) {
    let warm = tier::warm_cmds(cell);
    let stream = tier::stream_cmds(cell, tier::stream_id_base(cell));
    let cap = warm.len() + stream.len() + 8;
    // High band emits multiple fills per order.
    let mut eng = HpEngine::with_capacity(cap, 256);

    for c in &warm {
        let _ = eng.on_order(*c);
    }

    let mut fills = 0u64;
    let mut peak = eng.client_map_len();
    let t0 = Instant::now();
    for c in &stream {
        for e in eng.on_order(*c) {
            if matches!(e, HpEvent::Fill { .. }) {
                fills += 1;
            }
        }
        let n = eng.client_map_len();
        if n > peak {
            peak = n;
        }
    }
    (t0.elapsed().as_nanos(), fills, peak)
}

fn median_u128(xs: &mut [u128]) -> u128 {
    xs.sort_unstable();
    xs[xs.len() / 2]
}

fn median_usize(xs: &mut [usize]) -> usize {
    xs.sort_unstable();
    xs[xs.len() / 2]
}

fn gate_ok(tier: FillBand, fill_rate: f64) -> bool {
    match tier {
        FillBand::Low => (fill_rate - 0.10).abs() <= 0.05,
        FillBand::Mid => (fill_rate - 0.50).abs() <= 0.05,
        FillBand::High => fill_rate >= 1.5,
    }
}

fn main() {
    let args = Args::parse();
    let cells = match args.preset.as_str() {
        "default" => tier::preset_default(),
        "quick" => tier::preset_quick(),
        other => {
            eprintln!("unknown preset {other:?} (use quick|default)");
            std::process::exit(2);
        }
    };
    let runs = args.runs.max(1);
    let warmup = args.warmup;

    let header =
        "rest,stream,tier,n_fills,fill_rate,elapsed_ns,orders_per_sec,fills_per_sec,ns_per_order,peak_mapped";
    println!("{header}");

    let mut csv = args.out.as_ref().map(|path| {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let f = File::create(path).unwrap_or_else(|e| {
            eprintln!("failed to create {}: {e}", path.display());
            std::process::exit(2);
        });
        let mut w = BufWriter::new(f);
        writeln!(w, "{header}").ok();
        w
    });

    let mut failed = false;
    let mut rows = Vec::new();

    for cell in cells {
        for _ in 0..warmup {
            let _ = run_once(cell);
        }
        let mut elapsed = Vec::with_capacity(runs);
        let mut fills = Vec::with_capacity(runs);
        let mut peaks = Vec::with_capacity(runs);
        for _ in 0..runs {
            let (ns, f, peak) = run_once(cell);
            elapsed.push(ns);
            fills.push(f);
            peaks.push(peak);
        }
        let ns = median_u128(&mut elapsed);
        let n_fills = {
            fills.sort_unstable();
            fills[fills.len() / 2]
        };
        let peak = median_usize(&mut peaks);
        let stream = cell.stream as u64;
        let fill_rate = if stream == 0 {
            0.0
        } else {
            n_fills as f64 / stream as f64
        };
        let orders_per_sec = if ns == 0 {
            0.0
        } else {
            stream as f64 * 1_000_000_000.0 / ns as f64
        };
        let fills_per_sec = if ns == 0 {
            0.0
        } else {
            n_fills as f64 * 1_000_000_000.0 / ns as f64
        };
        let ns_per_order = if stream == 0 {
            0.0
        } else {
            ns as f64 / stream as f64
        };

        let row = CellStats {
            rest: cell.rest,
            stream: cell.stream,
            tier: cell.tier.as_str(),
            n_fills,
            fill_rate,
            elapsed_ns: ns,
            ns_per_order,
            orders_per_sec,
            fills_per_sec,
            peak_mapped: peak,
        };

        let line = format!(
            "{},{},{},{},{:.6},{},{:.3},{:.3},{:.3},{}",
            row.rest,
            row.stream,
            row.tier,
            row.n_fills,
            row.fill_rate,
            row.elapsed_ns,
            row.orders_per_sec,
            row.fills_per_sec,
            row.ns_per_order,
            row.peak_mapped
        );
        println!("{line}");
        if let Some(w) = csv.as_mut() {
            writeln!(w, "{line}").ok();
        }

        if !gate_ok(cell.tier, fill_rate) {
            eprintln!(
                "GATE: rest={} stream={} tier={} fill_rate={:.4}",
                cell.rest, cell.stream, row.tier, fill_rate
            );
            failed = true;
        }
        rows.push(row);
    }

    if let Some(w) = csv.as_mut() {
        w.flush().ok();
    }

    // Slowest cells by ns/order (for follow-up work).
    let mut by_ns: Vec<&CellStats> = rows.iter().collect();
    by_ns.sort_by(|a, b| {
        b.ns_per_order
            .partial_cmp(&a.ns_per_order)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    eprintln!("# slowest by ns/order (top 5)");
    for r in by_ns.iter().take(5) {
        eprintln!(
            "# rest={} stream={} tier={} ns/order={:.1} orders/s={:.0} peak_mapped={}",
            r.rest, r.stream, r.tier, r.ns_per_order, r.orders_per_sec, r.peak_mapped
        );
    }

    if failed && args.gate && !args.loose {
        std::process::exit(1);
    }
}
