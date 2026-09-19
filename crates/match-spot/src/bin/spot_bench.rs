//! Spot shell throughput microbench (inbound → worker → outbound).
//!
//! ```text
//! # single run (default 50k, 1 symbol)
//! cargo run -p match-spot --release --bin spot_bench -- 50000
//!
//! # full matrix: order counts × scenarios
//! cargo run -p match-spot --release --bin spot_bench -- sweep
//! cargo run -p match-spot --release --bin spot_bench -- sweep 1000,10000,50000
//! ```

use std::sync::Arc;
use std::time::Instant;

use match_core::{BbOrder, Engine, MatchEvent};
use match_protocol::{
    type_convert_spot, ORDER_FORM_LIMIT, ORDER_STATUS_REVOKE, ORDER_TYPE_BUY, ORDER_TYPE_SELL,
    MqOrder,
};
use match_spot::config::{TopicSplitConfig, DEFAULT_DEPTH_PUSH_INTERVAL_MS};
use match_spot::inbound::InboundRouter;
use match_spot::mq::memory::MemoryOrderSink;
use match_spot::mq::producer::Producer;
use match_spot::mq::OrderSink;
use match_spot::outbound::Outbound;
use match_spot::symbol_worker::spawn_symbol_worker;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scenario {
    Rest,
    Cross,
    Partial,
    Cancel,
    Multi10,
}

impl Scenario {
    fn name(self) -> &'static str {
        match self {
            Scenario::Rest => "rest",
            Scenario::Cross => "cross",
            Scenario::Partial => "partial",
            Scenario::Cancel => "cancel",
            Scenario::Multi10 => "multi10",
        }
    }

    fn all() -> &'static [Scenario] {
        &[
            Scenario::Rest,
            Scenario::Cross,
            Scenario::Partial,
            Scenario::Cancel,
            Scenario::Multi10,
        ]
    }
}

struct Stats {
    layer: &'static str,
    scenario: &'static str,
    n_orders: u64,
    match_fills: u64,
    mq_out: u64,
    elapsed_ns: u128,
}

impl Stats {
    fn orders_per_sec(&self) -> f64 {
        self.n_orders as f64 * 1e9 / self.elapsed_ns.max(1) as f64
    }

    fn ns_per_order(&self) -> f64 {
        self.elapsed_ns as f64 / self.n_orders.max(1) as f64
    }

    fn csv_header() -> &'static str {
        "layer,scenario,n_orders,match_fills,mq_out,ord_per_sec,ns_per_ord,elapsed_ms"
    }

    fn to_csv(&self) -> String {
        format!(
            "{},{},{},{},{},{:.0},{:.1},{:.2}",
            self.layer,
            self.scenario,
            self.n_orders,
            self.match_fills,
            self.mq_out,
            self.orders_per_sec(),
            self.ns_per_order(),
            self.elapsed_ns as f64 / 1e6,
        )
    }
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

fn to_bb(mq: &MqOrder) -> BbOrder {
    BbOrder(type_convert_spot(mq).expect("convert"))
}

fn rest_orders(n: usize, symbol: &str) -> Vec<MqOrder> {
    (0..n)
        .map(|i| {
            let tick = 10_000 + (i % 100) as i64;
            let price = format!("{}.{:02}", tick / 100, tick % 100);
            mq_limit(
                ORDER_TYPE_BUY,
                symbol,
                &format!("r{i}"),
                &price,
                "1",
            )
        })
        .collect()
}

fn cross_orders(n: usize, symbol: &str) -> Vec<MqOrder> {
    let half = n / 2;
    let mut out = Vec::with_capacity(n);
    for i in 0..half {
        out.push(mq_limit(
            ORDER_TYPE_SELL,
            symbol,
            &format!("s{i}"),
            "100.00",
            "1",
        ));
    }
    for i in 0..(n - half) {
        out.push(mq_limit(
            ORDER_TYPE_BUY,
            symbol,
            &format!("b{i}"),
            "100.00",
            "1",
        ));
    }
    out
}

