//! `mold_outbound_bench` — tx-side benchmark of the match-spot outbound path
//! (Linux only). The real outbound bridge calls MoldPublisher with a 1-byte
//! tag + payload; here we drive the same code path with fill/depth-shaped
//! payloads and compare:
//!   A. publish_tagged  — one message per datagram (default outbound)
//!   B. publish_batch   — packed many-per-datagram (MTU budget 1472)
//!   C. encode-only     — payload+tag construction without any sendto
//!
//! Usage: mold_outbound_bench [N]

#[cfg(target_os = "linux")]
mod real {
    use match_moldudp64::MoldPublisher;
    use std::net::SocketAddr;
    use std::time::Instant;

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let n: usize = std::env::args()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(100_000);

        let target: SocketAddr = "127.0.0.1:55002".parse()?;
        // Simulated fill event: tag(1) + 24B body (order id + qty + price).
        let fill_body = vec![0x11u8; 24];
        let depth_body = vec![0x22u8; 24];

        // ---- A. publish_tagged (one datagram per fill) ----
        let p = MoldPublisher::new("MATCH_RUST", target, 4096)?;
        let mut sink = std::net::UdpSocket::bind("0.0.0.0:55002")?;
        sink.set_nonblocking(true)?;
        // Drain thread so the sender never blocks on a full socket buffer.
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let drain = {
            let done = done.clone();
            std::thread::spawn(move || {
                let mut buf = vec![0u8; 4096];
                let mut n = 0usize;
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    match sink.recv_from(&mut buf) {
                        Ok((_, _)) => n += 1,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::hint::spin_loop();
                        }
                        Err(_) => break,
                    }
                }
                n
            })
        };

        let t0 = Instant::now();
        for _ in 0..n {
            loop {
                match p.publish_tagged(0x01, &fill_body) {
                    Ok(()) => break,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::hint::spin_loop();
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        }
        let a_elapsed = t0.elapsed();

        // ---- B. publish_batch (pack to 1472B budget) ----
        let p2 = MoldPublisher::new("MATCH_RUST", target, 4096)?;
        let batch: Vec<Vec<u8>> = (0..n)
            .map(|_| {
                let mut b = Vec::with_capacity(25);
                b.push(0x02);
                b.extend_from_slice(&depth_body);
                b
            })
            .collect();
        let t1 = Instant::now();
        let mut off = 0usize;
        while off < n {
            let used = p2.publish_batch(&batch[off..], 1472)?;
            if used == 0 {
                break;
            }
            off += used;
        }
        let b_elapsed = t1.elapsed();
        let b_packed = off;

        // ---- C. encode-only (no network) ----
        let t2 = Instant::now();
        let mut encoded = 0usize;
        for i in 0..n {
            let mut b = Vec::with_capacity(25);
            b.push(if i % 2 == 0 { 0x01 } else { 0x02 });
            b.extend_from_slice(&fill_body);
            encoded += b.len();
        }
        let c_elapsed = t2.elapsed();

        done.store(true, std::sync::atomic::Ordering::Relaxed);
        drain.join().ok();

        let secs_a = a_elapsed.as_secs_f64();
        let secs_b = b_elapsed.as_secs_f64();
        let secs_c = c_elapsed.as_secs_f64();
        println!("== outbound tx bench (N={n}, payload 25B incl. tag) ==");
        println!(
            "A. publish_tagged : {:.0} msg/s   ({:.1} ns/msg, {} datagrams)",
            n as f64 / secs_a,
            a_elapsed.as_nanos() as f64 / n as f64,
            n
        );
        println!(
            "B. publish_batch  : {:.0} msg/s   ({:.1} ns/msg, {} msgs packed)",
            b_packed as f64 / secs_b,
            b_elapsed.as_nanos() as f64 / b_packed.max(1) as f64,
            b_packed
        );
        println!(
            "C. encode-only    : {:.0} msg/s   ({:.1} ns/msg)",
            n as f64 / secs_c,
            c_elapsed.as_nanos() as f64 / n as f64
        );
        println!(
            "net+syscall overhead per msg (A - C): {:.1} ns",
            (a_elapsed.as_nanos() as f64 - c_elapsed.as_nanos() as f64) / n as f64
        );
        Ok(())
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = real::run() {
            eprintln!("outbound_bench error: {e}");
            std::process::exit(1);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("mold_outbound_bench is Linux-only");
        std::process::exit(1);
    }
}
