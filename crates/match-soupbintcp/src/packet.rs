//! SoupBinTCP packet codec (wire framing + typed packets).
//!
//! Wire format: `[Length u16 BE][Type u8][Payload]`, `Length = 1 + payload`.
//! All logic is transport-agnostic: feed any byte slice to [`parse_stream`].

use std::fmt;

pub const LOGIN_REQUEST: u8 = b'L';
pub const LOGIN_ACCEPTED: u8 = b'A';
pub const LOGIN_REJECTED: u8 = b'J';
pub const SEQUENCED_DATA: u8 = b'S';
pub const SERVER_HEARTBEAT: u8 = b'H';
pub const END_OF_SESSION: u8 = b'Z';
pub const UNSEQUENCED_DATA: u8 = b'U';
pub const CLIENT_HEARTBEAT: u8 = b'R';
pub const LOGOUT_REQUEST: u8 = b'O';
pub const DEBUG: u8 = b'+';

/// Login reject reason codes (official).
pub const REJECT_NOT_AUTHORIZED: u8 = b'A';
pub const REJECT_SESSION_UNAVAILABLE: u8 = b'S';

/// ASCII field widths (SoupBinTCP 3.x).
pub const USERNAME_LEN: usize = 6;
pub const PASSWORD_LEN: usize = 10;
pub const SESSION_LEN: usize = 10;
pub const SEQUENCE_LEN: usize = 20; // ASCII, right-aligned, space-padded

/// Fixed payload lengths.
pub const LOGIN_REQUEST_PAYLOAD_LEN: usize = USERNAME_LEN + PASSWORD_LEN + SESSION_LEN + SEQUENCE_LEN;
pub const LOGIN_ACCEPTED_PAYLOAD_LEN: usize = SESSION_LEN + SEQUENCE_LEN;
pub const LOGIN_REJECTED_PAYLOAD_LEN: usize = 1;

/// A decoded logical packet (type + payload). Business payloads are owned
/// bytes; login/accept payloads are also parsed into typed views on demand.
#[derive(Clone, PartialEq, Eq)]
pub struct Packet {
    pub packet_type: u8,
    pub payload: Vec<u8>,
}

impl Packet {
    pub fn new(packet_type: u8, payload: Vec<u8>) -> Self {
        Self {
            packet_type,
            payload,
        }
    }

    /// Serialize to the wire form `[Length 2BE][Type][Payload]`.
    pub fn to_bytes(&self) -> Vec<u8> {
        // Saturate instead of overflowing: a payload > 65_534 B cannot be
        // expressed in the u16 length field, so clamp to the maximum
        // representable value rather than panicking in debug or wrapping.
        let len = 1u16.saturating_add(self.payload.len() as u16);
        let mut out = Vec::with_capacity(2 + 1 + self.payload.len());
        out.extend_from_slice(&len.to_be_bytes());
        out.push(self.packet_type);
        out.extend_from_slice(&self.payload);
        out
    }

    pub fn is_login_request(&self) -> bool {
        self.packet_type == LOGIN_REQUEST
    }
    pub fn is_heartbeat(&self) -> bool {
        matches!(self.packet_type, CLIENT_HEARTBEAT | SERVER_HEARTBEAT)
    }

    /// Parse a Login Request payload (ASCII, space-padded).
    pub fn parse_login_request(&self) -> LoginRequest {
        let p = &self.payload;
        LoginRequest {
            username: ascii_field(p, 0, USERNAME_LEN),
            password: ascii_field(p, USERNAME_LEN, PASSWORD_LEN),
            requested_session: ascii_field(p, USERNAME_LEN + PASSWORD_LEN, SESSION_LEN),
            requested_sequence: ascii_field(
                p,
                USERNAME_LEN + PASSWORD_LEN + SESSION_LEN,
                SEQUENCE_LEN,
            )
            .trim()
            .parse::<u64>()
            .unwrap_or(0),
        }
    }