/// Many thin ask levels, then aggressive buys (partial book walk).
fn partial_orders(n: usize, symbol: &str) -> Vec<MqOrder> {
    let makers = (n * 4) / 5;
    let takers = n.saturating_sub(makers);
    let mut out = Vec::with_capacity(n);
    for i in 0..makers {
        let tick = 10_000 + (i % 200) as i64;
        let price = format!("{}.{:02}", tick / 100, tick % 100);
        out.push(mq_limit(
            ORDER_TYPE_SELL,
            symbol,
            &format!("pw_s{i}"),
            &price,
            "1",
        ));
    }
    for i in 0..takers {
        out.push(mq_limit(
            ORDER_TYPE_BUY,
            symbol,
            &format!("pw_b{i}"),
            "102.00",
            "5",
        ));
    }
    out
}

/// Rest `n/2` buys then revoke them (cancel-hot).
fn cancel_orders(n: usize, symbol: &str) -> Vec<MqOrder> {
    let half = n / 2;
    let mut out = Vec::with_capacity(n);
    for i in 0..half {
        let tick = 10_000 + (i % 50) as i64;
        let price = format!("{}.{:02}", tick / 100, tick % 100);
        out.push(mq_limit(
            ORDER_TYPE_BUY,
            symbol,
            &format!("c{i}"),
            &price,
            "1",
        ));
    }
    for i in 0..(n - half) {
        let tick = 10_000 + (i % 50) as i64;
        let price = format!("{}.{:02}", tick / 100, tick % 100);
        let mut mq = mq_limit(
            ORDER_TYPE_BUY,
            symbol,
            &format!("c{i}"),
            &price,
            "1",
        );
        mq.order_status = Some(ORDER_STATUS_REVOKE);
        out.push(mq);
    }
    out
}

fn scenario_orders(scenario: Scenario, n: usize) -> Vec<MqOrder> {
    match scenario {
        Scenario::Rest => rest_orders(n, "btcusdt"),
        Scenario::Cross => cross_orders(n, "btcusdt"),
        Scenario::Partial => partial_orders(n, "btcusdt"),
        Scenario::Cancel => cancel_orders(n, "btcusdt"),
        Scenario::Multi10 => {
            let symbols = 10usize;
            let per = n / symbols;
            let mut all = Vec::with_capacity(n);
            for s in 0..symbols {
                all.extend(rest_orders(per, &format!("sym{s:02}")));
            }
            all
        }
    }
}

fn encode_batches(orders: &[MqOrder]) -> Vec<Vec<u8>> {
    orders
        .iter()
        .map(|mq| serde_json::to_vec(&[mq]).expect("json"))
        .collect()
}

fn encode_multi(orders: &[MqOrder]) -> Vec<(String, Vec<u8>)> {
    orders
        .iter()
        .map(|mq| {
            let sym = mq.symbol_key.clone().unwrap_or_else(|| "btcusdt".into());
            (sym, serde_json::to_vec(&[mq]).expect("json"))
        })
        .collect()
}

fn bench_engine(scenario: Scenario, orders: &[MqOrder]) -> Stats {
    let bb: Vec<BbOrder> = orders.iter().map(to_bb).collect();
    let t0 = Instant::now();
    let mut eng = Engine::new();
    let mut fills = 0u64;
    for o in &bb {
        for e in eng.on_order(o.clone()) {
            if matches!(e, MatchEvent::Fill { .. }) {
                fills += 1;
            }
        }
    }
    Stats {
        layer: "engine",
        scenario: scenario.name(),
        n_orders: orders.len() as u64,
        match_fills: fills,
        mq_out: 0,
        elapsed_ns: t0.elapsed().as_nanos(),
    }
}

struct ShellRun {
    router: Arc<InboundRouter>,
    worker: JoinHandle<()>,
    sink: Arc<MemoryOrderSink>,
}

fn start_shell(symbol: &str, capacity: usize) -> ShellRun {
    let sink = Arc::new(MemoryOrderSink::new());
    let outbound = Arc::new(Outbound::new(
        Producer::new(Arc::clone(&sink) as Arc<dyn OrderSink>),
        None,
        DEFAULT_DEPTH_PUSH_INTERVAL_MS,
        TopicSplitConfig::default(),
    ));
    let router = Arc::new(InboundRouter::new());
    let (tx, rx) = mpsc::channel(capacity);
    router.register_queue(symbol, tx);
    let worker = spawn_symbol_worker(symbol.to_string(), rx, outbound);
    ShellRun {
        router,
        worker,
        sink,
    }
}

async fn drain_shell(run: ShellRun) -> u64 {
    run.router.shutdown_queues();
    let _ = run.worker.await;
    run.sink.sent().len() as u64
}

