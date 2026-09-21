//! `hp_engine_bench` — HpEngine（i64 tick/lot，Copy 命令，零分配路径）撮合基准
//!
//! 对照实验：match_core::Engine（String symbol + BigDecimal price + BTreeSet book，
//! 实测 ~800k/s/核）vs HpEngine（i64 价格/数量 + Copy HpCommand + 索引化订单簿，
//! LMAX 风格）。直接回答"Disruptor 600万/s/核，我们差在哪里"。

use match_core_hp::{HpCommand, HpEngine, Side, SpscRing};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

const SHARDS_DEFAULT: usize = 4;
const PER_SHARD_DEFAULT: usize = 100_000;
const RING_CAP: usize = 4096;

/// 客户端线程：生成 HpCommand（Copy，零堆分配）并推入 SPSC ring
fn client_run(per_shard: usize, ring: Arc<SpscRing>, multi_price: bool) -> (usize, f64) {
    let mut pushed = 0usize;
    let t0 = Instant::now();
    let mut i = 0usize;
    while i < per_shard {
        let side = if i % 2 == 0 { Side::Buy } else { Side::Sell };
        // 多价位模式：每两单换一个价位（B/S 同价成交，但订单簿在 10 万价位间跳，
        // 测索引/局部性开销）；同价模式：固定 100.00 tick。
        let price_tick = if multi_price {
            1_000_000_000i64 + ((i / 2) % 100_000) as i64
        } else {
            100_0000_0000i64
        };
        let cmd = HpCommand::Limit {
            side,
            price_tick,
            qty_lot: 1,
            ts: i as u64,
            client_id: 1,
        };
        if ring.try_push(cmd).is_ok() {
            pushed += 1;
            i += 1;
        }
        if pushed % 64 == 0 {
            std::thread::yield_now();
        }
    }
    (pushed, t0.elapsed().as_secs_f64())
}

/// 撮合线程：从 SPSC ring 批量取命令并撮合
fn shard_run(ring: Arc<SpscRing>, done: Arc<AtomicBool>) -> usize {
    let mut engine = HpEngine::new();
    let mut batch = Vec::with_capacity(64);
    let mut processed = 0usize;
    loop {
        batch.clear();
        let n = ring.pop_n(&mut batch, 64);
        if n > 0 {
            for cmd in batch.drain(..) {
                let _evs = engine.on_order(cmd);
                processed += 1;
            }
        } else if done.load(Ordering::Acquire) {
            break;
        } else {
            std::thread::yield_now();
        }
    }
    processed
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut shards = SHARDS_DEFAULT;
    let mut per_shard = PER_SHARD_DEFAULT;
    let mut multi_price = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--shards" => {
                i += 1;
                shards = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(SHARDS_DEFAULT);
            }
            "--per-shard" => {
                i += 1;
                per_shard = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(PER_SHARD_DEFAULT);
            }
            "--multi-price" => multi_price = true,
            v if v.starts_with("--") => {}
            v => per_shard = v.parse().unwrap_or(PER_SHARD_DEFAULT),
        }
        i += 1;
    }

    println!("=== hp_engine_bench: shards={shards} per_shard={per_shard} ring_cap={RING_CAP} multi_price={multi_price} ===");

    let t0 = Instant::now();
    let mut client_handles = Vec::with_capacity(shards);
    let mut shard_handles = Vec::with_capacity(shards);
    for _ in 0..shards {
        let ring = Arc::new(SpscRing::with_capacity(RING_CAP));
        let done = Arc::new(AtomicBool::new(false));
        let rc = Arc::clone(&ring);
        let dc = Arc::clone(&done);
        let rs = Arc::clone(&ring);
        let ds = Arc::clone(&done);
        client_handles.push(std::thread::spawn(move || {
            let (p, g) = client_run(per_shard, rc, multi_price);
            dc.store(true, Ordering::Release);
            (p, g)
        }));
        shard_handles.push(std::thread::spawn(move || shard_run(rs, ds)));
    }

    let mut pushed_total = 0usize;
    let mut gen_secs = 0f64;
    for h in client_handles {
        let (p, g) = h.join().expect("client");
        pushed_total += p;
        gen_secs += g;
    }
    let mut processed_total = 0usize;
    for h in shard_handles {
        processed_total += h.join().expect("shard");
    }
    let elapsed = t0.elapsed();
    let rate = processed_total as f64 / elapsed.as_secs_f64();
    let gen_rate = pushed_total as f64 / gen_secs;
    println!(
        "[hp-shard] pushed={pushed_total} processed={processed_total} elapsed={:.4}s rate={rate:.0}/s shards={shards}",
        elapsed.as_secs_f64()
    );
    println!("[client] gen_rate={gen_rate:.0}/s | hp_rate={rate:.0}/s (end-to-end)");
}
