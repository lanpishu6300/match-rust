//! MoldUDP64 publisher — the **production (matching-engine) side**.
//!
//! Responsibilities:
//! - assign one global 64-bit sequence number per business message;
//! - pack messages into Downstream packets (single message per packet by
//!   default, batch mode available) and send them via UDP multicast/unicast;
//! - emit periodic heartbeat packets (`msg_count == 0`) carrying the next
//!   sequence, so subscribers can both keep-alive and detect gaps;
//! - keep a bounded ring cache of recent messages, exposed as a shared handle
//!   so the retransmit server can serve NAKs from the same history.
//!
//! Thread-safety: `publish*` take `&self`; the mutable state (next seq + ring
//! cache) lives behind mutexes, which is a non-issue at market-data rates and
//! keeps the integration with `match-spot::Outbound` (which only has `&self`)
//! trivially safe.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};

use crate::ring_buf::{MessageRingBuf, SharedRingBuf};
use crate::types::{
    pack_session, DownstreamHeader, MSG_TAG_DEPTH, MSG_TAG_FILL_ORDER, MOLD_BLOCK_HEADER_LEN,
    MOLD_DOWNSTREAM_HEADER_LEN, MOLD_MAX_DATAGRAM,
};

/// Multicast-aware MoldUDP64 publisher.
pub struct MoldPublisher {
    session_id: [u8; crate::types::MOLD_SESSION_LEN],
    socket: UdpSocket,
    target: SocketAddr,
    next_seq: Mutex<u64>,
    /// Shared history: publisher writes, retransmit server reads.
    cache: SharedRingBuf,
}

