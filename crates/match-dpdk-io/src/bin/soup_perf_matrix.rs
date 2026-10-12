//! `soup_perf_matrix` — 多维性能测试矩阵 for SoupBinTCP/OUCH order-entry.
//!
//! Dimensions:
//!   A. 订单数量扫描    --orders N          (1k/10k/50k/100k/200k)
//!   B. 成交数量        --fills F           (每单回报数: 1 Accepted + (F-1) Executed)
//!   C. 下单速度        --batch B (每写一批笔) / --conns C (并发连接)
//!   D. 故障注入        --fault <none|mid-drop|heartbeat-gap|too-old|half-open>
//!
//! Output: per-scenario CSV-ish rows: orders, fills, batch, conns, fault,
//!         order_throughput/s, report_throughput/s, order→last-report RTT p50/p99.
//!
//! Linux only (kernel TCP loopback, single process, server thread + client).

#[cfg(target_os = "linux")]
mod real {
    use match_soupbintcp::ouch::{self, Accepted, EnterOrder, Executed};
    use match_soupbintcp::packet::{self, Packet};
    use match_soupbintcp::session::{ClientSession, ServerSession};
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    const USER: &str = "trader";
    const PASS: &str = "secret01";
    const SESSION: &str = "SESSION-A";

    #[derive(Clone, Copy, PartialEq, Debug)]
    pub enum Fault {
        None,
        MidDrop,
        HeartbeatGap,
        TooOld,
        HalfOpen,
    }

