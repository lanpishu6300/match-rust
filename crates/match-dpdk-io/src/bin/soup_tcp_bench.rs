//! `soup_tcp_bench` — SoupBinTCP over real kernel TCP: end-to-end order-entry
//! round trip (login → enter order → accepted report → heartbeat → re-login
//! replay). Linux only.
//!
//! Scenarios (one process: server thread + client on loopback):
//!   1. Login flow + latency (login → Login Accepted RTT)
//!   2. N order/report round trips (Unsequenced Enter → Sequenced Accepted):
//!      order→accept RTT p50/p99 + throughput
//!   3. Heartbeat both directions (Client 'R', Server 'H' on idle)
//!   4. Re-login "session snapshot": drop connection, re-login with a
//!      requested_sequence, server replays its message store, client verifies
//!      the exact expected sequence.
//!
//! Usage: soup_tcp_bench [N]

#[cfg(target_os = "linux")]
mod real {
    use match_soupbintcp::ouch::{self, Accepted, EnterOrder};
    use match_soupbintcp::packet::{self, Packet, CLIENT_HEARTBEAT};
    use match_soupbintcp::session::{ClientSession, ServerSession};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    const PORT: u16 = 55010;
    const USER: &str = "trader";
    const PASS: &str = "secret01";
    const SESSION: &str = "SESSION-A";

