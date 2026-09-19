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