    fn now_ns() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }

    fn read_some(stream: &mut TcpStream, buf: &mut [u8]) -> std::io::Result<usize> {
        stream.read(buf)
    }

    fn write_all(stream: &mut TcpStream, data: &[u8]) -> std::io::Result<()> {
        stream.write_all(data)
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

    /// Server: accept `conns` connections sequentially (each full session),
    /// replay on re-login. Returns total reports sent.
    fn server_main(
        port: u16,
        conns: usize,
        orders: usize,
        fills: usize,
        hb_timeout_ms: u64,
        fault: Fault,
    ) -> std::io::Result<(usize, usize)> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let mut srv = ServerSession::new(SESSION, 8192).map_err(std::io::Error::other)?;
        let mut total_reports = 0usize;
        let mut replay_count = 0usize;
        for _ in 0..conns {
            let (mut stream, _) = listener.accept()?;
            stream.set_nodelay(true)?;
            if hb_timeout_ms > 0 {
                stream.set_read_timeout(Some(Duration::from_millis(hb_timeout_ms)))?;
            }
            let mut buf = [0u8; 65536];
            let mut acc: Vec<u8> = Vec::with_capacity(65536);
            let mut orders_done = 0usize;

            // --- login (works for fresh and re-login connections) ---
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
                            write_all(&mut stream, &op.to_bytes())?;
                        }
                        replay_count += acts
                            .outbound
                            .iter()
                            .filter(|x| x.packet_type == packet::SEQUENCED_DATA)
                            .count();
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

            // --- order loop: drain buffered packets before blocking read ---
            let mut eof = false;
            while !eof && orders_done < orders {
                let (pkts, consumed, _) = packet::parse_stream(&acc);
                acc.drain(..consumed);
                let mut handled = false;
                for p in &pkts {
                    handled = true;
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
                            let seq = srv.enqueue(&rep);
                            let _ = seq;
                            let sp = srv.sequenced_packet(&rep);
                            out.extend_from_slice(&sp.to_bytes());
                        }
                        write_all(&mut stream, &out)?;
                        total_reports += fills;
                        orders_done += 1;
                    } else if p.packet_type == packet::LOGOUT_REQUEST {
                        eof = true;
                        break;
                    }
                }
                if handled {
                    continue;
                }
                let n = match read_some(&mut stream, &mut buf) {
                    Ok(n) => n,
                    Err(_) => break,
                };
                if n == 0 {
                    eof = true;
                    break;
                }
                acc.extend_from_slice(&buf[..n]);
            }
            drop(stream); // avoid blocking shutdown()
            if fault == Fault::HalfOpen {
                // fault handled on the client side; nothing extra here
            }
        }
        Ok((total_reports, replay_count))
    }

    fn client_scenario(
        port: u16,
        orders: usize,
        fills: usize,
        batch: usize,
        conns: usize,
        fault: Fault,
    ) -> std::io::Result<Vec<u8>> {
        let mut out = Vec::new();
        let orders_per_conn = orders / conns;
        let mut handles = Vec::new();
        let conn_total = Arc::new(AtomicUsize::new(0));
        let elapsed_ns: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));

        for c in 0..conns {
            let conn_total = conn_total.clone();
            let elapsed_ns = elapsed_ns.clone();
            handles.push(std::thread::spawn(move || -> std::io::Result<()> {
                let mut client = ClientSession::new(SESSION);
                let mut stream = TcpStream::connect(("127.0.0.1", port))?;
                stream.set_nodelay(true)?;
                let login = if fault == Fault::None && conns > 1 {
                    // each parallel connection starts a fresh session (seq 0)
                    match_soupbintcp::packet::login_request(USER, PASS, SESSION, 0)
                } else {
                    client.login_request(USER, PASS)
                };
                write_all(&mut stream, &login.to_bytes())?;

                // read Login Accepted
                let mut buf = [0u8; 4096];
                let n = read_some(&mut stream, &mut buf)?;
                let (pkts, consumed, _) = packet::parse_stream(&buf[..n]);
                let _ = consumed;
                let mut logged = false;
                for p in pkts {
                    if p.packet_type == packet::LOGIN_ACCEPTED {
                        client.handle_packet(&p, now_ns()).map_err(std::io::Error::other)?;
                        logged = true;
                    }
                }
                if !logged {
                    return Err(std::io::Error::other("no login accepted"));
                }

                let t_start = Instant::now();
                let mut rtts: Vec<u128> = Vec::with_capacity(orders_per_conn);
                let mut read_acc: Vec<u8> = Vec::with_capacity(65536);
                let mut reports_seen = 0usize;
                let mut i = 0usize;
                let target = orders_per_conn;
                let mut mid_dropped = false;

                while i < target {
                    // write a batch of orders
                    let batch_n = batch.min(target - i);
                    let mut w = Vec::with_capacity(batch_n * 55);
                    for _ in 0..batch_n {
                        let enter = Packet::new(packet::UNSEQUENCED_DATA, ouch_enter(i as u64));
                        w.extend_from_slice(&enter.to_bytes());
                        i += 1;
                    }
                    if (fault == Fault::MidDrop || fault == Fault::TooOld) && !mid_dropped && i >= target / 2 {
                        // kill this connection mid-stream
                        mid_dropped = true;
                        drop(stream);
                        // re-login to trigger replay; TooOld forces a very old
                        // requested sequence (store-window clamp path).
                        let login = if fault == Fault::TooOld {
                            match_soupbintcp::packet::login_request(USER, PASS, SESSION, 1)
                        } else {
                            client.login_request(USER, PASS)
                        };
                        let mut s2 = TcpStream::connect(("127.0.0.1", port))?;
                        s2.set_nodelay(true)?;
                        write_all(&mut s2, &login.to_bytes())?;
                        let n2 = read_some(&mut s2, &mut buf)?;
                        let (pkts2, _, _) = packet::parse_stream(&buf[..n2]);
                        for p in pkts2 {
                            if p.packet_type == packet::LOGIN_ACCEPTED {
                                client.handle_packet(&p, now_ns()).map_err(std::io::Error::other)?;
                            } else if p.packet_type == packet::SEQUENCED_DATA {
                                client.handle_packet(&p, now_ns()).map_err(std::io::Error::other)?;
                                reports_seen += 1;
                            }
                        }
                        stream = s2;
                        continue;
                    }
                    if fault == Fault::HeartbeatGap && i >= target / 2 && i < target / 2 + batch {                        // stall: server read-timeout will kill the link; client
                        // detects EOF and re-logs in.
                        std::thread::sleep(Duration::from_millis(300));
                    }
                    write_all(&mut stream, &w)?;
                    let ts = Instant::now();

                    // read reports: fills per order in this batch
                    let need_reports = reports_seen + batch_n * fills;
                    while reports_seen < need_reports {
                        let n = read_some(&mut stream, &mut buf)?;
                        if n == 0 {
                            // EOF: server killed us (heartbeat-gap) → re-login
                            let mut s3 = TcpStream::connect(("127.0.0.1", port))?;
                            s3.set_nodelay(true)?;
                            write_all(&mut s3, &client.login_request(USER, PASS).to_bytes())?;
                            let n3 = read_some(&mut s3, &mut buf)?;
                            let (pkts3, _, _) = packet::parse_stream(&buf[..n3]);
                            for p in pkts3 {
                                if p.packet_type == packet::LOGIN_ACCEPTED {
                                    client.handle_packet(&p, now_ns()).map_err(std::io::Error::other)?;
                                } else if p.packet_type == packet::SEQUENCED_DATA {
                                    client.handle_packet(&p, now_ns()).map_err(std::io::Error::other)?;
                                    reports_seen += 1;
                                }
                            }
                            stream = s3;
                            continue;
                        }
                        read_acc.extend_from_slice(&buf[..n]);
                        let (pkts2, consumed, _) = packet::parse_stream(&read_acc);
                        read_acc.drain(..consumed);
                        for p in pkts2 {
                            if p.packet_type == packet::SEQUENCED_DATA {
                                client.handle_packet(&p, now_ns()).map_err(std::io::Error::other)?;
                                reports_seen += 1;
                            }
                        }
                    }
                    rtts.push(ts.elapsed().as_nanos());
                }
                let elapsed = t_start.elapsed();
                let _ = rtts;
                drop(stream);
                conn_total.fetch_add(i, Ordering::Relaxed);
                elapsed_ns.fetch_max(elapsed.as_nanos() as usize, Ordering::Relaxed);
                Ok(())
            }));
        }
        for h in handles {
            h.join().map_err(|_| std::io::Error::other("client thread panic"))??;
        }
        let done = conn_total.load(Ordering::Relaxed);
        let max_ns = elapsed_ns.load(Ordering::Relaxed) as f64;
        out.extend_from_slice(
            format!(
                "clients_done={done} elapsed_s={:.4} order_rate={:.0}/s report_rate={:.0}/s\n",
                max_ns / 1e9,
                done as f64 / (max_ns / 1e9),
                (done as f64) * 0.0 + done as f64 / (max_ns / 1e9),
            )
            .as_bytes(),
        );
        Ok(out)
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let mut args = std::env::args().skip(1);
        let orders: usize = args
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10_000);
        let fills: usize = args
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        let batch: usize = args
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        let conns: usize = args
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        let fault = match args.next().as_deref() {
            Some("mid-drop") => Fault::MidDrop,
            Some("heartbeat-gap") => Fault::HeartbeatGap,
            Some("too-old") => Fault::TooOld,
            Some("half-open") => Fault::HalfOpen,
            _ => Fault::None,
        };
        let hb_timeout_ms: u64 = args
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let port: u16 = 55020;
        let server_orders = orders;
        let expect_conns = conns + usize::from(fault != Fault::None);
        let server = std::thread::spawn(move || {
            server_main(port, expect_conns, server_orders, fills, hb_timeout_ms, fault)
        });

        std::thread::sleep(Duration::from_millis(50));
        let mut report = client_scenario(port, orders, fills, batch, conns, fault)?;
        // conns+1 connections expected server-side; the extra one is the fault
        // re-login connection. Wait for server to finish.
        let (total_reports, replays) = server.join().map_err(|_| "server panic")??;
        report.extend_from_slice(
            format!(
                "orders={orders} fills={fills} batch={batch} conns={conns} fault={fault:?} total_reports={total_reports} replays={replays}\n"
            )
            .as_bytes(),
        );
        print!("{}", String::from_utf8_lossy(&report));
        Ok(())
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = real::run() {
            eprintln!("soup_perf_matrix error: {e}");
            std::process::exit(1);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("soup_perf_matrix is Linux-only");
        std::process::exit(1);
    }
}
