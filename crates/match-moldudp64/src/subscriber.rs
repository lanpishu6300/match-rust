//! MoldUDP64 subscriber — the **consumer side**.
//!
//! Receives Downstream packets (multicast or unicast replay), parses the
//! fixed 20-byte header + message blocks, tracks the last seen sequence and
//! detects gaps. On a gap it can emit a NAK `Request` packet to the configured
//! retransmit server (unicast) to fill the hole.
//!
//! `parse_packet` is deliberately public and transport-free: unit tests can
//! feed synthetic packets to exercise gap detection without touching the
//! network stack.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

use crate::types::{
    pack_session, RequestHeader, MOLD_BLOCK_HEADER_LEN, MOLD_DOWNSTREAM_HEADER_LEN,
    MOLD_REQUEST_HEADER_LEN, MOLD_SESSION_LEN,
};

/// Gap information reported when a packet's first sequence is ahead of the
/// expected `last_seq + 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GapInfo {
    /// First sequence we expected to see (i.e. last received + 1).
    pub expected: u64,
    /// Sequence the packet actually started at.
    pub first_seq: u64,
    /// Number of consecutive lost messages.
    pub lost: u64,
}

/// Result of parsing one Downstream packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseOutcome {
    pub first_seq: u64,
    pub msg_count: u16,
    pub is_heartbeat: bool,
    pub gap: Option<GapInfo>,
    /// `(seq, payload)` pairs for every message block (payload includes the
    /// 1-byte match-rust message-type tag when the publisher used tagged mode).
    pub messages: Vec<(u64, Vec<u8>)>,
}

impl ParseOutcome {
    pub fn heartbeat(seq: u64) -> Self {
        Self {
            first_seq: seq,
            msg_count: 0,
            is_heartbeat: true,
            gap: None,
            messages: Vec::new(),
        }
    }
}

/// MoldUDP64 subscriber with sequence tracking and NAK emission.
pub struct MoldSubscriber {
    session_id: [u8; MOLD_SESSION_LEN],
    socket: UdpSocket,
    /// Highest contiguous message sequence received. `None` until the first
    /// data packet (heartbeats before any data only sync the floor).
    last_seq: Option<u64>,
    /// Retransmit server address; `None` disables NAK emission.
    retransmit_addr: Option<SocketAddr>,
}

impl MoldSubscriber {
    /// Bind `bind` and (optionally) join the multicast group at `group`.
    /// Pass `retransmit_addr` to enable NAK requests on gap detection.
    pub fn new(
        session: &str,
        bind: SocketAddr,
        group: Option<Ipv4Addr>,
        retransmit_addr: Option<SocketAddr>,
    ) -> io::Result<Self> {
        let socket = UdpSocket::bind(bind)?;
        socket.set_nonblocking(true)?;
        if let Some(group) = group {
            socket.join_multicast_v4(&group, &Ipv4Addr::UNSPECIFIED)?;
        }
        Ok(Self {
            session_id: pack_session(session),
            socket,
            last_seq: None,
            retransmit_addr,
        })
    }

    /// Convenience for unicast loopback tests (no group join).
    pub fn new_unicast(
        session: &str,
        bind: SocketAddr,
        retransmit_addr: Option<SocketAddr>,
    ) -> io::Result<Self> {
        Self::new(session, bind, None, retransmit_addr)
    }

    /// Receive one datagram and parse it. Returns `Ok(None)` on would-block
    /// or when a short (< 20 B) datagram arrives (ignored, not fatal).
    pub fn recv(&mut self, buf: &mut [u8]) -> io::Result<Option<ParseOutcome>> {
        let (n, _src) = match self.socket.recv_from(buf) {
            Ok(v) => v,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
            Err(e) => return Err(e),
        };
        let data = &buf[..n];
        if data.len() < MOLD_DOWNSTREAM_HEADER_LEN {
            return Ok(None);
        }
        Ok(Some(self.parse_packet(data)))
    }

