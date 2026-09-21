//! `tcp_rss_shard_bench` — TCP RSS 直分多 shard 撮合基准（真实内核 TCP 路径）
//!
//! 与 `rss_shard_bench`（SPSC 直分，无网络栈）的对照：
//! - rss_shard_bench：独立客户端线程 + 无锁 SPSC ring → shard 撮合（无内核网络栈）。
//! - tcp_rss_shard_bench：**每 shard = 独立 TCP 连接（内核栈 recv）+ 独立撮合线程**，
//!   模拟"网卡 RSS 按 TCP 四元组哈希把连接分流到多队列 → 每队列独立收包线程撮合"。
//!   含真实 TCP 路径开销（syscall/软中断/唤醒），与本机回环 TCP 基线可比。
//!
//! 每 shard 专属 symbol 集（sym_{k*32}..），模拟 RSS 已把该标的连接投到本队列；
//! 同 symbol 严格 B/S 交替成交（订单簿稳态，避免 O(n²) 堆积）。

use match_core::{BbOrder, Engine, MatchEvent};
use match_protocol::{type_convert_spot, MqOrder, ORDER_FORM_LIMIT, ORDER_TYPE_BUY, ORDER_TYPE_SELL};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

const SHARDS_DEFAULT: usize = 4;
const PER_SHARD_DEFAULT: usize = 50_000;
const SYMBOLS_PER_SHARD: usize = 32;
const PORT_BASE: u16 = 56100;

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

fn parse_order(payload: &[u8]) -> Option<(BbOrder, String)> {
    let s = std::str::from_utf8(payload).ok()?;
    let mut it = s.split('|');
    let side = it.next()?;
    let symbol = it.next()?;
    let price = it.next()?;
    let qty = it.next()?;
    let no = it.next()?.to_string();
    let side_i = if side == "B" { ORDER_TYPE_BUY } else if side == "S" { ORDER_TYPE_SELL } else { return None; };
    let mq = mq_limit(side_i, symbol, &no, price, qty);
    Some((BbOrder(type_convert_spot(&mq).expect("convert")), no))
}

fn report_text(ev: &MatchEvent) -> Vec<u8> {
    match ev {
        MatchEvent::Fill { taker_order_no, maker_order_no, price, qty, .. } =>
            format!("E|{taker_order_no}|{maker_order_no}|{price}|{qty}").into_bytes(),
        MatchEvent::Revoke { order_no, symbol, .. } =>
            format!("A|{order_no}|{symbol}").into_bytes(),
    }
}

/// shard 服务端：accept 单连接，读行→解析→撮合→回报
fn server_run(shard_id: usize) -> std::io::Result<usize> {
    let listener = TcpListener::bind(("127.0.0.1", PORT_BASE + shard_id as u16))?;
    let (stream, _) = listener.accept()?;
    stream.set_nodelay(true)?;
    let mut engine = Engine::new();
    let rd = BufReader::new(stream.try_clone()?);
    let mut wr = BufWriter::new(stream);
    let mut handled = 0usize;
    let mut it = rd.split(b'\n');
    while let Some(Ok(raw)) = it.next() {
        if raw.is_empty() { continue; }
        if let Some((bb, no)) = parse_order(&raw) {
            let evs = engine.on_order(bb);
            handled += 1;
            if evs.is_empty() {
                let _ = wr.write_all(format!("A|{no}|sym\n").as_bytes());
            } else {
                for e in &evs {
                    let mut t = report_text(e);
                    t.push(b'\n');
                    let _ = wr.write_all(&t);
                }
            }
            let _ = wr.flush();
        }
    }
    Ok(handled)
}

/// shard 客户端：连接本 shard，发订单（专属 symbol 集），收回报，统计 RTT
fn client_run(shard_id: usize, per_shard: usize) -> std::io::Result<(usize, Vec<Duration>)> {
    let mut s = TcpStream::connect(("127.0.0.1", PORT_BASE + shard_id as u16))?;
    s.set_nodelay(true)?;
    let mut rd = BufReader::new(s.try_clone()?);
    let sym_base = shard_id * SYMBOLS_PER_SHARD;
    let mut rtts: Vec<Duration> = Vec::with_capacity(per_shard);
    for i in 0..per_shard {
        // k/2 换 symbol：同 symbol 严格 B/S 交替成交（稳态）
        let sym_id = sym_base + ((i / 2) % SYMBOLS_PER_SHARD);
        let sym = format!("sym_{sym_id:03}");
        let side = if i % 2 == 0 { "B" } else { "S" };
        let price = if i % 2 == 0 { "100.00" } else { "99.99" };
        let payload = format!("{side}|{sym}|{price}|1|t{shard_id}o{i}\n");
        let t = Instant::now();
        s.write_all(payload.as_bytes())?;
        let mut line = String::new();
        rd.read_line(&mut line)?;
        rtts.push(t.elapsed());
    }
    Ok((rtts.len(), rtts))
}

fn main() -> std::io::Result<()> {
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
    println!("=== tcp_rss_shard_bench: shards={shards} per_shard={per_shard} (total={}) ===", shards * per_shard);

    let t0 = Instant::now();
    let mut server_handles = Vec::with_capacity(shards);
    let mut client_handles = Vec::with_capacity(shards);
    for k in 0..shards {
        server_handles.push(std::thread::spawn(move || server_run(k)));
    }
    std::thread::sleep(Duration::from_millis(100)); // 等服务端监听
    for k in 0..shards {
        client_handles.push(std::thread::spawn(move || client_run(k, per_shard)));
    }

    let mut handled_total = 0usize;
    for h in server_handles {
        handled_total += h.join().expect("server join")?;
    }
    let mut all_rtts: Vec<Duration> = Vec::with_capacity(shards * per_shard);
    let mut sent_total = 0usize;
    for h in client_handles {
        let (n, rtts) = h.join().expect("client join")?;
        sent_total += n;
        all_rtts.extend(rtts);
    }
    let elapsed = t0.elapsed();
    let rate = handled_total as f64 / elapsed.as_secs_f64();

    all_rtts.sort();
    let p50 = all_rtts[all_rtts.len() / 2].as_micros();
    let p99i = ((all_rtts.len() as f64 * 0.99) as usize).min(all_rtts.len() - 1);
    let p99 = all_rtts[p99i].as_micros();

    println!(
        "[tcp-rss] sent={sent_total} handled={handled_total} elapsed={:.3}s rate={rate:.0}/s shards={shards}",
        elapsed.as_secs_f64()
    );
    println!("[tcp-rss-latency] p50={p50}µs p99={p99}µs (client RTT, per-order, loopback)");
    Ok(())
}
