//! `udp_rss_hp_bench` — UDP RSS 直分 + HpEngine 撮合基准（真实内核 UDP 路径）
//!
//! 模拟生产形态 D 的接收语义：**客户端 UDP 发单 → 内核 UDP 收包（每 shard 独立
//! socket/端口，等价 RSS 分流）→ 解析 → HpEngine 撮合 → ACK 回报**。
//!
//! 与 TCP RSS（tcp_rss_shard_bench，116k/s @8shards）对照，隔离差异：
//! - TCP：连接状态机 + 字节流重组 + 确认/重传（p50 18-62µs）
//! - UDP：无连接、每包独立（预期 p50 2-6µs）——**同内核路径，仅协议差**
//! 再结合 hp_shard_bench（HpEngine 纯撮合 23M/s @1shard）估算 DPDK UDP 全链路。
//!
//! 订单序列同款：k/2 换 symbol、同 symbol 严格 B/S 交替成交（book 稳态）。

use match_core_hp::{HpCommand, HpEngine, Side};
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

const SHARDS_DEFAULT: usize = 4;
const PER_SHARD_DEFAULT: usize = 50_000;
const SYMBOLS_PER_SHARD: usize = 32;
const BASE_PORT: u16 = 57000;
const BUY_TICK: i64 = 100_000;
const SELL_TICK: i64 = 99_990;

// 报文：side(1B) + price_tick(8B) + qty_lot(8B) + client_id(8B) = 25B
const PKT_LEN: usize = 25;

fn encode(side: Side, tick: i64, qty: i64, client_id: u64) -> [u8; PKT_LEN] {
    let mut b = [0u8; PKT_LEN];
    b[0] = match side { Side::Buy => 1, Side::Sell => 2 };
    b[1..9].copy_from_slice(&tick.to_le_bytes());
    b[9..17].copy_from_slice(&qty.to_le_bytes());
    b[17..25].copy_from_slice(&client_id.to_le_bytes());
    b
}

fn decode(b: &[u8; PKT_LEN]) -> (Side, i64, i64, u64) {
    let side = if b[0] == 1 { Side::Buy } else { Side::Sell };
    let tick = i64::from_le_bytes(b[1..9].try_into().unwrap());
    let qty = i64::from_le_bytes(b[9..17].try_into().unwrap());
    let client = u64::from_le_bytes(b[17..25].try_into().unwrap());
    (side, tick, qty, client)
}

/// 服务端：UDP 收包 → HpEngine → ACK
fn server_run(shard_id: usize) -> io::Result<(usize, usize)> {
    let sock = UdpSocket::bind(("127.0.0.1", BASE_PORT + shard_id as u16))?;
    // 客户端发完即退出：recv 超时（50ms）→ idle 累计 → break（否则 UDP 无连接，
    // 阻塞 recv 永远等不到 Err，join 卡死）
    sock.set_read_timeout(Some(Duration::from_millis(50)))?;
    let mut engine = HpEngine::new();
    let mut buf = [0u8; 2048];
    let mut handled = 0usize;
    let mut fills = 0usize;
    let mut idle = 0u32;
    let mut src: Option<SocketAddr> = None;
    loop {
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                src = Some(from);
                if n == PKT_LEN {
                    let (side, tick, qty, client) = decode(buf[..PKT_LEN].try_into().unwrap());
                    let cmd = HpCommand::Limit { side, price_tick: tick, qty_lot: qty, ts: 0, client_id: client };
                    for e in engine.on_order(cmd) {
                        if let match_core_hp::HpEvent::Fill { .. } = e { fills += 1; }
                    }
                    handled += 1;
                    let _ = sock.send_to(&[1u8], from); // ACK
                }
                idle = 0;
            }
            Err(_) => {
                // 超时（TimedOut/WouldBlock）：客户端已停止发送
                idle += 1;
                if idle > 10 { break; }
            }
        }
    }
    let _ = src;
    Ok((handled, fills))
}

/// 客户端：UDP 发单 + 收 ACK（RTT）
fn client_run(shard_id: usize, per_shard: usize) -> io::Result<(usize, Vec<Duration>)> {
    let sock = UdpSocket::bind(("127.0.0.1", 0))?;
    let server: SocketAddr = format!("127.0.0.1:{}", BASE_PORT + shard_id as u16).parse().unwrap();
    let sym_base = shard_id * SYMBOLS_PER_SHARD;
    let mut rtts: Vec<Duration> = Vec::with_capacity(per_shard);
    let mut ack = [0u8; 4];
    for i in 0..per_shard {
        let sym_id = sym_base + ((i / 2) % SYMBOLS_PER_SHARD);
        let (side, tick) = if i % 2 == 0 { (Side::Buy, BUY_TICK) } else { (Side::Sell, SELL_TICK) };
        let client_id = (shard_id as u64) << 32 | i as u64;
        let pkt = encode(side, tick, 1, client_id);
        let t = Instant::now();
        sock.send_to(&pkt, server)?;
        let (n, _) = sock.recv_from(&mut ack)?;
        if n > 0 {
            rtts.push(t.elapsed());
        }
    }
    Ok((rtts.len(), rtts))
}

fn main() -> io::Result<()> {
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
    println!("=== udp_rss_hp_bench: shards={shards} per_shard={per_shard} (total={}) pkt={PKT_LEN}B ===", shards * per_shard);

    let t0 = Instant::now();
    let mut server_handles = Vec::with_capacity(shards);
    let mut client_handles = Vec::with_capacity(shards);
    for k in 0..shards {
        server_handles.push(std::thread::spawn(move || server_run(k)));
    }
    std::thread::sleep(Duration::from_millis(100));
    for k in 0..shards {
        client_handles.push(std::thread::spawn(move || client_run(k, per_shard)));
    }

    let mut handled_total = 0usize;
    let mut fills_total = 0usize;
    for h in server_handles {
        let (n, f) = h.join().expect("server join")?;
        handled_total += n;
        fills_total += f;
    }
    let mut all_rtts = Vec::with_capacity(shards * per_shard);
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
        "[udp-rss-hp] sent={sent_total} handled={handled_total} fills={fills_total} elapsed={:.3}s rate={rate:.0}/s shards={shards}",
        elapsed.as_secs_f64()
    );
    println!("[udp-rss-hp-latency] p50={p50}µs p99={p99}µs (client RTT, per-order, loopback)");
    Ok(())
}
