//! MoldUDP64 (Nasdaq 1.00) wire format — big-endian, zero-dependency codec.
//!
//! Layout recap:
//! ```text
//! Downstream packet (20 B header + N message blocks):
//!   +0  session_id : [u8; 10]  ASCII, stable per data stream
//!   +10 sequence   : u64 BE    seq of the FIRST message in this packet
//!   +18 msg_count  : u16 BE    0 => heartbeat (no message blocks follow)
//!   then N blocks:
//!     +0 msg_len   : u16 BE    length of message data (excludes this 2 B)
//!     +2 msg_data  : [u8; msg_len]
//!
//! Request packet (client -> re-request server, 20 B, UDP unicast):
//!   +0  session_id : [u8; 10]
//!   +10 start_seq  : u64 BE    first missing message sequence to replay
//!   +18 msg_count  : u16 BE    number of consecutive messages requested
//! ```
//!
//! All integers are network byte order (big-endian). The publisher assigns one
//! global sequence number per business message; messages inside a packet are
//! implicitly consecutive.

use core::convert::TryFrom;

/// Session identifier length (fixed by the spec).
pub const MOLD_SESSION_LEN: usize = 10;
/// Downstream packet header length: session(10) + seq(8) + count(2).
pub const MOLD_DOWNSTREAM_HEADER_LEN: usize = 20;
/// Request packet header length: session(10) + start_seq(8) + count(2).
pub const MOLD_REQUEST_HEADER_LEN: usize = 20;
/// Message block header length (2-byte length prefix).
pub const MOLD_BLOCK_HEADER_LEN: usize = 2;
/// Typical IPv4 multicast MTU budget for a single MoldUDP64 datagram.
pub const MOLD_MAX_DATAGRAM: usize = 1472;

// ---------------------------------------------------------------------------
// Big-endian primitives
// ---------------------------------------------------------------------------

#[inline]
pub fn put_u16_be(buf: &mut [u8], off: usize, v: u16) {
    buf[off] = (v >> 8) as u8;
    buf[off + 1] = v as u8;
}

#[inline]
pub fn get_u16_be(buf: &[u8], off: usize) -> u16 {
    (u16::from(buf[off]) << 8) | u16::from(buf[off + 1])
}

#[inline]
pub fn put_u64_be(buf: &mut [u8], off: usize, v: u64) {
    buf[off..off + 8].copy_from_slice(&v.to_be_bytes());
}

#[inline]
pub fn get_u64_be(buf: &[u8], off: usize) -> u64 {
    u64::from_be_bytes(<[u8; 8]>::try_from(&buf[off..off + 8]).unwrap())
}

/// Pack a session string into the fixed 10-byte field (left-aligned, NUL-padded,
/// truncated when longer than 10 bytes).
#[inline]
pub fn pack_session(session: &str) -> [u8; MOLD_SESSION_LEN] {
    let mut out = [0u8; MOLD_SESSION_LEN];
    let bytes = session.as_bytes();
    let n = core::cmp::min(bytes.len(), MOLD_SESSION_LEN);
    out[..n].copy_from_slice(&bytes[..n]);
    out
}

// ---------------------------------------------------------------------------
// Downstream header (also used verbatim by the retransmit server replies)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownstreamHeader {
    pub session_id: [u8; MOLD_SESSION_LEN],
    /// Sequence number of the first message in the packet; for heartbeats this
    /// is the next sequence the publisher is about to emit.
    pub seq: u64,
    /// Number of message blocks; 0 means heartbeat.
    pub msg_count: u16,
}

impl DownstreamHeader {
    pub fn encode(&self, out: &mut [u8; MOLD_DOWNSTREAM_HEADER_LEN]) {
        out[..MOLD_SESSION_LEN].copy_from_slice(&self.session_id);
        put_u64_be(out, MOLD_SESSION_LEN, self.seq);
        put_u16_be(out, MOLD_SESSION_LEN + 8, self.msg_count);
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < MOLD_DOWNSTREAM_HEADER_LEN {
            return None;
        }
        let mut session_id = [0u8; MOLD_SESSION_LEN];
        session_id.copy_from_slice(&buf[..MOLD_SESSION_LEN]);
        Some(Self {
            session_id,
            seq: get_u64_be(buf, MOLD_SESSION_LEN),
            msg_count: get_u16_be(buf, MOLD_SESSION_LEN + 8),
        })
    }
}

// ---------------------------------------------------------------------------
// Request header (NAK / retransmission request)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestHeader {
    pub session_id: [u8; MOLD_SESSION_LEN],
    pub start_seq: u64,
    pub msg_count: u16,
}

impl RequestHeader {
    pub fn encode(&self, out: &mut [u8; MOLD_REQUEST_HEADER_LEN]) {
        out[..MOLD_SESSION_LEN].copy_from_slice(&self.session_id);
        put_u64_be(out, MOLD_SESSION_LEN, self.start_seq);
        put_u16_be(out, MOLD_SESSION_LEN + 8, self.msg_count);
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < MOLD_REQUEST_HEADER_LEN {
            return None;
        }
        let mut session_id = [0u8; MOLD_SESSION_LEN];
        session_id.copy_from_slice(&buf[..MOLD_SESSION_LEN]);
        Some(Self {
            session_id,
            start_seq: get_u64_be(buf, MOLD_SESSION_LEN),
            msg_count: get_u16_be(buf, MOLD_SESSION_LEN + 8),
        })
    }
}

// ---------------------------------------------------------------------------
// Payload tagging (exchange-side convention, NOT part of MoldUDP64 spec)
// ---------------------------------------------------------------------------
//
// MoldUDP64 carries opaque bytes; the upper layer decides the encoding. match-rust
// prefixes every payload with a 1-byte message-type tag so a subscriber can route
// without decoding the whole body.

/// Order / fill push (serialized `PushOrder` batch).
pub const MSG_TAG_FILL_ORDER: u8 = 0x01;
/// Handicap depth snapshot (`HandicapDepthData`).
pub const MSG_TAG_DEPTH: u8 = 0x02;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downstream_header_roundtrip() {
        let mut buf = [0u8; MOLD_DOWNSTREAM_HEADER_LEN];
        let hdr = DownstreamHeader {
            session_id: *b"MATCH_RUST",
            seq: 0x0102_0304_0506_0708,
            msg_count: 5,
        };
        hdr.encode(&mut buf);
        let dec = DownstreamHeader::decode(&buf).unwrap();
        assert_eq!(dec, hdr);
        // Spot-check big-endian bytes.
        assert_eq!(&buf[10..18], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&buf[18..20], &[0, 5]);
    }

    #[test]
    fn request_header_roundtrip() {
        let mut buf = [0u8; MOLD_REQUEST_HEADER_LEN];
        let req = RequestHeader {
            session_id: *b"MATCH_RUST",
            start_seq: 42,
            msg_count: 3,
        };
        req.encode(&mut buf);
        assert_eq!(RequestHeader::decode(&buf).unwrap(), req);
    }

    #[test]
    fn session_padding_and_truncation() {
        assert_eq!(&pack_session("ABCDEFGHIJKLMN")[..], b"ABCDEFGHIJ");
        assert_eq!(&pack_session("ab")[..], b"ab\0\0\0\0\0\0\0\0");
        assert_eq!(pack_session(""), [0u8; 10]);
    }

    #[test]
    fn decode_rejects_short_buffers() {
        assert!(DownstreamHeader::decode(&[0u8; 19]).is_none());
        assert!(RequestHeader::decode(&[0u8; 5]).is_none());
    }
}