    /// Parse a Login Accepted payload.
    pub fn parse_login_accepted(&self) -> LoginAccepted {
        let p = &self.payload;
        LoginAccepted {
            session: ascii_field(p, 0, SESSION_LEN),
            sequence: ascii_field(p, SESSION_LEN, SEQUENCE_LEN)
                .trim()
                .parse::<u64>()
                .unwrap_or(0),
        }
    }

    /// Parse a Login Rejected payload.
    pub fn parse_login_rejected(&self) -> u8 {
        self.payload.first().copied().unwrap_or(0)
    }
}

/// Build a Login Request packet (client side).
pub fn login_request(username: &str, password: &str, requested_session: &str, seq: u64) -> Packet {
    let mut payload = Vec::with_capacity(LOGIN_REQUEST_PAYLOAD_LEN);
    pad_left(&mut payload, username, USERNAME_LEN);
    pad_left(&mut payload, password, PASSWORD_LEN);
    pad_right(&mut payload, requested_session, SESSION_LEN);
    pad_number_right(&mut payload, seq, SEQUENCE_LEN);
    Packet::new(LOGIN_REQUEST, payload)
}

/// Build a Login Accepted packet (server side).
pub fn login_accepted(session: &str, seq: u64) -> Packet {
    let mut payload = Vec::with_capacity(LOGIN_ACCEPTED_PAYLOAD_LEN);
    pad_right(&mut payload, session, SESSION_LEN);
    pad_number_right(&mut payload, seq, SEQUENCE_LEN);
    Packet::new(LOGIN_ACCEPTED, payload)
}

/// Build a Login Rejected packet.
pub fn login_rejected(reason: u8) -> Packet {
    Packet::new(LOGIN_REJECTED, vec![reason])
}

/// Build a heartbeat (client 'R' or server 'H') — empty payload.
pub fn heartbeat(kind: u8) -> Packet {
    debug_assert!(kind == CLIENT_HEARTBEAT || kind == SERVER_HEARTBEAT);
    Packet::new(kind, Vec::new())
}

/// Build an End of Session packet.
pub fn end_of_session() -> Packet {
    Packet::new(END_OF_SESSION, Vec::new())
}

/// Build a Logout Request packet.
pub fn logout_request() -> Packet {
    Packet::new(LOGOUT_REQUEST, Vec::new())
}

/// Parse a TCP byte stream into complete packets. Returns `(packets, consumed,
/// needs_more)`. A trailing partial packet yields `consumed < buf.len()`.
pub fn parse_stream(buf: &[u8]) -> (Vec<Packet>, usize, bool) {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 3 <= buf.len() {
        let len = u16::from_be_bytes([buf[off], buf[off + 1]]) as usize;
        if len < 1 {
            break; // malformed; stop
        }
        let total = 2 + len;
        if off + total > buf.len() {
            break; // incomplete packet at the tail
        }
        out.push(Packet::new(buf[off + 2], buf[off + 3..off + total].to_vec()));
        off += total;
    }
    let needs_more = off < buf.len();
    (out, off, needs_more)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    pub requested_session: String,
    pub requested_sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginAccepted {
    pub session: String,
    pub sequence: u64,
}

impl fmt::Debug for Packet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Packet({} len={})",
            char::from(self.packet_type),
            self.payload.len()
        )
    }
}

// ---- field helpers (ASCII, space-padded) ----

fn ascii_field(p: &[u8], off: usize, len: usize) -> String {
    if off + len > p.len() {
        return String::new();
    }
    String::from_utf8_lossy(&p[off..off + len])
        .trim()
        .to_string()
}

fn pad_left(out: &mut Vec<u8>, s: &str, len: usize) {
    let b = s.as_bytes();
    let take = b.len().min(len);
    out.extend_from_slice(&b[..take]);
    out.extend(std::iter::repeat(b' ').take(len - take));
}