async fn bench_shell_single(scenario: Scenario, bodies: &[Vec<u8>]) -> Stats {
    let cap = bodies.len().max(1024);
    let t0 = Instant::now();
    let run = start_shell("btcusdt", cap);
    for body in bodies {
        let _ = run.router.handle_body(body);
    }
    let mq_out = drain_shell(run).await;
    Stats {
        layer: "shell",
        scenario: scenario.name(),
        n_orders: bodies.len() as u64,
        match_fills: mq_out,
        mq_out,
        elapsed_ns: t0.elapsed().as_nanos(),
    }
}

async fn bench_shell_multi(scenario: Scenario, bodies: &[(String, Vec<u8>)]) -> Stats {
    let cap = bodies.len().max(1024);
    let sink = Arc::new(MemoryOrderSink::new());
    let outbound = Arc::new(Outbound::new(
        Producer::new(Arc::clone(&sink) as Arc<dyn OrderSink>),
        None,
        DEFAULT_DEPTH_PUSH_INTERVAL_MS,
        TopicSplitConfig::default(),
    ));
    let router = Arc::new(InboundRouter::new());
    let mut workers = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (sym, _) in bodies {
        if seen.insert(sym.clone()) {
            let (tx, rx) = mpsc::channel(cap);
            router.register_queue(sym, tx);
            workers.push(spawn_symbol_worker(
                sym.clone(),
                rx,
                Arc::clone(&outbound),
            ));
        }
    }

    let t0 = Instant::now();
    for (_, body) in bodies {
        let _ = router.handle_body(body);
    }
    router.shutdown_queues();
    for w in workers {
        let _ = w.await;
    }
    Stats {
        layer: "shell",
        scenario: scenario.name(),
        n_orders: bodies.len() as u64,
        match_fills: sink.sent().len() as u64,
        mq_out: sink.sent().len() as u64,
        elapsed_ns: t0.elapsed().as_nanos(),
    }
}

async fn bench_scenario(scenario: Scenario, n: usize) -> Vec<Stats> {
    let orders = scenario_orders(scenario, n);
    let engine = bench_engine(scenario, &orders);
    let shell = match scenario {
        Scenario::Multi10 => {
            bench_shell_multi(scenario, &encode_multi(&orders)).await
        }
        _ => bench_shell_single(scenario, &encode_batches(&orders)).await,
    };
    vec![engine, shell]
}

fn default_sweep_sizes() -> Vec<usize> {
    vec![1_000, 10_000, 50_000, 100_000]
}

fn parse_sweep_sizes(arg: Option<&str>) -> Vec<usize> {
    arg.map(|s| {
        s.split(',')
            .filter_map(|x| x.trim().parse::<usize>().ok())
            .filter(|&n| n >= 2)
            .collect()
    })
    .filter(|v: &Vec<usize>| !v.is_empty())
    .unwrap_or_else(default_sweep_sizes)
}

async fn run_sweep(sizes: &[usize]) {
    println!(
        "match-spot sweep  depth_push_interval_ms={}  sizes={:?}",
        DEFAULT_DEPTH_PUSH_INTERVAL_MS, sizes
    );
    println!("{}", Stats::csv_header());
    for &n in sizes {
        for &scenario in Scenario::all() {
            for stat in bench_scenario(scenario, n).await {
                println!("{}", stat.to_csv());
            }
        }
    }
}

async fn run_single(n: usize, symbols: usize) {
    println!(
        "match-spot bench  n={n}  symbols={symbols}  depth_push_interval_ms={}",
        DEFAULT_DEPTH_PUSH_INTERVAL_MS
    );
    println!("{}", Stats::csv_header());
    for &scenario in Scenario::all() {
        if scenario == Scenario::Multi10 && symbols <= 1 {
            continue;
        }
        if scenario != Scenario::Multi10 && symbols > 1 {
            continue;
        }
        for stat in bench_scenario(scenario, n).await {
            println!("{}", stat.to_csv());
        }
    }
}

#[tokio::main]
async fn main() {
    let arg1 = std::env::args().nth(1);
    if arg1.as_deref() == Some("sweep") {
        let sizes = parse_sweep_sizes(std::env::args().nth(2).as_deref());
        run_sweep(&sizes).await;
        return;
    }

    let n: usize = arg1
        .and_then(|s| s.parse().ok())
        .unwrap_or(50_000)
        .max(2);
    let symbols: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .max(1);
    run_single(n, symbols).await;
}
