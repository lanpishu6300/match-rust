//! `rss_shard_bench` — RSS 直分多 shard 撮合基准（无中央路由单点）
//!
//! 与 `multi_shard_bench`（软件中央路由）的对照实验：
//! - multi_shard_bench：主线程接收全部订单 → hash(symbol) 路由 → mpsc → N 个 worker
//!   —— 主线程路由是全局吞吐单点，实测 shards>2 后不再扩展。
//! - rss_shard_bench：**每个 shard 独立收包线程 + 独立 Engine + 专属 symbol 集**，
//!   模拟 DPDK 多队列 RSS 直分（网卡按 symbol 哈希把报文直接投递到各 RX 队列，
//!   无软件路由单点）。吞吐上限 = min(网卡线速, N × 单核 engine 上限)。
//!
//! 每个 shard 用专属 symbol 子集（sym_{k*32:03}..sym_{k*32+31:03}），模拟"网卡已按
//! 哈希把该标的投到本队列"——同一标的固定同一 shard，无跨 shard 匹配。

use match_core::{BbOrder, Engine, MatchEvent};
use match_protocol::{type_convert_spot, MqOrder, ORDER_FORM_LIMIT, ORDER_TYPE_BUY, ORDER_TYPE_SELL};
use std::time::Instant;

const SHARDS_DEFAULT: usize = 8;
const PER_SHARD_DEFAULT: usize = 100_000;
const SYMBOLS_PER_SHARD: usize = 32;

fn mq_limit(side: i8, symbol: &str, no: &str, price: &str, qty: &str) -> MqOrder {
    MqOrder {
        user_id: Some(1),
        uid: Some(1),
        c_type: 1,
        deal_type: None,
        r#type: Some(1),
        order_type: Some(side),
        market_id: Some(1),
        coin_id: Some(2),
        symbol_key: Some(symbol.into()),
        coin_market: Some("BTC/USDT".into()),
        trust_order_no: Some(no.into()),
        close_position: None,
        start_deposit: None,
        position_type: None,
        taker_rate: None,
        order_status: Some(0),
        order_form: Some(ORDER_FORM_LIMIT),
        gear: None,
        lever_times: None,
        trust_number: Some(qty.into()),
        trust_price: Some(price.into()),
        create_time: Some(1_700_000_000),
        face_value: None,
        handicap_type: None,
    }
}

/// 单个 shard 线程：独立 Engine + 专属 symbol 集，直接撮合（模拟 RSS 直分投递）
fn shard_run(shard_id: usize, per_shard: usize) -> usize {
    let mut engine = Engine::new();
    let sym_base = shard_id * SYMBOLS_PER_SHARD;
    let mut processed = 0usize;
    for i in 0..per_shard {
        // symbol 轮转用 (i/2) 而非 (i)：否则 i%2(side) 与 32(symbol 步长, 偶数) 同奇偶，
        // 每个 symbol 只有单边订单、永不成交、订单簿 O(n²) 堆积。每两单换 symbol，
        // 同 symbol 严格 B/S 交替成交，订单簿稳态。
        let sym_id = sym_base + ((i / 2) % SYMBOLS_PER_SHARD);
        let sym = format!("sym_{sym_id:03}");
        let side = if i % 2 == 0 { ORDER_TYPE_BUY } else { ORDER_TYPE_SELL };
        let price = if i % 2 == 0 { "100.00" } else { "99.99" };
        let mq = mq_limit(side, &sym, &format!("s{shard_id}o{i}"), price, "1");
        let bb = BbOrder(type_convert_spot(&mq).expect("convert"));
        let _evs: Vec<MatchEvent> = engine.on_order(bb);
        processed += 1;
    }
    processed
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

    println!("=== rss_shard_bench: shards={shards} per_shard={per_shard} (total={}) ===", shards * per_shard);

    let t0 = Instant::now();
    let mut handles = Vec::with_capacity(shards);
    for k in 0..shards {
        handles.push(std::thread::spawn(move || shard_run(k, per_shard)));
    }
    let mut total = 0usize;
    for h in handles {
        total += h.join().expect("join");
    }
    let elapsed = t0.elapsed();
    let rate = total as f64 / elapsed.as_secs_f64();
    println!(
        "[rss-shard] total={total} elapsed={:.4}s rate={rate:.0}/s shards={shards}",
        elapsed.as_secs_f64()
    );
}