impl MoldPublisher {
    /// Bind an outbound socket and point it at `target` (a multicast group or
    /// a unicast address — the protocol is transport-agnostic).
    ///
    /// `cache_capacity` bounds the retransmission history kept for NAKs.
    pub fn new(session: &str, target: SocketAddr, cache_capacity: usize) -> io::Result<Self> {
        let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))?;
        socket.set_nonblocking(true)?;

        if let IpAddr::V4(group) = target.ip() {
            socket.set_multicast_ttl_v4(4)?;
            socket.set_multicast_loop_v4(true)?;
            // Join the group on the default interface so the same host can
            // loop its own multicast back (local integration tests).
            let _ = socket.join_multicast_v4(&group, &Ipv4Addr::UNSPECIFIED);
        }

        Ok(Self {
            session_id: pack_session(session),
            socket,
            target,
            next_seq: Mutex::new(1),
            cache: Arc::new(Mutex::new(MessageRingBuf::new(cache_capacity))),
        })
    }

    /// Publish one raw payload as its own Downstream packet.
    pub fn publish(&self, payload: &[u8]) -> io::Result<()> {
        let seq = self.alloc_seq();
        self.cache.lock().expect("cache lock").push(seq, payload.to_vec());
        self.send_packet(seq, &[payload])
    }

    /// Publish a payload prefixed with a 1-byte message-type tag.
    pub fn publish_tagged(&self, tag: u8, payload: &[u8]) -> io::Result<()> {
        let mut buf = Vec::with_capacity(payload.len() + 1);
        buf.push(tag);
        buf.extend_from_slice(payload);
        self.publish(&buf)
    }

    /// Publish a batch of payloads in a single Downstream packet, honoring
    /// `max_datagram_bytes` (default: 1472 IPv4 multicast MTU budget).
    /// Returns the number of messages actually packed into the datagram.
    pub fn publish_batch(
        &self,
        msgs: &[Vec<u8>],
        max_datagram_bytes: usize,
    ) -> io::Result<usize> {
        if msgs.is_empty() {
            return Ok(0);
        }
        let max = if max_datagram_bytes == 0 {
            MOLD_MAX_DATAGRAM
        } else {
            max_datagram_bytes
        };

        let first_seq = self.next_seq.lock().expect("next_seq lock").clone();
        let mut blocks: Vec<&[u8]> = Vec::new();
        let mut total_len = MOLD_DOWNSTREAM_HEADER_LEN;

        for payload in msgs {
            let block_total = MOLD_BLOCK_HEADER_LEN + payload.len();
            if !blocks.is_empty() && total_len + block_total > max {
                break;
            }
            if blocks.is_empty() && block_total + MOLD_DOWNSTREAM_HEADER_LEN > max {
                break; // single oversized message: skip (MTU policy)
            }
            total_len += block_total;
            blocks.push(payload.as_slice());
        }
        if blocks.is_empty() {
            return Ok(0);
        }

        // Commit sequence numbers + cache only for what actually fits.
        let mut seqs = Vec::with_capacity(blocks.len());
        {
            let mut next = self.next_seq.lock().expect("next_seq lock");
            for _ in &blocks {
                seqs.push(*next);
                *next += 1;
            }
        }
        {
            let mut cache = self.cache.lock().expect("cache lock");
            for (seq, payload) in seqs.iter().zip(msgs.iter()) {
                cache.push(*seq, payload.clone());
            }
        }

        let header = DownstreamHeader {
            session_id: self.session_id,
            seq: first_seq,
            msg_count: blocks.len() as u16,
        };
        let mut hdr_buf = [0u8; MOLD_DOWNSTREAM_HEADER_LEN];
        header.encode(&mut hdr_buf);

        let mut packet = Vec::with_capacity(total_len);
        packet.extend_from_slice(&hdr_buf);
        for payload in &blocks {
            packet.extend_from_slice(&(payload.len() as u16).to_be_bytes());
            packet.extend_from_slice(payload);
        }
        self.socket.send_to(&packet, self.target).map(|_| ())?;
        Ok(blocks.len())
    }

    /// Heartbeat: `msg_count == 0`, seq = next sequence about to be emitted.
    pub fn send_heartbeat(&self) -> io::Result<()> {
        let seq = *self.next_seq.lock().expect("next_seq lock");
        let header = DownstreamHeader {
            session_id: self.session_id,
            seq,
            msg_count: 0,
        };
        let mut buf = [0u8; MOLD_DOWNSTREAM_HEADER_LEN];
        header.encode(&mut buf);
        self.socket.send_to(&buf, self.target).map(|_| ())
    }

    /// Next sequence number to be assigned.
    pub fn next_seq(&self) -> u64 {
        *self.next_seq.lock().expect("next_seq lock")
    }

    /// Sequence of the last published message (0 when nothing published yet).
    pub fn last_seq(&self) -> u64 {
        self.next_seq().saturating_sub(1)
    }

    /// Shared handle to the retransmission history. Hand the same handle to
    /// `MoldRetransmitServer::new` so NAKs are served from this publisher's
    /// cache.
    pub fn shared_cache(&self) -> SharedRingBuf {
        Arc::clone(&self.cache)
    }

    fn alloc_seq(&self) -> u64 {
        let mut next = self.next_seq.lock().expect("next_seq lock");
        let seq = *next;
        *next += 1;
        seq
    }

    fn send_packet(&self, first_seq: u64, msgs: &[&[u8]]) -> io::Result<()> {
        let header = DownstreamHeader {
            session_id: self.session_id,
            seq: first_seq,
            msg_count: msgs.len() as u16,
        };
        let mut hdr_buf = [0u8; MOLD_DOWNSTREAM_HEADER_LEN];
        header.encode(&mut hdr_buf);

        let mut packet = Vec::with_capacity(MOLD_MAX_DATAGRAM);
        packet.extend_from_slice(&hdr_buf);
        for payload in msgs {
            packet.extend_from_slice(&(payload.len() as u16).to_be_bytes());
            packet.extend_from_slice(payload);
        }
        self.socket.send_to(&packet, self.target).map(|_| ())
    }

    /// Local socket address (useful for diagnostics).
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }
}

/// Payload tag helpers for the match-spot integration.
impl MoldPublisher {
    pub const TAG_FILL: u8 = MSG_TAG_FILL_ORDER;
    pub const TAG_DEPTH: u8 = MSG_TAG_DEPTH;
}
