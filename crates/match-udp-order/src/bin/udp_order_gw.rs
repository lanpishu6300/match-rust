//! `udp_order_gw` — 私有 UDP 下单 → 撮核（match-core::Engine）→ 回报闭环演示
//!
//! 验证阶段 3（撮核侧接入 UDP 下单端口）：
//! - UDP 服务端：`ServerSession`（seq 校验 + ORDER_ACK + 重传窗口）接到订单，
//!   转 `MqOrder → BbOrder → Engine::on_order`，把 Accepted/Executed 编成 REPORT 回报；
//! - UDP 客户端：`ClientSession` 交替发买单/卖单（保证成交），统计 RTT 与成交数；
//! - `--nak-test`：客户端故意丢弃 seq=1 的回报，验证 NAK 补帧闭环。
//!
//! Usage: udp_order_gw [orders] [--nak-test]
//! 订单 payload: "B|btcusdt|100.00|1|o123"（side B/S | symbol | price | qty | order_no）
//! REPORT payload: [type u8][文本]；ACCEPTED 回报 = "A|order_no|symbol"，EXECUTED = "E|taker|maker|price|qty"

use match_core::{Engine, MatchEvent};
use match_protocol::{
    type_convert_spot, MqOrder, ORDER_FORM_LIMIT, ORDER_TYPE_BUY, ORDER_TYPE_SELL,
};
use match_udp_order::packet::{self, t};
use match_udp_order::session::{
    ClientEvent, ClientSession, ServerEvent, ServerSession, REPORT_ACCEPTED, REPORT_EXECUTED,
};
use std::collections::HashMap;
use std::net::UdpSocket;
use std::os::unix::io::AsRawFd;
use std::time::{Duration, Instant};

const PORT_DEFAULT: u16 = 55040;
const BIND: &str = "127.0.0.1";

