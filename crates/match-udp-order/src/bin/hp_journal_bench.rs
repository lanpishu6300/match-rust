//! `hp_journal_bench` — HpEngine + mmap journal 持久化基准（参考 LMAX Disruptor 6M/s 口径）
//!
//! 三种口径（同一撮合引擎、同一订单序列）：
//!   1. no-persist    — 纯撮合（基线）
//!   2. mmap-append   — 每单 append 到 mmap（page cache 写，**非崩溃一致**）
//!   3. mmap-msync-N  — append + 每 N 单 msync(MS_SYNC) 批量落盘（真持久）
//!
//! 命令序列化：HpCommand::Limit 定长编码（tag + side + price_tick + qty_lot + ts + client_id = 35B/条）。
//! 订单序列：同 symbol 严格 B/S 交替成交（book 稳态）。
//!
//! 诚实口径：
//! - mmap-append 写 page cache 是内存速度（~ns），不等于持久——OS 崩溃丢脏页。
//! - mmap-msync-N 吞吐 ≈ N / msync 延迟（SSD 单次 msync 0.1-2ms）——6M/s 需 N 足够大。
//! - 与 LMAX 同口径：journal 批量写 + 不逐条 fsync（RAID 电池/专用盘 + 批量 flush）。

use match_core_hp::{HpCommand, HpEngine, HpEvent, MmapJournal, Side};
use std::path::PathBuf;
use std::time::Instant;

const PER_SHARD_DEFAULT: usize = 2_000_000;
const BUY_TICK: i64 = 100_000;
const SELL_TICK: i64 = 99_990;
const JOURNAL_CAP: usize = 256 * 1024 * 1024; // 256MB

/// HpCommand::Limit 定长编码（35B：tag u8 + side u8 + price i64 + qty i64 + ts u64 + client u64）
#[inline]
fn encode_limit(side: Side, price_tick: i64, qty_lot: i64, client_id: u64) -> [u8; 35] {
    let mut b = [0u8; 35];
    b[0] = 0; // tag Limit
    b[1] = if side == Side::Buy { 0 } else { 1 };
    b[2..10].copy_from_slice(&price_tick.to_le_bytes());
    b[10..18].copy_from_slice(&qty_lot.to_le_bytes());
    b[18..26].copy_from_slice(&0u64.to_le_bytes()); // ts
    b[26..34].copy_from_slice(&client_id.to_le_bytes());
    b[34] = 0; // padding（对齐 8B）
    b
}

fn run(per_shard: usize, mode: &str, msync_every: usize) -> (usize, f64) {
    let mut engine = HpEngine::new();
    let journal_path = std::env::temp_dir().join("hp_journal_bench.tmp");
    let mut journal = MmapJournal::create(&journal_path, JOURNAL_CAP).expect("journal");
    let mut t0 = Instant::now();
    let mut msync_n = 0usize;
    let mut processed = 0usize;
    for i in 0..per_shard {
        let (side, tick) = if i % 2 == 0 { (Side::Buy, BUY_TICK) } else { (Side::Sell, SELL_TICK) };
        let cmd = HpCommand::Limit {
            side,
            price_tick: tick,
            qty_lot: 1,
            ts: 0,
            client_id: i as u64,
        };
        if mode != "no-persist" {
            let enc = encode_limit(side, tick, 1, i as u64);
            journal.append(&enc).expect("journal full");
            if mode == "mmap-msync" {
                msync_n += 1;
                if msync_n >= msync_every {
                    journal.msync(true).expect("msync");
                    msync_n = 0;
                }
            }
        }
        let _evs: &[HpEvent] = engine.on_order(cmd);
        processed += 1;
    }
    if mode == "mmap-msync" && msync_n > 0 {
        journal.msync(true).expect("msync-tail");
    }
    let elapsed = t0.elapsed().as_secs_f64();
    std::fs::remove_file(&journal_path).ok();
    (processed, elapsed)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut per_shard = PER_SHARD_DEFAULT;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--per-shard" => {
                i += 1;
                per_shard = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(PER_SHARD_DEFAULT);
            }
            v => per_shard = v.parse().unwrap_or(PER_SHARD_DEFAULT),
        }
        i += 1;
    }
    println!("=== hp_journal_bench: per_shard={per_shard} journal_cap={}MB ===", JOURNAL_CAP >> 20);

    for (mode, label) in [
        ("no-persist", "no-persist (pure matching)"),
        ("mmap-append", "mmap-append (page-cache write, NOT crash-consistent)"),
        ("mmap-msync", "mmap + msync(MS_SYNC) every 4096"),
    ] {
        let (n, secs) = run(per_shard, mode, 4096);
        let rate = n as f64 / secs;
        println!("[{mode}] {label}: {n} orders in {secs:.3}s → {rate:.0}/s");
    }
    // 批量 msync 粒度扫描（真持久口径下的吞吐曲线）
    for every in [64usize, 512, 4096, 32768] {
        let (n, secs) = run(per_shard.min(1_000_000), "mmap-msync", every);
        let rate = n as f64 / secs;
        println!("[msync-every-{every}] {rate:.0}/s (per-order msync cost: {:.1}ns)", 1e9 / rate);
    }
    let _ = PathBuf::new();
}
