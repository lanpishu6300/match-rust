//! `client_shard_bench` — 独立客户端生成订单 + 无锁 SPSC 投递的多 shard 撮合基准
//!
//! 对照 `rss_shard_bench`（shard 线程内直接生成订单）的改进：
//! - rss_shard_bench：订单生成（format + type_convert）与撮合在同一线程串行，
//!   生成开销混入撮合线程，未模拟"客户端投递"路径。
//! - client_shard_bench：**每个 shard = 独立客户端线程（生成订单）+ 无锁 SPSC ring
//!   （模拟 RSS 队列投递）+ 撮合线程**。客户端生成与撮合完全解耦，可分别观察瓶颈。
//!
//! SPSC ring：单生产者单消费者无锁环形队列（缓存行对齐，参考 match-core-hp 设计）。

use match_core::{BbOrder, Engine, MatchEvent};
use match_protocol::{type_convert_spot, MqOrder, ORDER_FORM_LIMIT, ORDER_TYPE_BUY, ORDER_TYPE_SELL};
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

const SHARDS_DEFAULT: usize = 4;
const PER_SHARD_DEFAULT: usize = 100_000;
const SYMBOLS_PER_SHARD: usize = 32;
const RING_CAP: usize = 4096;

// ---- 无锁 SPSC ring（BbOrder 专用）----
#[repr(align(64))]
struct Pad<T>(T);

struct SpscRing {
    buf: Box<[UnsafeCell<Option<BbOrder>>]>,
    mask: usize,
    tail: Pad<AtomicUsize>,
    head: Pad<AtomicUsize>,
}

unsafe impl Send for SpscRing {}
unsafe impl Sync for SpscRing {}

impl SpscRing {
    fn with_capacity(cap: usize) -> Self {
        let cap = cap.max(2).next_power_of_two();
        let mut buf = Vec::with_capacity(cap);
        for _ in 0..cap {
            buf.push(UnsafeCell::new(None));
        }
        Self { buf: buf.into_boxed_slice(), mask: cap - 1, tail: Pad(AtomicUsize::new(0)), head: Pad(AtomicUsize::new(0)) }
    }

    fn try_push(&self, cmd: BbOrder) -> bool {
        let tail = self.tail.0.load(Ordering::Relaxed);
        let head = self.head.0.load(Ordering::Acquire);
        if tail.wrapping_sub(head) > self.mask { return false; }
        unsafe { *self.buf[tail & self.mask].get() = Some(cmd); }
        self.tail.0.store(tail.wrapping_add(1), Ordering::Release);
        true
    }

    fn pop_n(&self, out: &mut Vec<BbOrder>, max: usize) -> usize {
        if max == 0 { return 0; }
        let head = self.head.0.load(Ordering::Relaxed);
        let tail = self.tail.0.load(Ordering::Acquire);
        let n = tail.wrapping_sub(head).min(max);
        if n == 0 { return 0; }
        for i in 0..n {
            let idx = (head.wrapping_add(i)) & self.mask;
            let cmd = unsafe { (&mut *self.buf[idx].get()).take().expect("ring slot") };
            out.push(cmd);
        }
        self.head.0.store(head.wrapping_add(n), Ordering::Release);
        n
    }
}

// ---- 订单构造 ----
fn mq_limit(side: i8, symbol: &str, no: &str, price: &str, qty: &str) -> MqOrder {
    MqOrder {
        user_id: Some(1), uid: Some(1), c_type: 1, deal_type: None, r#type: Some(1),
        order_type: Some(side), market_id: Some(1), coin_id: Some(2),
        symbol_key: Some(symbol.into()), coin_market: Some("BTC/USDT".into()),
        trust_order_no: Some(no.into()), close_position: None, start_deposit: None,
        position_type: None, taker_rate: None, order_status: Some(0),
        order_form: Some(ORDER_FORM_LIMIT), gear: None, lever_times: None,
        trust_number: Some(qty.into()), trust_price: Some(price.into()),
        create_time: Some(1_700_000_000), face_value: None, handicap_type: None,
    }
}

/// 客户端线程：生成订单并推入 SPSC ring（模拟外部下单端 → RSS 队列投递）
fn client_run(shard_id: usize, per_shard: usize, ring: Arc<SpscRing>) -> (usize, f64) {
    let sym_base = shard_id * SYMBOLS_PER_SHARD;
    let mut pushed = 0usize;
    let t0 = Instant::now();
    let mut i = 0usize;
    while i < per_shard {
        // 订单序列：k/2 换 symbol，同 symbol 严格 B/S 交替成交（订单簿稳态）
        let sym_id = sym_base + ((i / 2) % SYMBOLS_PER_SHARD);
        let sym = format!("sym_{sym_id:03}");
        let side = if i % 2 == 0 { ORDER_TYPE_BUY } else { ORDER_TYPE_SELL };
        let price = if i % 2 == 0 { "100.00" } else { "99.99" };
        let mq = mq_limit(side, &sym, &format!("c{shard_id}o{i}"), price, "1");
        let bb = BbOrder(type_convert_spot(&mq).expect("convert"));
        if ring.try_push(bb) {
            pushed += 1;
            i += 1;
        }
        // ring 满则让出（模拟背压）
        if pushed % 64 == 0 {
            std::thread::yield_now();
        }
    }
    (pushed, t0.elapsed().as_secs_f64())
}

/// 撮合线程：从 SPSC ring 批量取单并撮合（done=true 且 ring 空时退出）
fn shard_run(ring: Arc<SpscRing>, done: Arc<AtomicBool>) -> usize {
    let mut engine = Engine::new();
    let mut batch = Vec::with_capacity(64);
    let mut processed = 0usize;
    loop {
        batch.clear();
        let n = ring.pop_n(&mut batch, 64);
        if n > 0 {
            for bb in batch.drain(..) {
                let _evs: Vec<MatchEvent> = engine.on_order(bb);
                processed += 1;
            }
        } else if done.load(Ordering::Acquire) {
            break; // 客户端已完成且 ring 空
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

    println!("=== client_shard_bench: shards={shards} per_shard={per_shard} ring_cap={RING_CAP} ===");

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
    for h in shard_handles {
        processed_total += h.join().expect("shard join");
    }
    let elapsed = t0.elapsed();
    let rate = processed_total as f64 / elapsed.as_secs_f64();
    let gen_rate = pushed_total as f64 / gen_rate_total;

    println!(
        "[client-shard] pushed={pushed_total} processed={processed_total} elapsed={:.4}s rate={rate:.0}/s shards={shards}",
        elapsed.as_secs_f64()
    );
    println!("[client] gen_rate={gen_rate:.0}/s (client-side only) | shard_rate={rate:.0}/s (end-to-end)");
}