/// 放大内核 UDP buffer（macOS 默认 ~786KB，8k+ 订单回报突发 ~720KB 会溢出丢包；
/// 真机/生产用 DPDK 大内存池或 sysctl 调大，此处 demo 直接 setsockopt）
fn bump_sockbuf(sock: &UdpSocket, bytes: usize) {
    unsafe {
        let fd = sock.as_raw_fd();
        let val: libc::c_int = bytes as libc::c_int;
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVBUF,
            &val as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_SNDBUF,
            &val as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
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

/// 解析订单 payload → MqOrder
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

fn report_text(ev: &MatchEvent) -> (u8, Vec<u8>) {
    match ev {
        MatchEvent::Fill {
            taker_order_no,
            maker_order_no,
            price,
            qty,
            ..
        } => (
            REPORT_EXECUTED,
            format!("E|{taker_order_no}|{maker_order_no}|{price}|{qty}").into_bytes(),
        ),
        MatchEvent::Revoke {
            order_no, symbol, ..
        } => (
            REPORT_ACCEPTED,
            format!("A|{order_no}|{symbol}").into_bytes(),
        ),
    }
}

fn server_main(orders_target: usize, port: u16) -> std::io::Result<u64> {
    let sock = UdpSocket::bind((BIND, port))?;
    bump_sockbuf(&sock, 4 * 1024 * 1024);
    sock.set_nonblocking(true)?;
    // demo：store_cap 跟随订单数（真实系统窗口有限，evict 后靠 GAP_TOO_OLD 全量重建）
    let mut srv = ServerSession::new(if orders_target > 0 {
        orders_target + 8192
    } else {
        8192
    });
    let mut engine = Engine::new();
    let mut buf = [0u8; 65536];
    let mut processed = 0u64;
    let mut fills = 0u64;
    let mut total_evs = 0u64;
    let mut dup_ack = 0u64;
    let start = Instant::now();
    let mut out_buf: Vec<Vec<u8>> = Vec::new();
    // 优雅关闭窗口：processed 达标后继续收尾 3s（幂等处理 client 尾部重发/NAK），再关 socket
    let mut closing_at: Option<Instant> = None;

    while closing_at.map_or(true, |t| Instant::now() < t) {
        let mut got = false;
        loop {
            match sock.recv_from(&mut buf) {
                Ok((n, peer)) => {
                    got = true;
                    let (evs, mut out) = srv.on_datagram(&buf[..n]);
                    total_evs += evs.len() as u64;
                    out_buf.append(&mut out);
                    for ev in evs {
                        match ev {
                            ServerEvent::Order(cli_seq, payload) => {
                                if processed < 10 {
                                    eprintln!("[s] ORDER seq={cli_seq}");
                                }
                                if let Some((mq, no)) = parse_order(&payload) {
                                    let bb = match_core::BbOrder(
                                        type_convert_spot(&mq).expect("convert"),
                                    );
                                    let evs2 = engine.on_order(bb);
                                    processed += 1;
                                    for e in &evs2 {
                                        if matches!(e, MatchEvent::Fill { .. }) {
                                            fills += 1;
                                        }
                                        let (rt, text) = report_text(e);
                                        let mut rp = Vec::with_capacity(1 + text.len());
                                        rp.push(rt);
                                        rp.extend_from_slice(&text);
                                        out_buf.extend(srv.ack_and_report(cli_seq, &rp));
                                    }
                                    if evs2.is_empty() {
                                        // 无成交也无撤销 → 挂单 Accepted
                                        let text = format!("A|{no}|btcusdt");
                                        let mut rp = Vec::with_capacity(1 + text.len());
                                        rp.push(REPORT_ACCEPTED);
                                        rp.extend_from_slice(text.as_bytes());
                                        out_buf.extend(srv.ack_and_report(cli_seq, &rp));
                                    }
                                } else {
                                    out_buf.extend(srv.reject_order(cli_seq));
                                }
                            }
                            _ => {}
                        }
                    }
                    // debug：server 端记录 NAK_REQUEST 的补帧范围
                    for dg in &out_buf {
                        let _ = sock.send_to(dg, peer);
                    }
                    out_buf.clear();
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        if orders_target > 0 && processed >= orders_target as u64 && closing_at.is_none() {
            closing_at = Some(Instant::now() + Duration::from_secs(3));
        }
        if got {
            if processed > 0 && processed % 5000 == 0 {
                eprintln!(
                    "[server] processed={processed} fills={fills} evs={total_evs} t={:.2}s",
                    start.elapsed().as_secs_f64()
                );
            }
            continue;
        }
        std::thread::sleep(Duration::from_micros(100));
    }
    let _ = sock;
    eprintln!("[server] DONE processed={processed} evs={total_evs}");
    Ok(fills)
}

fn client_main(orders: usize, nak_test: bool, port: u16) -> std::io::Result<()> {
    let sock = UdpSocket::bind((BIND, 0))?;
    bump_sockbuf(&sock, 4 * 1024 * 1024);
    sock.set_nonblocking(true)?;
    sock.connect((BIND, port))?;
    let mut cli = ClientSession::new();
    let mut rtts: Vec<u128> = Vec::with_capacity(orders);
    let mut sent_ts: HashMap<String, Instant> = HashMap::new();
    let mut buf = [0u8; 65536];
    let mut out_buf: Vec<Vec<u8>> = Vec::new();
    let mut reports = 0u64;
    let mut executed = 0u64;
    let mut nak_issued = 0u64;
    let mut dropped_seq1 = nak_test; // --nak-test：丢弃 seq=1 的回报

    // 重连水位（fresh session：last=0）
    out_buf.push(cli.send_hello(42, 0));

    for i in 0..orders {
        let side = if i % 2 == 0 { "B" } else { "S" };
        // 买方挂 100.00，卖方挂 99.99 → 必有交叉
        let price = if side == "B" { "100.00" } else { "99.99" };
        let no = format!("o{i:06}");
        let payload = format!("{side}|btcusdt|{price}|1|{no}");
        sent_ts.insert(no, Instant::now());
        out_buf.push(cli.send_order(payload.as_bytes()));
    }

    let t0 = Instant::now();
    let mut next = 0usize; // 下一个待发帧索引（滑动窗口）
    let target_reports = orders as u64; // 每笔订单恰好 1 条回报（EXECUTED 或 ACCEPTED）
    let mut last_report_at = Instant::now();
    let mut loop_no = 0u64;

    while reports < target_reports {
        loop_no += 1;
        if loop_no % 10 == 0 {
            eprintln!(
                "[client] loop={} t={:.3}s reports={} pending={} next={} total={}",
                loop_no,
                t0.elapsed().as_secs_f64(),
                reports,
                cli.pending_len(),
                next,
                out_buf.len()
            );
        }
        // 1) 发送下一窗（最多 128 帧，UDP 无流控，窗口内回报 3×≈384 帧，稳定不丢包）
        let mut sent = 0;
        while next < out_buf.len() && sent < 128 {
            let _ = sock.send(&out_buf[next]);
            next += 1;
            sent += 1;
        }
        // 2) 回报停滞检测：尾部 REPORT 丢失时（无后续帧触发缺口），立即 NAK 补帧
        //    count=256（响应 <4KB，远小于 UDP 64KB 上限，避免响应被截断丢弃）
        if reports > 0 && last_report_at.elapsed() > Duration::from_millis(200) {
            let _ = sock.send(&cli.nak_request(cli.expected_report_seq(), 256));
            last_report_at = Instant::now();
        }
        // 3) 收并处理本窗回报（NAK/重发输出追加到 out_buf 尾部；50ms 窗口 > server 处理 512 笔时间，防积压）
        let rx_deadline = Instant::now() + Duration::from_millis(50);
        loop {
            if Instant::now() > rx_deadline {
                break;
            }
            match sock.recv_from(&mut buf) {
                Ok((n, _)) => {
                    // --nak-test：在交给会话层前模拟真实丢包（seq=1 的回报直接丢弃）
                    if dropped_seq1 {
                        if let Some((h, _)) = packet::decode(&buf[..n]) {
                            if h.mtype == t::REPORT && h.seq == 1 {
                                dropped_seq1 = false;
                                continue;
                            }
                        }
                    }
                    let (evs, mut out) = cli.on_datagram(&buf[..n]);
                    out_buf.append(&mut out);
                    for ev in evs {
                        match ev {
                            ClientEvent::OrderAck(cli_seq, result) => {
                                if result == packet::ACK_RETRY {
                                    // 乱序 → 立即重发
                                    let _ = cli_seq;
                                }
                            }
                            ClientEvent::Report(seq, payload) => {
                                if reports < 10 {
                                    eprintln!("[c] REPORT seq={seq}");
                                }
                                last_report_at = Instant::now();
                                let rt = payload[0];
                                let text = String::from_utf8_lossy(&payload[1..]).to_string();
                                if rt == REPORT_EXECUTED {
                                    executed += 1;
                                }
                                // 用订单号关联 RTT：E/A 回报第 2 段为订单号
                                if let Some((_, rest)) = text.split_once('|') {
                                    if let Some(no) = rest.split('|').next() {
                                        if let Some(ts) = sent_ts.remove(no) {
                                            rtts.push(ts.elapsed().as_nanos());
                                        }
                                    }
                                }
                                let _ = seq;
                                reports += 1;
                            }
                            ClientEvent::HelloAck(server_last, store_start, flags) => {
                                eprintln!(
                                    "[client] hello ack: server_last={server_last} store_start={store_start} flags={flags}"
                                );
                                if flags != 0 {
                                    eprintln!("[client] GAP_TOO_OLD → full state rebuild required (v1: reset)");
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }

        // 超时重发（2s 窗口；正常回环 <1ms，不会触发）
        // 超时重发（2s 窗口；正常回环 <1ms，不会触发）——立即发送，不排订单队尾
        for dg in cli.retransmit_due(Duration::from_secs(2)) {
            let _ = sock.send(&dg);
        }

        if t0.elapsed() > Duration::from_secs(30) {
            eprintln!("[client] timeout: reports={reports} sent={next}");
            break;
        }
        // 无条件节流：每轮至多发 2048 帧，给 server 消化时间，防 UDP 无流控丢包
        std::thread::sleep(Duration::from_micros(100));
    }

    let elapsed = t0.elapsed();
    let rate = reports as f64 / elapsed.as_secs_f64();
    rtts.sort_unstable();
    let n = rtts.len();
    let p50 = if n > 0 { rtts[n / 2] } else { 0 };
    let p99 = if n > 0 { rtts[(n as f64 * 0.99) as usize] } else { 0 };
    println!("[client] orders={orders} reports={reports} executed={executed} nak={nak_issued} rate={rate:.0}/s elapsed={:.3}s RTT p50={:.1}µs p99={:.1}µs", elapsed.as_secs_f64(), p50 as f64 / 1000.0, p99 as f64 / 1000.0);
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut orders: usize = 2000;
    let mut port = PORT_DEFAULT;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--nak-test" | "--ping" => {}
            "--port" => {
                if i + 1 < args.len() {
                    port = args[i + 1].parse().unwrap_or(PORT_DEFAULT);
                    i += 1;
                }
            }
            s => {
                if let Ok(n) = s.parse() {
                    orders = n;
                }
            }
        }
        i += 1;
    }
    let nak_test = args.iter().any(|a| a == "--nak-test");
    let ping_mode = args.iter().any(|a| a == "--ping");
    println!("=== match-udp-order: UDP 下单 → Engine 撮合 → 回报（orders={orders} nak_test={nak_test} ping={ping_mode} port={port}）===");
    let orders_gw = if ping_mode { 0 } else { orders }; // ping 模式 server 常驻
    let server = std::thread::spawn(move || {
        let _ = server_main(orders_gw, port);
    });
    std::thread::sleep(Duration::from_millis(100));
    if ping_mode {
        let _ = client_ping(64, port);
    } else {
        let _ = client_main(orders, nak_test, port);
    }
    // 不 join server：client 完成后直接退出，防残留进程占端口
}

/// 单笔在飞模式：发一笔等回报，测真实端到端 RTT（做市链路核心指标）
fn client_ping(rounds: usize, port: u16) -> std::io::Result<()> {
    let sock = UdpSocket::bind((BIND, 0))?;
    bump_sockbuf(&sock, 4 * 1024 * 1024);
    sock.set_nonblocking(true)?;
    sock.connect((BIND, port))?;
    let mut cli = ClientSession::new();
    let mut buf = [0u8; 65536];
    let mut rtts: Vec<u128> = Vec::with_capacity(rounds);
    let mut out_buf: Vec<Vec<u8>> = Vec::new();
    out_buf.push(cli.send_hello(42, 0));

    for round in 0..rounds {
        // 发送 hello 或等待 ack
        while out_buf.len() > 0 {
            for dg in out_buf.drain(..) {
                let _ = sock.send(&dg);
            }
            let mut got_ack = false;
            let t_wait = Instant::now();
            while t_wait.elapsed() < Duration::from_millis(500) {
                match sock.recv_from(&mut buf) {
                    Ok((n, _)) => {
                        let (evs, mut out) = cli.on_datagram(&buf[..n]);
                        out_buf.append(&mut out);
                        for ev in evs {
                            if matches!(ev, ClientEvent::HelloAck(..)) {
                                eprintln!("[ping] hello ack received");
                                got_ack = true;
                            }
                        }
                        if got_ack {
                            break;
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_micros(50));
                    }
                    Err(e) => return Err(e),
                }
            }
            if !got_ack {
                eprintln!("[ping] hello ack timeout");
                break;
            }
        }

        let side = if round % 2 == 0 { "B" } else { "S" };
        let price = if side == "B" { "100.00" } else { "99.99" };
        let no = format!("p{round:03}");
        let payload = format!("{side}|btcusdt|{price}|1|{no}");
        let t0 = Instant::now();
        let dg = cli.send_order(payload.as_bytes());
        let _ = sock.send(&dg);

        let mut done = false;
        let t_wait = Instant::now();
        while t_wait.elapsed() < Duration::from_millis(1000) {
            match sock.recv_from(&mut buf) {
                Ok((n, _)) => {
                    let (evs, mut out) = cli.on_datagram(&buf[..n]);
                    out_buf.append(&mut out);
                    for ev in evs {
                        if let ClientEvent::Report(_, _) = ev {
                            rtts.push(t0.elapsed().as_nanos());
                            done = true;
                        }
                    }
                    if done {
                        break;
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_micros(50));
                }
                Err(e) => return Err(e),
            }
        }
        if !done {
            eprintln!("[ping] round {round} timeout");
        }
    }

    if rtts.is_empty() {
        println!("[ping] no RTT samples");
        return Ok(());
    }
    rtts.sort_unstable();
    let n = rtts.len();
    let p50 = rtts[n / 2];
    let p99 = rtts[(n as f64 * 0.99) as usize];
    let avg = rtts.iter().sum::<u128>() / n as u128;
    println!(
        "[ping] rounds={rounds} samples={n} RTT avg={:.1}µs p50={:.1}µs p99={:.1}µs min={:.1}µs",
        avg as f64 / 1000.0,
        p50 as f64 / 1000.0,
        p99 as f64 / 1000.0,
        rtts[0] as f64 / 1000.0
    );
    Ok(())
}
