//! MoldUDP64 re-request (retransmit) server — the NAK filler.
//!
//! A subscriber that detects a gap sends a 20-byte `Request` packet (unicast)
//! to this server: `session_id + start_seq + msg_count`. The server replies
//! with a standard Downstream packet carrying the available messages from the
//! shared publisher cache, sent as unicast to the requester.
//!
//! The cache is shared with the publisher (`MoldPublisher::shared_cache`), so
//! the server can replay recent history without any network round trip.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

use crate::ring_buf::SharedRingBuf;
use crate::types::{
    pack_session, DownstreamHeader, RequestHeader, MOLD_DOWNSTREAM_HEADER_LEN,
    MOLD_REQUEST_HEADER_LEN, MOLD_SESSION_LEN,
};

/// Blocking (per call) UDP server: `serve_once` handles exactly one request.
/// Production code typically runs it on a dedicated thread calling
/// `serve_loop` with a bounded timeout.
pub struct MoldRetransmitServer {
    session_id: [u8; MOLD_SESSION_LEN],
    socket: UdpSocket,
    cache: SharedRingBuf,
}

impl MoldRetransmitServer {
    pub fn new(session: &str, bind: SocketAddr, cache: SharedRingBuf) -> io::Result<Self> {
        let socket = UdpSocket::bind(bind)?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            session_id: pack_session(session),
            socket,
            cache,
        })
    }

    /// Receive and serve one NAK request. Returns the requester address on
    /// success, `Ok(None)` on would-block. Ignores malformed or
    /// foreign-session requests.
    pub fn serve_once(&self, buf: &mut [u8]) -> io::Result<Option<SocketAddr>> {
        let (n, from) = match self.socket.recv_from(buf) {
            Ok(v) => v,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
            Err(e) => return Err(e),
        };
        let Some(req) = RequestHeader::decode(&buf[..n]) else {
            return Ok(Some(from)); // short request: nothing to do
        };
        if req.session_id != self.session_id {
            return Ok(Some(from)); // foreign session: drop
        }
        self.reply_range(req.start_seq, req.msg_count, from)?;
        Ok(Some(from))
    }

    /// Build one Downstream packet with `count` (or fewer, if the cache is
    /// missing some) messages starting at `start_seq` and send it as unicast
    /// to `to`. Returns the number of messages replayed.
    pub fn reply_range(&self, start_seq: u64, count: u16, to: SocketAddr) -> io::Result<usize> {
        let msgs = {
            let cache = self.cache.lock().expect("cache lock");
            cache.get_range(start_seq, count as u64)
        };
        if msgs.is_empty() {
            return Ok(0);
        }

        let header = DownstreamHeader {
            session_id: self.session_id,
            seq: start_seq,
            msg_count: msgs.len() as u16,
        };
        let mut hdr_buf = [0u8; MOLD_DOWNSTREAM_HEADER_LEN];
        header.encode(&mut hdr_buf);

        let mut packet = Vec::with_capacity(MOLD_DOWNSTREAM_HEADER_LEN + 512);
        packet.extend_from_slice(&hdr_buf);
        for m in &msgs {
            packet.extend_from_slice(&(m.payload.len() as u16).to_be_bytes());
            packet.extend_from_slice(&m.payload);
        }
        self.socket.send_to(&packet, to)?;
        Ok(msgs.len())
    }

    /// Loop: serve requests until `stop` returns true (e.g. an atomic flag or
    /// a duration-based deadline). Blocks on the socket with `timeout_ms`
    /// granularity.
    pub fn serve_loop<F>(&self, timeout_ms: u64, mut stop: F) -> io::Result<()>
    where
        F: FnMut() -> bool,
    {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        let mut buf = [0u8; MOLD_REQUEST_HEADER_LEN + 64];
        loop {
            if stop() || std::time::Instant::now() >= deadline {
                return Ok(());
            }
            match self.serve_once(&mut buf) {
                Ok(Some(_)) => {}
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(1)),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(1))
                }
                Err(e) => return Err(e),
            }
        }
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Multicast group for a (rare) multicast-based re-request topology;
    /// NAKs are unicast by spec, so this is informational.
    pub fn _group_hint(group: Ipv4Addr) -> Ipv4Addr {
        group
    }
}

impl std::fmt::Debug for MoldRetransmitServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MoldRetransmitServer")
            .field("session_id", &String::from_utf8_lossy(&self.session_id))
            .field("local_addr", &self.socket.local_addr())
            .finish_non_exhaustive()
    }
}

