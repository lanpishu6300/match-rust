//! `mold_udp_bench` — kernel-IP-stack baseline for MoldUDP64 (Linux only).
//!
//! Two measurements, both in-container on the loopback:
//!   1. throughput: N Mold messages via MoldPublisher -> MoldSubscriber
//!      (std::net::UdpSocket), measures end-to-end msg/s incl. parse.
//!   2. rtt: raw UDP loopback ping-pong with ns timestamps -> P50/P99/mean
//!      one-way latency (network stack only, no Mold parse).
//!
//! Usage: mold_udp_bench [N] [RTT_ITERS]

#[cfg(target_os = "linux")]
mod real {
    use match_moldudp64::MoldPublisher;
    use match_moldudp64::MoldSubscriber;
    use std::net::{SocketAddr, UdpSocket};
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    const PORT: u16 = 55001;

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let n: usize = std::env::args()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(100_000);
        let rtt_iters: usize = std::env::args()
            .nth(2)
            .and_then(|s| s.parse().ok())
            .unwrap_or(2_000);

        // ---- 1. throughput (publisher thread -> subscriber thread) ----
        let target: SocketAddr = format!("127.0.0.1:{PORT}").parse()?;
        let payload = vec![0x42u8; 24];

        let pub_handle = {
            let target = target;
            let payload = payload.clone();
            thread::spawn(move || -> std::io::Result<u64> {
                let p = MoldPublisher::new("MATCH_RUST", target, 4096)?;
                for _ in 0..n {
                    p.publish_tagged(0x01, &payload)?;
                }
                Ok(p.next_seq() - 1)
            })
        };

        let mut sub = MoldSubscriber::new_unicast(
            "MATCH_RUST",
            format!("0.0.0.0:{PORT}").parse::<SocketAddr>()?,
            None,
        )?;

        let t0 = Instant::now();
        let mut seen = 0usize;
        let mut buf = vec![0u8; 2048];
        while seen < n {
            match sub.recv(&mut buf)? {
                Some(out) => {
                    if !out.is_heartbeat {
                        seen += out.messages.len();
                    }
                }
                None => {
                    // Non-blocking socket: busy-poll (DPDK-style), so the
                    // receiver keeps up with the sender's burst.
                    std::hint::spin_loop();
                }
            }
        }
        let elapsed = t0.elapsed();
        let last_seq = sub.last_seq();
        drop(sub);
        let sent = pub_handle.join().map_err(|_| "pub thread panicked")??;

        let secs = elapsed.as_secs_f64();
        let msg_s = seen as f64 / secs;
        let bytes = seen as u64 * (payload.len() as u64 + 24); // mold overhead ~24B
        println!("== kernel UDP throughput (loopback) ==");
        println!("messages: {seen} (publisher sent {sent}, last_seq={last_seq:?})");
        println!("elapsed: {:.3} s", secs);
        println!("throughput: {:.0} msg/s", msg_s);
        println!("throughput: {:.2} MB/s", bytes as f64 / secs / 1e6);
        println!("per-message CPU+net: {:.1} ns", elapsed.as_nanos() as f64 / seen as f64);

        // ---- 2. raw UDP RTT ping-pong (loopback) ----
        let a = UdpSocket::bind("127.0.0.1:0")?;
        let b = UdpSocket::bind("127.0.0.1:0")?;
        let a_addr = a.local_addr()?;
        let b_addr = b.local_addr()?;
        a.set_read_timeout(Some(Duration::from_secs(5)))?;
        b.set_read_timeout(Some(Duration::from_secs(5)))?;

        // warm-up
        let mut ts = [0u8; 16];
        a.send_to(&ts, b_addr)?;
        let _ = b.recv_from(&mut ts)?;
        b.send_to(&ts, a_addr)?;
        let _ = a.recv_from(&mut ts)?;

        let mut rtts: Vec<u128> = Vec::with_capacity(rtt_iters);
        let mut cnt = 0;
        while cnt < rtt_iters {
            let t_send = SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_nanos() as u64;
            ts[..8].copy_from_slice(&t_send.to_le_bytes());
            a.send_to(&ts, b_addr)?;
            let _ = b.recv_from(&mut ts)?;
            b.send_to(&ts, a_addr)?;
            let (_, _) = a.recv_from(&mut ts)?;
            let t_recv = SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_nanos() as u64;
            let rtt = t_recv - u64::from_le_bytes(ts[..8].try_into()?);
            rtts.push(rtt as u128);
            cnt += 1;
        }
        rtts.sort_unstable();
        let p50 = rtts[rtts.len() / 2];
        let p99 = rtts[(rtts.len() as f64 * 0.99) as usize];
        let mean = rtts.iter().sum::<u128>() / rtts.len() as u128;
        println!();
        println!("== kernel UDP loopback RTT (raw socket ping-pong) ==");
        println!("iterations: {rtt_iters}");
        println!("RTT p50: {p50} ns  p99: {p99} ns  mean: {mean} ns");
        println!("one-way (RTT/2): p50 {:.0} ns  p99 {:.0} ns  mean {:.0} ns",
            p50 as f64 / 2.0, p99 as f64 / 2.0, mean as f64 / 2.0);
        Ok(())
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = real::run() {
            eprintln!("udp_bench error: {e}");
            std::process::exit(1);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("mold_udp_bench is Linux-only");
        std::process::exit(1);
    }
}