fn pad_right(out: &mut Vec<u8>, s: &str, len: usize) {
    let b = s.as_bytes();
    let take = b.len().min(len);
    out.extend(std::iter::repeat(b' ').take(len - take));
    out.extend_from_slice(&b[..take]);
}

fn pad_number_right(out: &mut Vec<u8>, n: u64, len: usize) {
    let s = n.to_string();
    pad_right(out, &s, len);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_roundtrip() {
        let p = login_request("trader", "secret01", "SESS1", 42);
        let wire = p.to_bytes();
        let (pkts, consumed, _) = parse_stream(&wire);
        assert_eq!(consumed, wire.len());
        assert_eq!(pkts.len(), 1);
        let lr = pkts[0].parse_login_request();
        assert_eq!(lr.username, "trader");
        assert_eq!(lr.password, "secret01");
        assert_eq!(lr.requested_session, "SESS1");
        assert_eq!(lr.requested_sequence, 42);
    }

    #[test]
    fn split_and_coalesced_stream() {
        // Coalesced: two packets in one buffer.
        let a = login_accepted("SESS1", 7).to_bytes();
        let b = heartbeat(SERVER_HEARTBEAT).to_bytes();
        let mut combined = a.clone();
        combined.extend_from_slice(&b);
        let (pkts, consumed, _) = parse_stream(&combined);
        assert_eq!(consumed, combined.len());
        assert_eq!(pkts.len(), 2);

        // Split: one packet across two buffers.
        let mut first = a.clone();
        let split = first.len() / 2;
        let second = first.split_off(split);
        let (pkts1, c1, need1) = parse_stream(&first);
        assert!(need1);
        assert!(pkts1.is_empty());
        let mut full = first;
        full.extend_from_slice(&second);
        let (pkts2, c2, _) = parse_stream(&full);
        assert_eq!(pkts2.len(), 1 + (c1 > 0) as usize);
        assert_eq!(c2, full.len());
    }

    #[test]
    fn heartbeat_sizes() {
        assert_eq!(heartbeat(CLIENT_HEARTBEAT).to_bytes().len(), 3); // 2+1
        assert_eq!(heartbeat(SERVER_HEARTBEAT).to_bytes().len(), 3);
    }

    #[test]
    fn parse_stream_zero_length_is_malformed_and_stops() {
        // A zero length prefix is malformed: parsing stops, nothing consumed,
        // but the remaining bytes still report needs_more (unconsumed tail).
        let buf = [0u8, 0u8, b'S'];
        let (pkts, consumed, needs_more) = parse_stream(&buf);
        assert!(pkts.is_empty());
        assert_eq!(consumed, 0);
        assert!(needs_more);
    }

    #[test]
    fn parse_stream_length_exceeding_buffer_stops_partial() {
        // Declared length 100 but only 5 bytes follow => tail is partial.
        let mut buf = vec![0u8, 100, b'S'];
        buf.extend_from_slice(b"abc");
        let (pkts, consumed, needs_more) = parse_stream(&buf);
        assert!(pkts.is_empty());
        assert_eq!(consumed, 0);
        assert!(needs_more);
    }

    #[test]
    fn parse_stream_multiple_packets_and_exact_boundary() {
        let a = login_accepted("S", 1).to_bytes(); // 2+1+30 = 33
        let h = heartbeat(SERVER_HEARTBEAT).to_bytes(); // 3
        let mut combined = a.clone();
        combined.extend_from_slice(&h);
        combined.extend_from_slice(&a); // exact boundary at the end
        let (pkts, consumed, needs_more) = parse_stream(&combined);
        assert_eq!(pkts.len(), 3);
        assert_eq!(consumed, combined.len());
        assert!(!needs_more);
    }

    #[test]
    fn packet_type_predicates() {
        assert!(login_request("u", "p", "S", 1).is_login_request());
        assert!(!login_accepted("S", 1).is_login_request());
        assert!(heartbeat(CLIENT_HEARTBEAT).is_heartbeat());
        assert!(heartbeat(SERVER_HEARTBEAT).is_heartbeat());
        assert!(!end_of_session().is_heartbeat());
        assert_eq!(logout_request().packet_type, LOGOUT_REQUEST);
        assert_eq!(end_of_session().packet_type, END_OF_SESSION);
        assert_eq!(login_rejected(REJECT_NOT_AUTHORIZED).parse_login_rejected(), REJECT_NOT_AUTHORIZED);
    }

    #[test]
    fn login_request_field_truncation_and_padding() {
        // Over-long username/password truncated; short session padded right.
        let p = login_request("abcdefghijklmn", "0123456789XX", "AB", 123456);
        let wire = p.to_bytes();
        let (pkts, _, _) = parse_stream(&wire);
        let lr = pkts[0].parse_login_request();
        assert_eq!(lr.username, "abcdef"); // 6-char truncation
        assert_eq!(lr.password, "0123456789"); // 10-char truncation
        assert_eq!(lr.requested_session, "AB"); // right-padded, trimmed
        assert_eq!(lr.requested_sequence, 123456);
    }

    #[test]
    fn login_request_sequence_edge_values() {
        let p = login_request("u", "p", "S", u64::MAX);
        let (pkts, _, _) = parse_stream(&p.to_bytes());
        assert_eq!(pkts[0].parse_login_request().requested_sequence, u64::MAX);

        // Non-numeric sequence text parses as 0 (lenient).
        let mut payload = vec![b' '; 46];
        payload.push(b'X'); // last byte non-digit
        let p = Packet::new(LOGIN_REQUEST, payload);
        assert_eq!(p.parse_login_request().requested_sequence, 0);
    }

    #[test]
    fn short_login_payloads_parse_leniently() {
        // Empty payload: all fields empty, seq 0.
        let p = Packet::new(LOGIN_REQUEST, vec![]);
        let lr = p.parse_login_request();
        assert_eq!(lr.username, "");
        assert_eq!(lr.requested_session, "");
        assert_eq!(lr.requested_sequence, 0);

        // Partial accepted payload: session field needs all 10 bytes, so a
        // 2-byte payload yields an empty session (lenient, no panic).
        let p = Packet::new(LOGIN_ACCEPTED, b"AB".to_vec());
        let acc = p.parse_login_accepted();
        assert_eq!(acc.session, "");
        assert_eq!(acc.sequence, 0);

        // Empty rejected payload => reason 0.
        let p = Packet::new(LOGIN_REJECTED, vec![]);
        assert_eq!(p.parse_login_rejected(), 0);
    }

    #[test]
    fn to_bytes_handles_oversized_payload_with_saturation() {
        // 65_535-byte payload: the u16 length field saturates at 65535
        // instead of panicking or wrapping. The header stays 0xFF 0xFF and
        // the wire stream remains parseable; only the excess payload byte is
        // not addressable by the length field and stays unconsumed.
        let big = vec![b'x'; 65_535];
        let p = Packet::new(SEQUENCED_DATA, big);
        let wire = p.to_bytes();
        assert_eq!(wire.len(), 2 + 1 + 65_535);
        // Length header saturates at 0xFF 0xFF (max representable).
        assert_eq!(&wire[0..2], &[0xFF, 0xFF]);
        let (pkts, consumed, needs_more) = parse_stream(&wire);
        assert_eq!(pkts.len(), 1);
        assert_eq!(pkts[0].payload.len(), 65_534, "payload beyond the u16 window is truncated");
        assert_eq!(consumed, 2 + 65_535);
        assert!(needs_more, "one trailing byte remains unconsumed");
    }

    #[test]
    fn packet_debug_format() {
        assert_eq!(format!("{:?}", heartbeat(CLIENT_HEARTBEAT)), "Packet(R len=0)");
        assert_eq!(format!("{:?}", Packet::new(b'S', vec![1, 2])), "Packet(S len=2)");
    }
}