// Keep the IpAddr import used in doc examples / future unicast checks.
#[allow(dead_code)]
fn _is_unicast(addr: SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(v4) => !v4.is_multicast() && !v4.is_broadcast(),
        IpAddr::V6(v6) => !v6.is_multicast(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring_buf::new_shared;
    use crate::types::RequestHeader;

    fn server(cache_cap: usize) -> MoldRetransmitServer {
        MoldRetransmitServer::new("TEST", "127.0.0.1:0".parse().unwrap(), new_shared(cache_cap)).unwrap()
    }

    const SID10: [u8; 10] = [b'T', b'E', b'S', b'T', 0, 0, 0, 0, 0, 0];

    fn req_bytes(session: [u8; 10], start: u64, count: u16) -> [u8; 20] {
        let r = RequestHeader { session_id: session, start_seq: start, msg_count: count };
        let mut b = [0u8; 20];
        r.encode(&mut b);
        b
    }

    fn fill_cache(cache: &SharedRingBuf, seqs: std::ops::RangeInclusive<u64>) {
        let mut g = cache.lock().unwrap();
        for s in seqs {
            g.push(s, vec![s as u8]);
        }
    }

    #[test]
    fn reply_range_empty_cache_returns_zero() {
        let s = server(8);
        let to: SocketAddr = "127.0.0.1:9".parse().unwrap();
        assert_eq!(s.reply_range(1, 5, to).unwrap(), 0);
        // count == 0 never replays anything.
        fill_cache(&s.cache, 1..=3);
        assert_eq!(s.reply_range(1, 0, to).unwrap(), 0);
    }

    #[test]
    fn reply_range_partial_and_over_window() {
        let s = server(8);
        fill_cache(&s.cache, 1..=5);
        let to: SocketAddr = "127.0.0.1:9".parse().unwrap();
        assert_eq!(s.reply_range(3, 1, to).unwrap(), 1);
        // Beyond the cache => 0.
        assert_eq!(s.reply_range(100, 2, to).unwrap(), 0);
        // Over the retained window edge: 5..7 => only 5 available.
        assert_eq!(s.reply_range(5, 3, to).unwrap(), 1);
    }

    #[test]
    fn serve_once_rejects_short_and_foreign_session() {
        let s = server(4);
        fill_cache(&s.cache, 1..=2);
        let srv = s.local_addr().unwrap();
        let from: SocketAddr = "127.0.0.1:9".parse().unwrap();
        // Send a well-formed request to the server socket.
        let _ = s.socket.send_to(&req_bytes(SID10, 1, 1), srv).unwrap();

        let mut srv_buf = [0u8; 1500];
        // Poll until the request arrives.
        for _ in 0..50 {
            if let Some(_f) = s.serve_once(&mut srv_buf).unwrap() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        // Short request (< 20 B): handled, no reply.
        let short = [0u8; 19];
        let _ = s.socket.send_to(&short, srv).unwrap();
        let mut got = None;
        for _ in 0..50 {
            if let Some(f) = s.serve_once(&mut srv_buf).unwrap() {
                got = Some(f);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(got.is_some());

        // Foreign session: dropped without reply.
        let _ = s.socket.send_to(&req_bytes([b'O', b'T', b'H', b'E', b'R', 0, 0, 0, 0, 0], 1, 1), srv).unwrap();
        let mut got2 = None;
        for _ in 0..50 {
            if let Some(f) = s.serve_once(&mut srv_buf).unwrap() {
                got2 = Some(f);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(got2.is_some());
    }

    #[test]
    fn serve_loop_stops_immediately_and_respects_deadline() {
        let s = server(4);
        // stop() true on first iteration => immediate return.
        assert!(s.serve_loop(10_000, || true).is_ok());
        // Zero deadline => returns immediately.
        assert!(s.serve_loop(0, || false).is_ok());
    }

    #[test]
    fn serve_loop_processes_a_request() {
        let s = server(8);
        fill_cache(&s.cache, 1..=2);
        let srv = s.local_addr().unwrap();
        let _ = s.socket.send_to(&req_bytes(SID10, 1, 1), srv).unwrap();

        // Run the loop on this thread with a stop flag flipped after ~30ms.
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop2 = std::sync::Arc::clone(&stop);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            stop2.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        s.serve_loop(2_000, || stop.load(std::sync::atomic::Ordering::SeqCst)).unwrap();
    }

    #[test]
    fn debug_format_and_helpers() {
        let s = server(2);
        let fmt = format!("{s:?}");
        assert!(fmt.contains("MoldRetransmitServer"));
        assert!(fmt.contains("TEST"));
        assert!(_is_unicast("10.0.0.1:1".parse().unwrap()));
        assert!(!_is_unicast("239.1.1.1:1".parse().unwrap()));
        assert!(!_is_unicast("255.255.255.255:1".parse().unwrap()));
        assert!(_is_unicast("[::1]:1".parse().unwrap()));
        assert!(!_is_unicast("[ff02::1]:1".parse().unwrap()));
        assert_eq!(MoldRetransmitServer::_group_hint(Ipv4Addr::LOCALHOST), Ipv4Addr::LOCALHOST);
    }
}

