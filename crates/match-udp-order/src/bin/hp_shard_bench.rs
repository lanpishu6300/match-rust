//! `hp_shard_bench` — HpEngine（match-core-hp）多 shard 直分撮合基准
//!
//! 与 `rss_shard_bench`（match_core::Engine）**同形态对比**：
//! - 同结构：每 shard 线程内直接生成订单（无网络栈、无客户端线程），SPSC 直分语义
//! - 唯一变量：撮合引擎 —— match_core::Engine（String 价格、Vec<MatchEvent>） vs
//!   HpEngine（tick/lot 整数空间、&[HpEvent] 零拷贝事件）
//! - 订单序列同款：k/2 换 symbol、同 symbol 严格 B/S 交替成交（订单簿稳态）
//!
//! 对比基线（match_core::Engine, macOS 10 核）：1sh=789k, 2sh=1.59M, 4sh=2.79M, 8sh=5.13M/s

use match_core_hp::{HpCommand, HpEngine, HpEvent, Side};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

const SHARDS_DEFAULT: usize = 4;
const PER_SHARD_DEFAULT: usize = 100_000;
const SYMBOLS_PER_SHARD: usize = 32;
// tick 空间：买单 100000 ≥ 卖单 99990 → 严格交替成交（与 rss_shard_bench 的 100.00/99.99 对齐）
const BUY_TICK: i64 = 100_000;
const SELL_TICK: i64 = 99_990;

/// 每 shard：直接生成 HpCommand + HpEngine 撮合（与 rss_shard_bench 同结构）
fn shard_run(shard_id: usize, per_shard: usize) -> (usize, usize, usize) {
    let mut engine = HpEngine::new();
    let sym_base = shard_id * SYMBOLS_PER_SHARD;
    let mut processed = 0usize;
    let mut fills = 0usize;
    let mut rests = 0usize;
    for i in 0..per_shard {
        let sym_id = sym_base + ((i / 2) % SYMBOLS_PER_SHARD);
        // 同 symbol 严格 B/S 交替：Buy(100000) → Sell(99990) → 成交
        // HpEngine 语义：每个 client_id 同时只能有一个在途订单 → 每单唯一 client_id
        let (side, tick) = if i % 2 == 0 { (Side::Buy, BUY_TICK) } else { (Side::Sell, SELL_TICK) };
        let cmd = HpCommand::Limit {
            side,
            price_tick: tick,
            qty_lot: 1,
            ts: 0,
            client_id: (shard_id as u64) << 32 | i as u64, // 每单唯一
        };
        let evs = engine.on_order(cmd);
        for e in evs {
            match e {
                HpEvent::Fill { .. } => fills += 1,
                HpEvent::Rest { .. } => rests += 1,
                HpEvent::Revoke { .. } => {}
            }
        }
        processed += 1;
    }
    (processed, fills, rests)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut shards = SHARDS_DEFAULT;
    let mut per_shard = PER_SHARD_DEFAULT;
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
            v if v.starts_with("--") => {}
            v => per_shard = v.parse().unwrap_or(PER_SHARD_DEFAULT),
        }
        i += 1;
    }

    println!("=== hp_shard_bench: shards={shards} per_shard={per_shard} (total={}) ===", shards * per_shard);

    let t0 = Instant::now();
    let handles: Vec<_> = (0..shards)
        .map(|k| std::thread::spawn(move || shard_run(k, per_shard)))
        .collect();
    let mut total = 0usize;
    let mut fills = 0usize;
    let mut rests = 0usize;
    for h in handles {
        let (p, f, r) = h.join().expect("shard join");
        total += p;
        fills += f;
        rests += r;
    }
    let elapsed = t0.elapsed();
    let rate = total as f64 / elapsed.as_secs_f64();
    println!("[hp-shard] processed={total} fills={fills} rests={rests} elapsed={:.4}s rate={rate:.0}/s shards={shards}", elapsed.as_secs_f64());
}
