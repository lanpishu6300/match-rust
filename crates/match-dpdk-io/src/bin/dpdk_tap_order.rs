//! `dpdk_tap_order` — DPDK 用户态下单链路实时验证（net_tap vdev）
//!
//! 链路：client（AF_PACKET，内核侧）→ tap0 → DPDK PMD 收包（用户态轮询）
//!   → 解析 Eth/IPv4/UDP + 9B 帧头 → ServerSession（seq 校验/ACK/NAK）
//!   → Engine 撮合 → 回报帧（交换 mac/IP/端口 + 重算 IP 校验和）→ PMD 发包 → tap0 → client
//!
//! 验证目标：DPDK 用户态收发包 + 会话层/撮合在实时链路的往返延迟（容器内
//! net_tap 是能跑的最高保真路径；物理 NIC 见 scripts/dpdk-verify-realnic.sh）。
//!
//! Usage（容器内）:
//!   ip link add ... (无需；vdev 自建 tap0)
//!   cargo run --release -p match-dpdk-io --bin dpdk_tap_order
//!   对端: python3 scripts/tap_udp_client.py <tap_mac> <orders>

use match_core::{Engine, MatchEvent};
use match_dpdk_io::dpdk::port::{rx_burst, setup_port, tx_frame};
use match_dpdk_io::dpdk::{eal_cleanup, eal_init, eth_stats_get, mbuf_pool, mtod, pkt_len, rte_mbuf};
use match_protocol::{
    type_convert_spot, MqOrder, ORDER_FORM_LIMIT, ORDER_TYPE_BUY, ORDER_TYPE_SELL,
};
use match_udp_order::packet::{self, t};
use match_udp_order::session::{ServerEvent, ServerSession, REPORT_ACCEPTED, REPORT_EXECUTED};
use std::time::{Duration, Instant};

const BURST: usize = 64;

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

/// 入包 → (tap_mac, src_mac, src_ip, dst_ip, src_port, dst_port, payload_off)
struct Frame {
    dst_mac: [u8; 6],
    src_mac: [u8; 6],
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    payload_off: usize,
    payload_len: usize,
}

unsafe fn parse_frame(m: *const rte_mbuf) -> Option<Frame> {
    let base = mtod(m);
    let total = pkt_len(m);
    if total < 14 + 20 + 8 {
        return None;
    }
    let ether_type = u16::from_be_bytes([*base.add(12), *base.add(13)]);
    if ether_type != 0x0800 {
        return None;
    }
    let ip = base.add(14);
    if *ip.add(9) != 17 {
        return None;
    }
    let ihl = (*ip & 0x0f) as usize * 4;
    let udp = ip.add(ihl);
    let payload_off = 14 + ihl + 8;
    let udp_len = u16::from_be_bytes([*udp.add(4), *udp.add(5)]) as usize;
    if udp_len < 8 || payload_off + (udp_len - 8) > total {
        return None;
    }
    let mut dst_mac = [0u8; 6];
    let mut src_mac = [0u8; 6];
    let eth = std::slice::from_raw_parts(base, 14);
    let ip_bytes = std::slice::from_raw_parts(ip, 20);
    dst_mac.copy_from_slice(&eth[0..6]);
    src_mac.copy_from_slice(&eth[6..12]);
    let mut src_ip = [0u8; 4];
    let mut dst_ip = [0u8; 4];
    src_ip.copy_from_slice(&ip_bytes[12..16]);
    dst_ip.copy_from_slice(&ip_bytes[16..20]);
    Some(Frame {
        dst_mac,
        src_mac,
        src_ip,
        dst_ip,
        src_port: u16::from_be_bytes([*udp.add(0), *udp.add(1)]),
        dst_port: u16::from_be_bytes([*udp.add(2), *udp.add(3)]),
        payload_off,
        payload_len: udp_len - 8,
    })
}

/// 组回包：交换 mac/IP/端口，UDP 校验和置 0，IP 校验和重算
fn build_reply(f: &Frame, payload: &[u8]) -> Vec<u8> {
    let total = 14 + 20 + 8 + payload.len();
    let mut b = Vec::with_capacity(total);
    // Eth
    b.extend_from_slice(&f.src_mac); // dst = 入包 src
    b.extend_from_slice(&f.dst_mac); // src = 入包 dst（tap mac）
    b.extend_from_slice(&[0x08, 0x00]);
    // IPv4
    b.push(0x45);
    b.push(0);
    // total-14 必须转 u16（usize::to_be_bytes() 是 8 字节，会写坏 IP 头）
    b.extend_from_slice(&((total - 14) as u16).to_be_bytes());
    b.extend_from_slice(&[0, 0, 0, 0]); // id, flags/frag
    b.push(64); // ttl
    b.push(17); // udp
    b.extend_from_slice(&[0, 0]); // checksum 占位
    b.extend_from_slice(&f.dst_ip); // src = 入包 dst
    b.extend_from_slice(&f.src_ip); // dst = 入包 src
    // IPv4 checksum (1's complement)
    let ip_sum = checksum(&b[14..34]);
    b[24] = (ip_sum >> 8) as u8;
    b[25] = (ip_sum & 0xff) as u8;
    // UDP
    b.extend_from_slice(&f.dst_port.to_be_bytes());
    b.extend_from_slice(&f.src_port.to_be_bytes());
    b.extend_from_slice(&((8 + payload.len()) as u16).to_be_bytes());
    b.extend_from_slice(&[0, 0]); // checksum = 0（合法）
    b.extend_from_slice(payload);
    b
}

fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn main() -> Result<(), String> {
    // 模式：默认 net_tap；DPDK_VDEV=net_pcap0,rx_pcap=in.pcap,tx_pcap=out.pcap 走文件回放
    let vdev = std::env::var("DPDK_VDEV")
        .unwrap_or_else(|_| "--vdev=net_tap0,iface=tap0".to_string());
    let extra = [vdev];
    let extra_refs: Vec<&str> = extra.iter().map(|s| s.as_str()).collect();
    let consumed = eal_init(&[], &extra_refs)?;
    eprintln!("[dpdk] EAL initialized (consumed {consumed} args) vdev={}", extra[0]);
    let pool = mbuf_pool("tap_pool")?;
    let port: u16 = 0;
    unsafe { setup_port(port, pool)? };
    eprintln!("[dpdk] port {port} up (net_tap0 → iface tap0)");

    let mut srv = ServerSession::new(1 << 15);
    let mut engine = Engine::new();
    let mut mbufs: Vec<*mut rte_mbuf> = vec![std::ptr::null_mut(); BURST];
    let mut processed = 0u64;
    let mut frames = 0u64;
    let mut tap_mac: Option<[u8; 6]> = None;
    let t0 = Instant::now();

    loop {
        let n = unsafe { rx_burst(port, &mut mbufs) };
        if n == 0 {
            continue;
        }
        for i in 0..n as usize {
            let m = mbufs[i];
            unsafe {
                let Some(f) = parse_frame(m) else {
                    frames += 1;
                    continue;
                };
                if tap_mac.is_none() {
                    tap_mac = Some(f.dst_mac);
                    eprintln!(
                        "[dpdk] tap mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                        f.dst_mac[0], f.dst_mac[1], f.dst_mac[2], f.dst_mac[3], f.dst_mac[4], f.dst_mac[5]
                    );
                }
                let payload =
                    std::slice::from_raw_parts(mtod(m).add(f.payload_off), f.payload_len);
                let (evs, outs) = srv.on_datagram(payload);
                let mut replies: Vec<Vec<u8>> = Vec::new();
                for ev in evs {
                    match ev {
                        ServerEvent::Hello(cid, last) => {
                            eprintln!("[dpdk] HELLO cid={cid} last={last} outs={}", outs.len());
                        }
                        ServerEvent::Order(cli_seq, body) => {
                            if let Some((mq, no)) = parse_order(body) {
                                let bb = match_core::BbOrder(type_convert_spot(&mq).expect("convert"));
                                let evs2 = engine.on_order(bb);
                                processed += 1;
                                if evs2.is_empty() {
                                    let text = format!("A|{no}|btcusdt");
                                    let mut rp = Vec::with_capacity(1 + text.len());
                                    rp.push(REPORT_ACCEPTED);
                                    rp.extend_from_slice(text.as_bytes());
                                    replies.extend(srv.ack_and_report(cli_seq, &rp));
                                } else {
                                    for e in &evs2 {
                                        let (rt, text) = report_text(e);
                                        let mut rp = Vec::with_capacity(1 + text.len());
                                        rp.push(rt);
                                        rp.extend_from_slice(&text);
                                        replies.extend(srv.ack_and_report(cli_seq, &rp));
                                    }
                                }
                            } else {
                                replies.extend(srv.reject_order(cli_seq));
                            }
                        }
                        _ => {}
                    }
                }
                if processed % 1000 == 0 {
                    let st = unsafe { eth_stats_get(port) };
                    eprintln!(
                        "[dpdk] stats rx={} tx={} proc={}",
                        st.ipackets, st.opackets, processed
                    );
                }
                for out in outs.into_iter().chain(replies) {
                    let frame = build_reply(&f, &out);
                    if let Err(e) = tx_frame(port, pool, &frame) {
                        eprintln!("[dpdk] tx fail: {e}");
                    }
                }
            }
            unsafe { match_dpdk_io::dpdk::rte_pktmbuf_free(m) };
        }
        if processed > 0 && processed % 5000 == 0 {
            eprintln!(
                "[dpdk] processed={processed} t={:.3}s rate={:.0}/s",
                t0.elapsed().as_secs_f64(),
                processed as f64 / t0.elapsed().as_secs_f64()
            );
        }
        // pcap 文件回放模式：达到 EXPECT 笔数即退出；否则空闲 watchdog
        let expect = std::env::var("EXPECT").ok().and_then(|s| s.parse::<u64>().ok());
        if let Some(e) = expect {
            if processed >= e {
                eprintln!("[dpdk] EXPECT {e} reached, stopping");
                break;
            }
        }
        // pcap 文件回放模式：文件读尽后空闲 5s 即退出；tap 模式 600s
        let idle = if extra[0].contains("pcap") { 5 } else { 600 };
        if t0.elapsed() > Duration::from_secs(idle) {
            eprintln!("[dpdk] {idle}s idle watchdog, stopping");
            break;
        }
    }
    eal_cleanup();
    Ok(())
}
