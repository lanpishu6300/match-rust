//! `soup_cmp_bench` — SoupBinTCP 下单性能对比：协议层开销 vs 裸 TCP，单条 vs
//! 批量 vs 多成交，输出 RTT p50/p99/mean + 吞吐。
//!
//! Scenarios (same process, kernel TCP loopback, server thread + client):
//!   echo        裸 TCP echo 基线 —— server 原样回显, 无协议层/无会话状态
//!   single      SoupBinTCP: 登录 + 每笔 1 订单 → 1 Accepted (Sequenced)
//!   batch-N     SoupBinTCP: 每写一批 N 笔订单, 回报按序聚合
//!   fills-N     SoupBinTCP: 每笔订单 → 1 Accepted + (N-1) Executed
//!
//! Usage: soup_cmp_bench <scenario> <orders> [N]

#[cfg(target_os = "linux")]
mod real {
    use match_soupbintcp::ouch::{self, Accepted, EnterOrder, Executed};
    use match_soupbintcp::packet::{self, Packet};
    use match_soupbintcp::session::{ClientSession, ServerSession};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    const USER: &str = "trader";
    const PASS: &str = "secret01";
    const SESSION: &str = "SESSION-A";
    const PORT: u16 = 55030;

    fn now_ns() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }

    fn read_some(s: &mut TcpStream, b: &mut [u8]) -> std::io::Result<usize> {
        s.read(b)
    }

    fn ouch_enter(i: u64) -> Vec<u8> {
        let m = EnterOrder {
            token: format!("ORD-{i:06}"),
            side: b'B',
            shares: 100,
            stock: "AAPL".into(),
            price: 195_5000,
            time_in_force: 99998,
            firm: "ABCD".into(),
            display: b'Y',
            capacity: b'A',
            intermarket_sweep: b'N',
            min_qty: 0,
            cross_type: b'N',
            customer_type: b' ',
        };
        ouch::encode_enter(&m)
    }

    fn ouch_accepted(i: u64) -> Vec<u8> {
        let m = Accepted {
            timestamp: now_ns(),
            token: format!("ORD-{i:06}"),
            side: b'B',
            shares: 100,
            stock: "AAPL".into(),
            price: 195_5000,
            time_in_force: 99998,
            firm: "ABCD".into(),
            display: b'Y',
            ref_number: 1_000_000 + i,
            order_state: b'L',
        };
        ouch::encode_accepted(&m)
    }

    fn ouch_executed(i: u64, fill_no: u64) -> Vec<u8> {
        let m = Executed {
            timestamp: now_ns(),
            token: format!("ORD-{i:06}"),
            shares: 25,
            match_number: (fill_no as u32) + 1,
            price: 195_5000,
        };
        ouch::encode_executed(&m)
    }

    fn summarize(name: &str, orders: usize, elapsed: Duration, rtts: &[u128]) {
        let mut sorted = rtts.to_vec();
        sorted.sort_unstable();
        let n = sorted.len();
        let p50 = sorted[n / 2];
        let p99 = sorted[(n as f64 * 0.99) as usize];
        let mean: u128 = sorted.iter().sum::<u128>() / n.max(1) as u128;
        let rate = orders as f64 / elapsed.as_secs_f64();
        println!(
            "{name:>12} orders={orders:7} elapsed={:7.3}s rate={:9.0}/s  RTT p50={:6.1}µs p99={:7.1}µs mean={:6.1}µs",
            elapsed.as_secs_f64(),
            rate,
            p50 as f64 / 1000.0,
            p99 as f64 / 1000.0,
            mean as f64 / 1000.0
        );
    }

    enum Scenario {
        Echo,
        Single,
        Batch(usize),
        Fills(usize),
    }

    /// Server side per scenario.
    fn server_main(scenario: Scenario, orders: usize, fills: usize) -> std::io::Result<()> {
        let listener = TcpListener::bind(("127.0.0.1", PORT))?;
        let (mut stream, _) = listener.accept()?;
        stream.set_nodelay(true)?;
        let mut srv = ServerSession::new(SESSION, 65536).map_err(std::io::Error::other)?;
        let mut buf = [0u8; 65536];
        let mut acc: Vec<u8> = Vec::with_capacity(65536);
        let mut orders_done = 0usize;

        // login
        let mut logged_in = false;
        while !logged_in {
            let (pkts, consumed, _) = packet::parse_stream(&acc);
            acc.drain(..consumed);
            let mut handled = false;
            for p in &pkts {
                handled = true;
                if p.is_login_request() {
                    let req = p.parse_login_request();
                    let acts = srv.handle_login(&req).map_err(std::io::Error::other)?;
                    for op in &acts.outbound {
                        stream.write_all(&op.to_bytes())?;
                    }
                    logged_in = true;
                }
            }
            if handled {
                continue;
            }
            let n = read_some(&mut stream, &mut buf)?;
            if n == 0 {
                break;
            }
            acc.extend_from_slice(&buf[..n]);
        }

        // orders
        while orders_done < orders {
            let (pkts, consumed, _) = packet::parse_stream(&acc);
            acc.drain(..consumed);
            let mut handled = false;
            for p in &pkts {
                handled = true;
                match &scenario {
                    Scenario::Echo => {
                        // raw echo: return the same payload, no session state
                        let mut out = Vec::with_capacity(p.to_bytes().len());
                        out.extend_from_slice(&p.to_bytes());
                        stream.write_all(&out)?;
                        orders_done += 1;
                    }
                    _ => {
                        if p.packet_type == packet::UNSEQUENCED_DATA {
                            let idx: u64 = String::from_utf8_lossy(&p.payload[1..15])
                                .trim()
                                .trim_start_matches("ORD-")
                                .parse()
                                .unwrap_or(0);
                            let mut out = Vec::with_capacity(fills * 70);
                            for f in 0..fills {
                                let rep = if f == 0 {
                                    ouch_accepted(idx)
                                } else {
                                    ouch_executed(idx, f as u64)
                                };
                                let _seq = srv.enqueue(&rep);
                                out.extend_from_slice(&srv.sequenced_packet(&rep).to_bytes());
                            }
                            stream.write_all(&out)?;
                            orders_done += 1;
                        }
                    }
                }
            }
            if handled {
                continue;
            }
            let n = read_some(&mut stream, &mut buf)?;
            if n == 0 {
                break;
            }
            acc.extend_from_slice(&buf[..n]);
        }
        Ok(())
    }

    fn client_main(scenario: &Scenario, orders: usize) -> std::io::Result<Vec<u128>> {
        let mut client = ClientSession::new(SESSION);
        let mut stream = TcpStream::connect(("127.0.0.1", PORT))?;
        stream.set_nodelay(true)?;
        let login = match_soupbintcp::packet::login_request(USER, PASS, SESSION, 0);
        stream.write_all(&login.to_bytes())?;

        let mut buf = [0u8; 65536];
        let mut acc: Vec<u8> = Vec::with_capacity(65536);
        // login accepted
        loop {
            let n = read_some(&mut stream, &mut buf)?;
            acc.extend_from_slice(&buf[..n]);
            let (pkts, consumed, _) = packet::parse_stream(&acc);
            acc.drain(..consumed);
            let mut done = false;
            for p in &pkts {
                if p.packet_type == packet::LOGIN_ACCEPTED {
                    client.handle_packet(&p, now_ns()).map_err(std::io::Error::other)?;
                    done = true;
                }
            }
            if done {
                break;
            }
        }

        let (batch, fills) = match scenario {
            Scenario::Echo => (1, 1),
            Scenario::Single => (1, 1),
            Scenario::Batch(n) => (*n, 1),
            Scenario::Fills(n) => (1, *n),
        };

        let mut rtts: Vec<u128> = Vec::with_capacity(orders);
        let mut i = 0usize;
        let mut reports_seen = 0usize;
        let mut order_buf: Vec<u8> = Vec::with_capacity(batch * 60);

        while i < orders {
            let batch_n = batch.min(orders - i);
            order_buf.clear();
            for _ in 0..batch_n {
                let enter = Packet::new(packet::UNSEQUENCED_DATA, ouch_enter(i as u64));
                order_buf.extend_from_slice(&enter.to_bytes());
                i += 1;
            }
            let ts = Instant::now();
            stream.write_all(&order_buf)?;
            let need = reports_seen + batch_n * fills;
            while reports_seen < need {
                let n = read_some(&mut stream, &mut buf)?;
                if n == 0 {
                    break;
                }
                acc.extend_from_slice(&buf[..n]);
                let (pkts, consumed, _) = packet::parse_stream(&acc);
                acc.drain(..consumed);
                for p in pkts {
                    if p.packet_type == packet::SEQUENCED_DATA {
                        client.handle_packet(&p, now_ns()).map_err(std::io::Error::other)?;
                        reports_seen += 1;
                    } else if p.packet_type == packet::UNSEQUENCED_DATA {
                        // echo mode returns the raw order
                        reports_seen += 1;
                    }
                }
            }
            let per_order = ts.elapsed().as_nanos() / batch_n as u128;
            rtts.push(per_order);
        }
        Ok(rtts)
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let mut args = std::env::args().skip(1);
        let scenario_name = args.next().unwrap_or_else(|| "single".into());
        let orders: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(20_000);
        let n: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(1);

        let scenario = match scenario_name.as_str() {
            "echo" => Scenario::Echo,
            "single" => Scenario::Single,
            "batch" => Scenario::Batch(n),
            "fills" => Scenario::Fills(n),
            other => {
                eprintln!("unknown scenario {other}: echo|single|batch|fills");
                std::process::exit(2);
            }
        };
        let fills = match scenario {
            Scenario::Fills(f) => f,
            _ => 1,
        };
        let scenario_for_server = match &scenario {
            Scenario::Batch(_) => Scenario::Batch(1), // server reads whatever arrives
            other => match other {
                Scenario::Echo => Scenario::Echo,
                _ => Scenario::Fills(fills),
            },
        };

        let server = std::thread::spawn(move || {
            let _ = server_main(scenario_for_server, orders, fills);
        });
        std::thread::sleep(Duration::from_millis(50));
        let t0 = Instant::now();
        let rtts = client_main(&scenario, orders)?;
        let elapsed = t0.elapsed();
        let _ = server.join();

        summarize(&scenario_name, orders, elapsed, &rtts);
        Ok(())
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = real::run() {
            eprintln!("soup_cmp_bench error: {e}");
            std::process::exit(1);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("soup_cmp_bench is Linux-only");
        std::process::exit(1);
    }
}
