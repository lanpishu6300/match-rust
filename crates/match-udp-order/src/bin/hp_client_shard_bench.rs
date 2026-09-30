//! `hp_client_shard_bench` — RSS 直分形态 + HpEngine（独立客户端 → SPSC 队列 → 撮合线程）
//!
//! 与 `client_shard_bench`（BbOrder + 自写 ring + match_core::Engine）**同形态对比**：
//! - 每 shard = 独立客户端线程（生成 HpCommand，整数零开销）+ match-core-hp 自带 SpscRing
//!   （模拟 RSS 队列投递）+ 撮合线程（HpEngine::on_order）
//! - 唯一变量：命令表示 + 引擎 —— HpCommand(整数) + HpEngine vs BbOrder(String) + Engine
//!
//! 对比基线（client_shard_bench, macOS 10 核）：1sh=1.05M, 2sh=1.95M, 4sh=2.98M, 8sh=3.20M/s
//! （8 shards 时客户端 String 生成为瓶颈 gen_rate 430k/s）

use match_core_hp::{Busy, HpCommand, HpEngine, HpEvent, Side, SpscRing};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

const SHARDS_DEFAULT: usize = 4;
const PER_SHARD_DEFAULT: usize = 100_000;
const SYMBOLS_PER_SHARD: usize = 32;
const RING_CAP: usize = 4096;
const BUY_TICK: i64 = 100_000;
const SELL_TICK: i64 = 99_990;

/// 客户端线程：生成 HpCommand 并推入 SPSC ring（模拟外部下单端 → RSS 队列投递）
fn client_run(shard_id: usize, per_shard: usize, ring: Arc<SpscRing>) -> (usize, f64) {
    let mut pushed = 0usize;
    let t0 = Instant::now();
    let mut i = 0usize;
    while i < per_shard {
        let (side, tick) = if i % 2 == 0 { (Side::Buy, BUY_TICK) } else { (Side::Sell, SELL_TICK) };
        let cmd = HpCommand::Limit {
            side,
            price_tick: tick,
            qty_lot: 1,
            ts: 0,
            client_id: (shard_id as u64) << 32 | i as u64, // 每单唯一（避开单在途语义）
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

/// 撮合线程：从 SPSC ring 批量取单并撮合（done=true 且 ring 空时退出）
fn shard_run(ring: Arc<SpscRing>, done: Arc<AtomicBool>) -> (usize, usize) {
    let mut engine = HpEngine::new();
    let mut batch = Vec::with_capacity(64);
    let mut processed = 0usize;
    let mut fills = 0usize;
    loop {
        batch.clear();
        let n = ring.pop_n(&mut batch, 64);
        if n > 0 {
            for cmd in batch.drain(..) {
                let evs: &[HpEvent] = engine.on_order(cmd);
                processed += 1;
                for e in evs {
                    if let HpEvent::Fill { .. } = e { fills += 1; }
                }
            }
        } else if done.load(Ordering::Acquire) {
            break;
        } else {
            std::thread::yield_now();
        }
    }
    (processed, fills)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut shards = SHARDS_DEFAULT;
    let mut per_shard = PER_SHARD_DEFAULT;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--shards" => { i += 1; shards = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(SHARDS_DEFAULT); }
            "--per-shard" => { i += 1; per_shard = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(PER_SHARD_DEFAULT); }
            v if v.starts_with("--") => {}
            v => per_shard = v.parse().unwrap_or(PER_SHARD_DEFAULT),
        }
        i += 1;
    }
    println!("=== hp_client_shard_bench: shards={shards} per_shard={per_shard} ring_cap={RING_CAP} ===");

    let t0 = Instant::now();
    let mut client_handles = Vec::with_capacity(shards);
    let mut shard_handles = Vec::with_capacity(shards);
    for k in 0..shards {
        let ring = Arc::new(SpscRing::with_capacity(RING_CAP));
        let done = Arc::new(AtomicBool::new(false));
        let rc = Arc::clone(&ring);
        let dc = Arc::clone(&done);
        let rs = Arc::clone(&ring);
        let ds = Arc::clone(&done);
        client_handles.push(std::thread::spawn(move || {
            let (p, g) = client_run(k, per_shard, rc);
            dc.store(true, Ordering::Release);
            (p, g)
        }));
        shard_handles.push(std::thread::spawn(move || shard_run(rs, ds)));
    }

    let mut pushed_total = 0usize;
    let mut gen_rate_total = 0f64;
    for h in client_handles {
        let (p, g) = h.join().expect("client join");
        pushed_total += p;
        gen_rate_total += g;
    }
    let mut processed_total = 0usize;
    let mut fills_total = 0usize;
    for h in shard_handles {
        let (p, f) = h.join().expect("shard join");
        processed_total += p;
        fills_total += f;
    }
    let elapsed = t0.elapsed();
    let rate = processed_total as f64 / elapsed.as_secs_f64();
    let gen_rate = pushed_total as f64 / gen_rate_total;

    println!(
        "[hp-client-shard] pushed={pushed_total} processed={processed_total} fills={fills_total} elapsed={:.4}s rate={rate:.0}/s shards={shards}",
        elapsed.as_secs_f64()
    );
    println!("[client] gen_rate={gen_rate:.0}/s (client-side only) | shard_rate={rate:.0}/s (end-to-end)");
}
