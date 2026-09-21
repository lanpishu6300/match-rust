//! `engine_pure_bench` — match_core::Engine vs HpEngine 纯撮合热路径公平对比
//!
//! 背景：rss_shard_bench（Engine + 字符串 format/convert 生成）789k/s @1shard，
//!       hp_shard_bench（HpEngine + 整数命令）23M/s @1shard —— 差距含订单构造成本，
//!       不能归因于引擎本身。
//!
//! 本 bench：**两种引擎都预生成订单（各自原生类型），只测 on_order 热循环**，
//! 隔离引擎差异（字符串/整数、Vec<MatchEvent> 分配 vs &[HpEvent] 零分配、symbol 查找）。
//!
//! 订单序列同款：k/2 换 symbol、同 symbol 严格 B/S 交替成交（订单簿稳态）。

use match_core::{BbOrder, Engine, MatchEvent};
use match_core_hp::{HpCommand, HpEngine, HpEvent, Side};
use match_protocol::{type_convert_spot, MqOrder, ORDER_FORM_LIMIT, ORDER_TYPE_BUY, ORDER_TYPE_SELL};
use std::time::Instant;

const SHARDS_DEFAULT: usize = 4;
const PER_SHARD_DEFAULT: usize = 100_000;
const SYMBOLS_PER_SHARD: usize = 32;
const BUY_TICK: i64 = 100_000;
const SELL_TICK: i64 = 99_990;

fn mq_limit(side: i8, symbol: &str, no: &str, price: &str) -> MqOrder {
    MqOrder {
        user_id: Some(1), uid: Some(1), c_type: 1, deal_type: None, r#type: Some(1),
        order_type: Some(side), market_id: Some(1), coin_id: Some(2),
        symbol_key: Some(symbol.into()), coin_market: Some("BTC/USDT".into()),
        trust_order_no: Some(no.into()), close_position: None, start_deposit: None,
        position_type: None, taker_rate: None, order_status: Some(0),
        order_form: Some(ORDER_FORM_LIMIT), gear: None, lever_times: None,
        trust_number: Some("1".into()), trust_price: Some(price.into()),
        create_time: Some(1_700_000_000), face_value: None, handicap_type: None,
    }
}

/// 预生成 BbOrder（format + convert 移出热循环）
fn pregen_core(shard_id: usize, per_shard: usize) -> Vec<BbOrder> {
    let sym_base = shard_id * SYMBOLS_PER_SHARD;
    (0..per_shard)
        .map(|i| {
            let sym_id = sym_base + ((i / 2) % SYMBOLS_PER_SHARD);
            let sym = format!("sym_{sym_id:03}");
            let (side, price) = if i % 2 == 0 { (ORDER_TYPE_BUY, "100.00") } else { (ORDER_TYPE_SELL, "99.99") };
            let mq = mq_limit(side, &sym, &format!("c{shard_id}o{i}"), price);
            BbOrder(type_convert_spot(&mq).expect("convert"))
        })
        .collect()
}

/// 预生成 HpCommand（整数，无构造成本）
fn pregen_hp(shard_id: usize, per_shard: usize) -> Vec<HpCommand> {
    (0..per_shard)
        .map(|i| {
            let (side, tick) = if i % 2 == 0 { (Side::Buy, BUY_TICK) } else { (Side::Sell, SELL_TICK) };
            HpCommand::Limit {
                side,
                price_tick: tick,
                qty_lot: 1,
                ts: 0,
                client_id: (shard_id as u64) << 32 | i as u64,
            }
        })
        .collect()
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
    println!("=== engine_pure_bench: shards={shards} per_shard={per_shard} (total={}) ===", shards * per_shard);

    // match_core::Engine：预生成 + 纯 on_order
    let t0 = Instant::now();
    let handles: Vec<_> = (0..shards)
        .map(|k| {
            let orders = pregen_core(k, per_shard);
            std::thread::spawn(move || {
                let mut engine = Engine::new();
                let mut n = 0usize;
                let mut fills = 0usize;
                for bb in orders {
                    // move 语义（与 rss_shard_bench 一致），不计入 clone/深拷贝
                    let evs: Vec<MatchEvent> = engine.on_order(bb);
                    n += 1;
                    for e in &evs {
                        if let MatchEvent::Fill { .. } = e { fills += 1; }
                    }
                }
                (n, fills)
            })
        })
        .collect();
    let (mut total, mut fills) = (0usize, 0usize);
    for h in handles {
        let (t, f) = h.join().expect("core join");
        total += t;
        fills += f;
    }
    let core_rate = total as f64 / t0.elapsed().as_secs_f64();

    // HpEngine：预生成 + 纯 on_order
    let t1 = Instant::now();
    let handles: Vec<_> = (0..shards)
        .map(|k| {
            let cmds = pregen_hp(k, per_shard);
            std::thread::spawn(move || {
                let mut engine = HpEngine::new();
                let mut n = 0usize;
                let mut fills = 0usize;
                for cmd in &cmds {
                    let evs: &[HpEvent] = engine.on_order(*cmd);
                    n += 1;
                    for e in evs {
                        if let HpEvent::Fill { .. } = e { fills += 1; }
                    }
                }
                (n, fills)
            })
        })
        .collect();
    let (mut total2, mut fills2) = (0usize, 0usize);
    for h in handles {
        let (t, f) = h.join().expect("hp join");
        total2 += t;
        fills2 += f;
    }
    let hp_rate = total2 as f64 / t1.elapsed().as_secs_f64();

    println!("[core] match_core::Engine   processed={total} fills={fills} rate={core_rate:.0}/s shards={shards}");
    println!("[hp]   HpEngine            processed={total2} fills={fills2} rate={hp_rate:.0}/s shards={shards}");
    println!("[ratio] hp/core = {:.2}x", hp_rate / core_rate);
}
