//! `multi_shard_bench` — 多 shard 按标的分片撮合性能基准
//!
//! 架构：
//!   订单流 ──路由(hash(symbol) % shards)──▶ shard0(Engine+线程)
//!                                      ─▶ shard1(Engine+线程)
//!                                      ─▶ ...
//!   每个 shard 独立单线程无锁撮合，保持低单路延迟；总吞吐 = shard 数 × 单核上限。
//!
//! 对比：--shards 1（单线程基准）vs --shards 2/4/8（分片扩容）。
//! 订单格式：`B|sym_XXX|price|qty|no`，symbol 轮转（sym_000 ~ sym_127）保证跨 shard 分布。

use match_core::{BbOrder, Engine, MatchEvent};
use match_protocol::{type_convert_spot, MqOrder, ORDER_FORM_LIMIT, ORDER_TYPE_BUY, ORDER_TYPE_SELL};
use std::sync::mpsc::{self, Sender};
use std::time::Instant;

const SHARD_DEFAULT: usize = 4;
const ORDERS_DEFAULT: usize = 20000;
const SYMBOL_SPACE: usize = 128;

#[derive(Debug, Clone)]
enum OrderMsg {
    Order(BbOrder),
    Done,
}

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

/// 简易 symbol 哈希（FNV-1a，稳定跨 shard 路由）
fn symbol_hash(sym: &str) -> usize {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in sym.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h as usize
}

fn shard_worker(rx: mpsc::Receiver<OrderMsg>) -> usize {
    let mut engine = Engine::new();
    let mut processed = 0usize;
    while let Ok(msg) = rx.recv() {
        match msg {
            OrderMsg::Order(bb) => {
                let _evs = engine.on_order(bb);
                processed += 1;
            }
            OrderMsg::Done => break,
        }
    }
    processed
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut shards = SHARD_DEFAULT;
    let mut orders = ORDERS_DEFAULT;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--shards" => {
                i += 1;
                shards = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(SHARD_DEFAULT);
            }
            "--orders" => {
                i += 1;
                orders = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(ORDERS_DEFAULT);
            }
            v if v.starts_with("--") => {}
            v => orders = v.parse().unwrap_or(ORDERS_DEFAULT),
        }
        i += 1;
    }

    println!("=== multi_shard_bench: shards={shards} orders={orders} sym_space={SYMBOL_SPACE} ===");

    // 启动 shard 线程池
    let mut senders: Vec<Sender<OrderMsg>> = Vec::with_capacity(shards);
    let mut handles = Vec::with_capacity(shards);
    for _ in 0..shards {
        let (tx, rx) = mpsc::channel::<OrderMsg>();
        senders.push(tx);
        handles.push(std::thread::spawn(move || shard_worker(rx)));
    }

    // 路由并发送订单
    let t0 = Instant::now();
    let mut per_shard = vec![0usize; shards];
    for k in 0..orders {
        let sym_id = k % SYMBOL_SPACE;
        let sym = format!("sym_{sym_id:03}");
        let side = if k % 2 == 0 { ORDER_TYPE_BUY } else { ORDER_TYPE_SELL };
        let price = if k % 2 == 0 { "100.00" } else { "99.99" };
        let mq = mq_limit(side, &sym, &format!("o{k}"), price, "1");
        let bb = BbOrder(type_convert_spot(&mq).expect("convert"));
        let s = symbol_hash(&sym) % shards;
        per_shard[s] += 1;
        senders[s].send(OrderMsg::Order(bb)).expect("send");
    }

    // 通知完成并等待
    for tx in &senders {
        let _ = tx.send(OrderMsg::Done);
    }
    let mut total = 0usize;
    for h in handles {
        total += h.join().expect("join");
    }
    let elapsed = t0.elapsed();
    let rate = total as f64 / elapsed.as_secs_f64();

    println!(
        "[multi-shard] total={total} elapsed={:.3}s rate={rate:.0}/s shards={shards}",
        elapsed.as_secs_f64()
    );
    println!("[dist] per-shard: {:?}", per_shard);
}