    fn now_ns() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }

    fn read_packet(stream: &mut TcpStream) -> std::io::Result<Packet> {
        let mut lenb = [0u8; 2];
        stream.read_exact(&mut lenb)?;
        let len = u16::from_be_bytes(lenb) as usize;
        let mut body = vec![0u8; len];
        stream.read_exact(&mut body)?;
        Ok(Packet::new(body[0], body[1..].to_vec()))
    }

    fn write_packet(stream: &mut TcpStream, p: &Packet) -> std::io::Result<()> {
        stream.write_all(&p.to_bytes())
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

    /// Server: accept connections until the session is deleted (never here),
    /// handling login + unsequenced orders + heartbeats + logout per conn.
    fn server_loop(listener: TcpListener, n: usize) -> std::io::Result<(u64, Vec<u64>)> {
        let mut srv = ServerSession::new(SESSION, 8192)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let mut total_orders = 0u64;
        let mut replayed = 0u64;
        for _conn in 0..2 {
            let (mut stream, _) = listener.accept()?;
            stream.set_nodelay(true)?;
            let lr = read_packet(&mut stream)?;
            assert!(lr.is_login_request());
            let req = lr.parse_login_request();
            let acts = srv
                .handle_login(&req)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            for p in &acts.outbound {
                write_packet(&mut stream, p)?;
            }
            if req.requested_sequence > 1 {
                // Login Accepted + replay packets were queued above.
                replayed += acts
                    .outbound
                    .iter()
                    .filter(|p| p.packet_type == packet::SEQUENCED_DATA)
                    .count() as u64;
            }
            loop {
                // EOF (client dropped) = end of this connection.
                let pkt = match read_packet(&mut stream) {
                    Ok(p) => p,
                    Err(_) => break,
                };
                let acts = srv
                    .handle_packet(&pkt)
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
                for p in &acts.outbound {
                    write_packet(&mut stream, p)?;
                }
                if acts.close {
                    break;
                }
                if pkt.packet_type == packet::UNSEQUENCED_DATA {
                    let m = ouch::parse(&pkt.payload).expect("OUCH Enter expected");
                    let token = match &m {
                        ouch::OuchMessage::EnterOrder(e) => e.token.clone(),
                        _ => "?".into(),
                    };
                    let idx: u64 = token[4..].parse().unwrap_or(0);
                    let report = ouch_accepted(idx);
                    let seq = srv.enqueue(&report);
                    let _ = seq;
                    let sp = srv.sequenced_packet(&report);
                    write_packet(&mut stream, &sp)?;
                    total_orders += 1;
                    if total_orders as usize >= n {
                        break; // done with conn1 orders; wait for disconnect
                    }
                }
            }
        }
        Ok((total_orders, vec![replayed]))
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let n: usize = std::env::args()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(50_000);

        let listener = TcpListener::bind(("127.0.0.1", PORT))?;
        let server_handle = std::thread::spawn(move || server_loop(listener, n));

        std::thread::sleep(Duration::from_millis(50));

        // ---- connection 1: full session ----
        let mut client = ClientSession::new(SESSION);
        let mut stream = TcpStream::connect(("127.0.0.1", PORT))?;
        stream.set_nodelay(true)?;

        let t0 = Instant::now();
        write_packet(&mut stream, &client.login_request(USER, PASS))?;
        let ack = read_packet(&mut stream)?;
        assert_eq!(ack.packet_type, packet::LOGIN_ACCEPTED);
        client.handle_packet(&ack, now_ns())?;
        let login_rtt = t0.elapsed();

        let mut rtts = Vec::with_capacity(n);
        let t1 = Instant::now();
        for i in 0..n as u64 {
            let ts = Instant::now();
            write_packet(&mut stream, &Packet::new(packet::UNSEQUENCED_DATA, ouch_enter(i)))?;
            let rep = read_packet(&mut stream)?;
            assert_eq!(rep.packet_type, packet::SEQUENCED_DATA);
            client.handle_packet(&rep, now_ns())?;
            rtts.push(ts.elapsed().as_nanos());
        }
        let t_elapsed = t1.elapsed();
        rtts.sort_unstable();
        let p50 = rtts[n / 2];
        let p99 = rtts[(n as f64 * 0.99) as usize];
        println!("== SoupBinTCP over kernel TCP (N={n}) ==");
        println!("login → accepted RTT: {login_rtt:?} (start_seq={})", client.start_seq);
        println!(
            "order→accept RTT: p50 {:.1} µs  p99 {:.1} µs  mean {:.1} µs",
            p50 as f64 / 1000.0,
            p99 as f64 / 1000.0,
            rtts.iter().sum::<u128>() as f64 / n as f64 / 1000.0
        );
        println!(
            "throughput: {:.0} order/report pairs/s",
            n as f64 / t_elapsed.as_secs_f64()
        );
        let seq_end = client.expected_seq;
        assert_eq!(seq_end, client.start_seq + n as u64, "seq continuity");

        // heartbeat (client 'R') — server silently accepts.
        write_packet(&mut stream, &packet::heartbeat(CLIENT_HEARTBEAT))?;
        drop(stream);

        // ---- connection 2: re-login with requested_sequence -> snapshot replay
        let replay_from = n as u64 / 2;
        let mut client2 = ClientSession::new(SESSION);
        client2.expected_seq = replay_from; // simulate client that missed [1..replay_from)
        let mut stream2 = TcpStream::connect(("127.0.0.1", PORT))?;
        stream2.set_nodelay(true)?;
        write_packet(&mut stream2, &client2.login_request(USER, PASS))?;
        let ack2 = read_packet(&mut stream2)?;
        assert_eq!(ack2.packet_type, packet::LOGIN_ACCEPTED);
        let acc = ack2.parse_login_accepted();
        client2.handle_packet(&ack2, now_ns())?;
        // Server replay queue: packets queued after the Login Accepted.
        // The server replays from `granted` (clamped to its store window);
        // expect exactly next_seq - granted reports (next_seq = n + 1).
        let expect_replay = n as u64 + 1 - acc.sequence;
        let mut replayed_count = 0u64;
        while replayed_count < expect_replay {
            let rep = read_packet(&mut stream2)?;
            assert_eq!(rep.packet_type, packet::SEQUENCED_DATA, "replay packet");
            client2.handle_packet(&rep, now_ns())?;
            replayed_count += 1;
        }
        println!(
            "re-login snapshot: requested={replay_from} granted={} replayed={replayed_count} (store window={}, full={})",
            acc.sequence,
            replayed_count,
            acc.sequence == replay_from,
        );
        assert_eq!(replayed_count, n as u64 + 1 - acc.sequence, "replay to high-water mark");
        drop(stream2); // tell the server this connection is done

        let (orders, extra) = server_handle.join().map_err(|_| "server panicked")??;
        println!("server: {orders} orders, replay-conns extra={extra:?}");
        assert_eq!(orders as usize, n);
        println!("OK: login/RTT/throughput/heartbeat/replay all verified");
        Ok(())
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = real::run() {
            eprintln!("soup_tcp_bench error: {e}");
            std::process::exit(1);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("soup_tcp_bench is Linux-only");
        std::process::exit(1);
    }
}
