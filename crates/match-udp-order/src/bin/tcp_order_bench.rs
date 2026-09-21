//! `tcp_order_bench` — TCP 下单性能基准（与 udp_order_gw 同一撮合链路，同口径对比）
//!
//! 链路：TCP 行协议订单 → match-core::Engine → REPORT（A|/E|）逐行回报。
//! 与 `udp_order_gw` 完全等价：订单 payload "B|btcusdt|100.00|1|oN" / "S|btcusdt|99.99|1|oN"，
//! 交替 B/S 保证成交；TCP_NODELAY 关闭（生产下单网关均关闭 Nagle）。
//!
//! Usage:
//!   tcp_order_bench --ping N       单笔往返 RTT（N 轮）
//!   tcp_order_bench --batch N      批量吞吐（连发 N 笔，收 N 条回报）
//!   tcp_order_bench --port N       端口（默认 55180）
//!
//! 服务端与客户端同进程（服务端在子线程 accept），与 udp_order_gw 结构一致。

use match_core::{Engine, MatchEvent};
use match_protocol::{
    type_convert_spot, MqOrder, ORDER_FORM_LIMIT, ORDER_TYPE_BUY, ORDER_TYPE_SELL,
};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

const PORT_DEFAULT: u16 = 55180;

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

/// 订单 payload 解析（与 udp_order_gw 同一实现，保证对比口径一致）
fn parse_order(payload: &[u8]) -> Option<(MqOrder, String)> {
    let s = std::str::from_utf8(payload).ok()?;
    let mut it = s.split('|');
    let side = it.next()?;
    let symbol = it.next()?;
    let price = it.next()?;
    let qty = it.next()?;
    let no = it.next()?.to_string();
    let side_i = if side == "B" {
        ORDER_TYPE_BUY
    } else if side == "S" {
        ORDER_TYPE_SELL
    } else {
        return None;
    };
    Some((mq_limit(side_i, symbol, &no, price, qty), no))
}

fn report_text(ev: &MatchEvent) -> Vec<u8> {
    match ev {
        MatchEvent::Fill {
            taker_order_no,
            maker_order_no,
            price,
            qty,
            ..
        } => format!("E|{taker_order_no}|{maker_order_no}|{price}|{qty}").into_bytes(),
        MatchEvent::Revoke {
            order_no, symbol, ..
        } => format!("A|{order_no}|{symbol}").into_bytes(),
    }
}

fn server(port: u16) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let (stream, _) = listener.accept()?;
    stream.set_nodelay(true)?;
    let mut engine = Engine::new();
    let rd = BufReader::new(stream.try_clone()?);
    let mut wr = BufWriter::new(stream);
    let mut line: Vec<u8> = Vec::new();
    let mut handled = 0u64;
    let t0 = Instant::now();
    let mut it = rd.split(b'\n');
    while let Some(Ok(raw)) = it.next() {
        if raw.is_empty() {
            continue;
        }
        if let Some((mq, no)) = parse_order(&raw) {
            let bb = match_core::BbOrder(type_convert_spot(&mq).expect("convert"));
            let evs = engine.on_order(bb);
            handled += 1;
            if evs.is_empty() {
                let _ = wr.write_all(format!("A|{no}|btcusdt\n").as_bytes());
            } else {
                for e in &evs {
                    let mut t = report_text(e);
                    t.push(b'\n');
                    let _ = wr.write_all(&t);
                }
            }
            // 每条回报立即发出（真实下单网关行为；不 flush 会因 BufWriter 缓冲与
            // client 读回报互等而死锁）
            let _ = wr.flush();
        }
        if handled % 5000 == 0 {
            eprintln!(
                "[s] handled={handled} t={:.3}s rate={:.0}/s",
                t0.elapsed().as_secs_f64(),
                handled as f64 / t0.elapsed().as_secs_f64()
            );
        }
    }
    wr.flush()?;
    Ok(())
}

fn client_ping(rounds: usize, port: u16) -> std::io::Result<()> {
    let mut s = TcpStream::connect(("127.0.0.1", port))?;
    s.set_nodelay(true)?;
    let mut rd = BufReader::new(s.try_clone()?);
    let mut rtts: Vec<Duration> = Vec::with_capacity(rounds);
    for i in 0..rounds {
        let payload = if i % 2 == 0 {
            format!("B|btcusdt|100.00|1|oP{i}\n")
        } else {
            format!("S|btcusdt|99.99|1|oP{i}\n")
        };
        let t = Instant::now();
        s.write_all(payload.as_bytes())?;
        let mut line = String::new();
        rd.read_line(&mut line)?;
        rtts.push(t.elapsed());
    }
    let mut sorted = rtts.clone();
    sorted.sort();
    let sum: u128 = rtts.iter().map(|d| d.as_micros()).sum();
    let avg = sum as f64 / rtts.len() as f64;
    let p50 = sorted[sorted.len() / 2].as_micros();
    let p99i = ((sorted.len() as f64 * 0.99) as usize).min(sorted.len() - 1);
    let p99 = sorted[p99i].as_micros();
    let min = sorted[0].as_micros();
    println!(
        "[ping] rounds={} RTT avg={avg:.1}µs p50={p50}µs p99={p99}µs min={min}µs",
        rtts.len()
    );
    Ok(())
}

fn client_batch(n: usize, port: u16) -> std::io::Result<()> {
    let mut s = TcpStream::connect(("127.0.0.1", port))?;
    s.set_nodelay(true)?;
    let mut rd = BufReader::new(s.try_clone()?);
    let mut buf = String::with_capacity(n * 32);
    for i in 0..n {
        if i % 2 == 0 {
            buf.push_str(&format!("B|btcusdt|100.00|1|oB{i}\n"));
        } else {
            buf.push_str(&format!("S|btcusdt|99.99|1|oB{i}\n"));
        }
    }
    let t = Instant::now();
    s.write_all(buf.as_bytes())?;
    let mut reports = 0u64;
    let mut line = String::new();
    while reports < n as u64 {
        line.clear();
        if rd.read_line(&mut line)? == 0 {
            break;
        }
        reports += 1;
    }
    let elapsed = t.elapsed();
    let rate = reports as f64 / elapsed.as_secs_f64();
    println!(
        "[batch] orders={n} reports={reports} rate={rate:.0}/s elapsed={:.3}s",
        elapsed.as_secs_f64()
    );
    Ok(())
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mut mode = "batch";
    let mut n = 20000usize;
    let mut port = PORT_DEFAULT;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--ping" => {
                mode = "ping";
                i += 1;
                n = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(500);
            }
            "--batch" => {
                mode = "batch";
                i += 1;
                n = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(20000);
            }
            "--port" => {
                i += 1;
                port = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(PORT_DEFAULT);
            }
            v if v.starts_with("--") => {}
            v => n = v.parse().unwrap_or(20000),
        }
        i += 1;
    }
    println!("=== tcp_order_bench: {mode} n={n} port={port} ===");
    let srv = std::thread::spawn(move || server(port));
    std::thread::sleep(Duration::from_millis(50));
    let res = match mode {
        "ping" => client_ping(n, port),
        _ => client_batch(n, port),
    };
    res?;
    Ok(())
}