    /// Parse a Downstream packet in place (transport-free, unit-testable).
    pub fn parse_packet(&mut self, buf: &[u8]) -> ParseOutcome {
        let header = crate::types::DownstreamHeader::decode(buf)
            .expect("short MoldUDP64 datagram rejected at recv boundary");
        debug_assert_eq!(
            header.session_id, self.session_id,
            "session mismatch: datagram belongs to another stream"
        );

        let first_seq = header.seq;
        let msg_count = header.msg_count;

        if msg_count == 0 {
            // Heartbeat: publisher advertises the *next* sequence, so the
            // highest contiguous message seen is seq-1 (when known).
            if let Some(last) = self.last_seq {
                self.last_seq = Some(last.max(first_seq.saturating_sub(1)));
            } else if first_seq > 1 {
                self.last_seq = Some(first_seq - 1);
            }
            return ParseOutcome::heartbeat(first_seq);
        }

        let expected = self.last_seq.map(|l| l + 1).unwrap_or(first_seq);
        let gap = if first_seq > expected {
            Some(GapInfo {
                expected,
                first_seq,
                lost: first_seq - expected,
            })
        } else {
            None
        };

        let mut messages = Vec::with_capacity(msg_count as usize);
        let mut ptr = &buf[MOLD_DOWNSTREAM_HEADER_LEN..];
        let mut seq = first_seq;
        let mut truncated = false;
        for _ in 0..msg_count {
            if ptr.len() < MOLD_BLOCK_HEADER_LEN {
                truncated = true;
                break; // malformed tail: stop, keep what we have
            }
            let len = crate::types::get_u16_be(ptr, 0) as usize;
            ptr = &ptr[MOLD_BLOCK_HEADER_LEN..];
            if ptr.len() < len {
                truncated = true;
                break;
            }
            messages.push((seq, ptr[..len].to_vec()));
            ptr = &ptr[len..];
            seq += 1;
        }

        // Only a fully-parsed packet advances the contiguous watermark; a
        // truncated datagram is treated as invalid for state tracking. The
        // watermark never moves backwards: a replayed/out-of-order packet
        // (first_seq < last) must not lower the highest contiguous sequence.
        if !truncated && !messages.is_empty() {
            let end = first_seq + messages.len() as u64 - 1;
            self.last_seq = Some(match self.last_seq {
                Some(last) => last.max(end),
                None => end,
            });
        }
        ParseOutcome {
            first_seq,
            msg_count,
            is_heartbeat: false,
            gap,
            messages,
        }
    }

    /// Send a NAK request for `count` consecutive messages starting at
    /// `start_seq` (unicast to the configured retransmit server).
    pub fn send_nak(&self, start_seq: u64, count: u16) -> io::Result<()> {
        let Some(addr) = self.retransmit_addr else {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "retransmit server not configured",
            ));
        };
        let req = RequestHeader {
            session_id: self.session_id,
            start_seq,
            msg_count: count,
        };
        let mut buf = [0u8; MOLD_REQUEST_HEADER_LEN];
        req.encode(&mut buf);
        self.socket.send_to(&buf, addr)?;
        Ok(())
    }

    /// Automatically NAK any gap found in the last parsed packet. Returns the
    /// gap that triggered a request, if any.
    pub fn auto_nak(&self, outcome: &ParseOutcome, max_request: u16) -> io::Result<Option<GapInfo>> {
        let Some(gap) = outcome.gap else {
            return Ok(None);
        };
        self.send_nak(gap.expected, gap.lost.min(max_request as u64) as u16)?;
        Ok(Some(gap))
    }

    pub fn last_seq(&self) -> Option<u64> {
        self.last_seq
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Helper for macOS: whether the bound address is a multicast-capable one.
    pub fn is_multicast(addr: SocketAddr) -> bool {
        matches!(addr.ip(), IpAddr::V4(v4) if v4.is_multicast())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DownstreamHeader;

    fn data_packet(session: &[u8; 10], first_seq: u64, msgs: &[&[u8]]) -> Vec<u8> {
        let mut buf = Vec::new();
        let hdr = DownstreamHeader {
            session_id: *session,
            seq: first_seq,
            msg_count: msgs.len() as u16,
        };
        let mut h = [0u8; MOLD_DOWNSTREAM_HEADER_LEN];
        hdr.encode(&mut h);
        buf.extend_from_slice(&h);
        for m in msgs {
            buf.extend_from_slice(&(m.len() as u16).to_be_bytes());
            buf.extend_from_slice(m);
        }
        buf
    }

    const SID: [u8; 10] = *b"MATCH_RUST";

    #[test]
    fn parses_contiguous_packets_without_gap() {
        let mut sub = MoldSubscriber::new_unicast("MATCH_RUST", "127.0.0.1:0".parse().unwrap(), None)
            .unwrap();
        let pkt = data_packet(&SID, 1, &[b"a", b"b", b"c"]);
        let out = sub.parse_packet(&pkt);
        assert!(!out.is_heartbeat);
        assert!(out.gap.is_none());
        assert_eq!(out.messages.len(), 3);
        assert_eq!(out.messages[0].0, 1);
        assert_eq!(out.messages[2].0, 3);
        assert_eq!(sub.last_seq(), Some(3));
    }

    #[test]
    fn detects_gap_and_advances() {
        let mut sub = MoldSubscriber::new_unicast("MATCH_RUST", "127.0.0.1:0".parse().unwrap(), None)
            .unwrap();
        // First packet establishes baseline 1..3.
        sub.parse_packet(&data_packet(&SID, 1, &[b"a", b"b", b"c"]));
        // Next packet starts at 6 => messages 4,5 lost.
        let out = sub.parse_packet(&data_packet(&SID, 6, &[b"f", b"g"]));
        let gap = out.gap.unwrap();
        assert_eq!(gap.expected, 4);
        assert_eq!(gap.first_seq, 6);
        assert_eq!(gap.lost, 2);
        assert_eq!(sub.last_seq(), Some(7));
    }

    #[test]
    fn heartbeat_syncs_and_never_creates_gap() {
        let mut sub = MoldSubscriber::new_unicast("MATCH_RUST", "127.0.0.1:0".parse().unwrap(), None)
            .unwrap();
        let hb = data_packet(&SID, 1, &[]); // msg_count == 0
        let out = sub.parse_packet(&hb);
        assert!(out.is_heartbeat);
        assert!(out.gap.is_none());
        assert_eq!(sub.last_seq(), None); // no data yet => floor unknown

        sub.parse_packet(&data_packet(&SID, 5, &[b"e"])); // data up to 5
        let hb2 = sub.parse_packet(&data_packet(&SID, 10, &[])); // heartbeat advertises 10
        assert!(hb2.is_heartbeat);
        assert!(hb2.gap.is_none());
        assert_eq!(sub.last_seq(), Some(9)); // synced to hb.seq - 1
    }

    #[test]
    fn truncated_tail_is_tolerated() {
        let mut sub = MoldSubscriber::new_unicast("MATCH_RUST", "127.0.0.1:0".parse().unwrap(), None)
            .unwrap();
        let mut pkt = data_packet(&SID, 1, &[b"hello"]);
        pkt.truncate(pkt.len() - 3); // cut the last 3 payload bytes
        let out = sub.parse_packet(&pkt);
        assert_eq!(out.messages.len(), 0);
        assert_eq!(sub.last_seq(), None, "malformed packet must not advance state");
    }

    #[test]
    fn truncated_at_length_field_only() {
        let mut sub = MoldSubscriber::new_unicast("MATCH_RUST", "127.0.0.1:0".parse().unwrap(), None)
            .unwrap();
        // Header + a 2-byte length prefix declaring 5 bytes but no payload.
        let mut pkt = data_packet(&SID, 1, &[b"hello"]);
        pkt.truncate(MOLD_DOWNSTREAM_HEADER_LEN + 2);
        let out = sub.parse_packet(&pkt);
        assert_eq!(out.messages.len(), 0);
        assert_eq!(out.msg_count, 1);
        assert_eq!(sub.last_seq(), None);

        // Header with no block header at all.
        let bare = data_packet(&SID, 1, &[b"x"])[..MOLD_DOWNSTREAM_HEADER_LEN].to_vec();
        let out = sub.parse_packet(&bare);
        assert_eq!(out.messages.len(), 0);
        assert_eq!(sub.last_seq(), None);
    }

    #[test]
    fn partial_block_keeps_earlier_messages() {
        let mut sub = MoldSubscriber::new_unicast("MATCH_RUST", "127.0.0.1:0".parse().unwrap(), None)
            .unwrap();
        // Three messages; truncate inside the 3rd payload.
        let mut pkt = data_packet(&SID, 1, &[b"aaa", b"bbb", b"cccccc"]);
        pkt.truncate(MOLD_DOWNSTREAM_HEADER_LEN + 2 + 3 + 2 + 3 + 2 + 3);
        let out = sub.parse_packet(&pkt);
        assert_eq!(out.messages.len(), 2, "first two blocks kept");
        assert_eq!(out.messages[0].0, 1);
        assert_eq!(out.messages[1].0, 2);
        assert_eq!(sub.last_seq(), None, "truncated packet must not advance watermark");
    }

    #[test]
    fn first_packet_without_baseline_reports_no_gap() {
        let mut sub = MoldSubscriber::new_unicast("MATCH_RUST", "127.0.0.1:0".parse().unwrap(), None)
            .unwrap();
        // With no baseline, expected == first_seq: the very first packet can
        // never be "lost" (nothing to compare against), it just establishes
        // the watermark.
        let out = sub.parse_packet(&data_packet(&SID, 5, &[b"e"]));
        assert!(out.gap.is_none());
        assert_eq!(out.messages[0].0, 5);
        assert_eq!(sub.last_seq(), Some(5));
    }

    #[test]
    fn out_of_order_replay_does_not_rewind_or_gap() {
        let mut sub = MoldSubscriber::new_unicast("MATCH_RUST", "127.0.0.1:0".parse().unwrap(), None)
            .unwrap();
        sub.parse_packet(&data_packet(&SID, 1, &[b"a", b"b", b"c"]));
        // Replay of an old packet (seq 2) must not report a gap or move the watermark.
        let out = sub.parse_packet(&data_packet(&SID, 2, &[b"b"]));
        assert!(out.gap.is_none());
        assert_eq!(out.messages[0].0, 2);
        assert_eq!(sub.last_seq(), Some(3), "watermark unchanged");
        // Duplicate of the whole range likewise.
        let out = sub.parse_packet(&data_packet(&SID, 1, &[b"a", b"b", b"c"]));
        assert!(out.gap.is_none());
        assert_eq!(sub.last_seq(), Some(3));
    }

    #[test]
    fn heartbeat_first_seq_gt_one_sets_floor() {
        let mut sub = MoldSubscriber::new_unicast("MATCH_RUST", "127.0.0.1:0".parse().unwrap(), None)
            .unwrap();
        // First-ever packet is a heartbeat advertising seq 10 => floor = 9.
        let out = sub.parse_packet(&data_packet(&SID, 10, &[]));
        assert!(out.is_heartbeat);
        assert_eq!(sub.last_seq(), Some(9));

        // Heartbeat advertising seq 1 (no data yet) => floor stays unknown.
        let mut sub2 = MoldSubscriber::new_unicast("MATCH_RUST", "127.0.0.1:0".parse().unwrap(), None)
            .unwrap();
        let out = sub2.parse_packet(&data_packet(&SID, 1, &[]));
        assert!(out.is_heartbeat);
        assert_eq!(sub2.last_seq(), None);

        // Heartbeat below the current watermark => floor does not go backwards.
        let out = sub.parse_packet(&data_packet(&SID, 5, &[]));
        assert!(out.is_heartbeat);
        assert_eq!(sub.last_seq(), Some(9));
    }

    #[test]
    fn minimal_gap_and_many_messages() {
        let mut sub = MoldSubscriber::new_unicast("MATCH_RUST", "127.0.0.1:0".parse().unwrap(), None)
            .unwrap();
        sub.parse_packet(&data_packet(&SID, 1, &[b"a"]));
        // Smallest possible gap: expected 2, packet starts at 3.
        let out = sub.parse_packet(&data_packet(&SID, 3, &[b"c"]));
        assert_eq!(out.gap.unwrap().lost, 1);
        assert_eq!(sub.last_seq(), Some(3));

        // Many (255) empty blocks parse without loss.
        let mut many = Vec::new();
        let hdr = DownstreamHeader { session_id: SID, seq: 4, msg_count: 255 };
        let mut h = [0u8; MOLD_DOWNSTREAM_HEADER_LEN];
        hdr.encode(&mut h);
        many.extend_from_slice(&h);
        for _ in 0..255 {
            many.extend_from_slice(&[0, 0]); // empty block
        }
        let out = sub.parse_packet(&many);
        assert_eq!(out.messages.len(), 255);
        assert_eq!(out.messages[0].0, 4);
        assert_eq!(out.messages[254].0, 258);
        assert_eq!(sub.last_seq(), Some(258));
    }

    #[test]
    fn send_nak_without_server_is_error_and_auto_nak_clamps() {
        let sub = MoldSubscriber::new_unicast("MATCH_RUST", "127.0.0.1:0".parse().unwrap(), None)
            .unwrap();
        assert!(sub.send_nak(1, 3).is_err(), "no retransmit server configured");
        assert!(sub.auto_nak(&ParseOutcome::heartbeat(1), 10).unwrap().is_none());

        // auto_nak with a gap on a configured subscriber emits exactly max_request.
        let mut sub2 = MoldSubscriber::new_unicast(
            "MATCH_RUST",
            "127.0.0.1:0".parse().unwrap(),
            Some("127.0.0.1:9".parse().unwrap()),
        )
        .unwrap();
        sub2.parse_packet(&data_packet(&SID, 1, &[b"a"])); // baseline, last=1
        let out = sub2.parse_packet(&data_packet(&SID, 20, &[b"t"]));
        let gap = sub2.auto_nak(&out, 5).unwrap().unwrap();
        assert_eq!(gap.lost, 18, "expected=2, first=20 => lost=18");
    }

    #[test]
    fn multicast_predicate() {
        assert!(MoldSubscriber::is_multicast("239.0.0.1:1".parse().unwrap()));
        assert!(!MoldSubscriber::is_multicast("127.0.0.1:1".parse().unwrap()));
    }

    #[test]
    fn recv_returns_none_on_would_block() {
        let mut sub = MoldSubscriber::new_unicast("MATCH_RUST", "127.0.0.1:0".parse().unwrap(), None)
            .unwrap();
        let mut buf = [0u8; 64];
        let r = sub.recv(&mut buf).unwrap();
        assert!(r.is_none(), "empty non-blocking socket yields None");
    }
}
