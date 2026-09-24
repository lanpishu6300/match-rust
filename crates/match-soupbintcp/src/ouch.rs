//! OUCH 4.2 — NASDAQ's order-entry business protocol carried by SoupBinTCP.
//!
//! OUCH defines *message formats*, not transport: inbound (client→host)
//! messages ride SoupBinTCP Unsequenced Data packets; outbound (host→client)
//! reports ride Sequenced Data packets. ITCH plays the mirror role for market
//! data (outbound only). See the OUCH 4.2 spec (Updated October 2025) for the
//! authoritative field tables; this module implements the core fixed-length
//! subset used by the integration tests:
//!
//! ```text
//! Enter Order     'O' 49 B   token(14) side(1) shares(4) stock(8)
//!                             price(4) tif(4) firm(4) display(1)
//!                             capacity(1) ise(1) minqty(4) crosstype(1) custtype(1)
//! Cancel Order    'X' 19 B   token(14) shares(4)
//! Replace Order   'U' 49 B   existing(14) replacement(14) shares(4) price(4)
//!                             tif(4) display(1) ise(1) minqty(4) crosstype(1) custtype(1)
//! Modify Order    'M' 21 B   token(14) side(1) shares(4)
//! Accepted        'A' 66 B   timestamp(8) token(14) side(1) shares(4) stock(8)
//!                             price(4) tif(4) firm(4) display(1) ref(8)
//!                             capacity(1) ise(1) minqty(4) crosstype(1) state(1) bbw(1)
//! Executed        'E' 35 B   timestamp(8) token(14) shares(4) match(4) price(4)
//! Canceled        'C' 28 B   timestamp(8) token(14) decrement(4) reason(1)
//! ```
//!
//! Field conventions: integers are unsigned big-endian; alpha fields are
//! left-justified space-padded; price is fixed-point 6.4 decimal.

use std::fmt;

/// Enter Order Message ('O', 49 bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnterOrder {
    pub token: String,          // 14, alphanumeric, day-unique per account
    pub side: u8,               // B/S/T/E
    pub shares: u32,
    pub stock: String,          // 8
    pub price: i32,             // fixed-point 6.4
    pub time_in_force: u32,     // seconds; 0=IOC, 99998=market hours...
    pub firm: String,           // 4
    pub display: u8,            // A/Y/N/P/I/M/W/L/O/T/Q/m/n/B
    pub capacity: u8,           // A/P/R (else O)
    pub intermarket_sweep: u8,  // Y/N/y
    pub min_qty: u32,
    pub cross_type: u8,         // N/O/C/H/S/E/A
    pub customer_type: u8,      // R/N/space
}

/// Cancel Order Message ('X', 19 bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CancelOrder {
    pub token: String, // 14
    pub shares: u32,   // new intended order size; 0 cancels all
}

/// Replace Order Message ('U', 49 bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplaceOrder {
    pub existing_token: String,
    pub replacement_token: String,
    pub shares: u32,
    pub price: i32,
    pub time_in_force: u32,
    pub display: u8,
    pub intermarket_sweep: u8,
    pub min_qty: u32,
    pub cross_type: u8,
    pub customer_type: u8,
}

/// Accepted Message ('A', 66 bytes) — acknowledgement of Enter Order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accepted {
    pub timestamp: u64,  // ns since midnight
    pub token: String,   // 14
    pub side: u8,
    pub shares: u32,
    pub stock: String,   // 8
    pub price: i32,
    pub time_in_force: u32,
    pub firm: String,    // 4
    pub display: u8,
    pub ref_number: u64, // day-unique order reference
    pub order_state: u8, // L=live, D=dead
}

/// Executed Message ('E', 35 bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Executed {
    pub timestamp: u64,
    pub token: String, // 14
    pub shares: u32,
    pub match_number: u32,
    pub price: i32,
}

/// Canceled Message ('C', 28 bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Canceled {
    pub timestamp: u64,
    pub token: String,     // 14
    pub decrement: u32,    // shares removed
    pub reason: u8,        // code
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OuchMessage {
    EnterOrder(EnterOrder),
    CancelOrder(CancelOrder),
    ReplaceOrder(ReplaceOrder),
    Accepted(Accepted),
    Executed(Executed),
    Canceled(Canceled),
}

impl fmt::Display for OuchMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EnterOrder(m) => write!(f, "O token={} side={} sh={} {} px={}", m.token, char::from(m.side), m.shares, m.stock, m.price),
            Self::CancelOrder(m) => write!(f, "X token={} sh={}", m.token, m.shares),
            Self::ReplaceOrder(m) => write!(f, "U old={} new={} sh={} px={}", m.existing_token, m.replacement_token, m.shares, m.price),
            Self::Accepted(m) => write!(f, "A token={} sh={} ref={} state={}", m.token, m.shares, m.ref_number, char::from(m.order_state)),
            Self::Executed(m) => write!(f, "E token={} sh={} match={} px={}", m.token, m.shares, m.match_number, m.price),
            Self::Canceled(m) => write!(f, "C token={} dec={} reason={}", m.token, m.decrement, char::from(m.reason)),
        }
    }
}

const ENTER_LEN: usize = 49;
const CANCEL_LEN: usize = 19;
const REPLACE_LEN: usize = 49; // 1 + 14 + 14 + 4 + 4 + 4 + 1 + 1 + 4 + 1 + 1
const ACCEPTED_LEN: usize = 66;
const EXECUTED_LEN: usize = 35; // 1 + 8 + 14 + 4 + 4 + 4
const CANCELED_LEN: usize = 28; // 1 + 8 + 14 + 4 + 1

// ---- encode ----

fn pad_alpha(buf: &mut Vec<u8>, s: &str, len: usize) {
    let b = s.as_bytes();
    let take = b.len().min(len);
    buf.extend_from_slice(&b[..take]);
    buf.extend(std::iter::repeat(b' ').take(len - take));
}

fn push_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_be_bytes());
}

fn push_i32(buf: &mut Vec<u8>, v: i32) {
    buf.extend_from_slice(&v.to_be_bytes());
}

fn push_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_be_bytes());
}

/// Encode an Enter Order message to its 49-byte OUCH form.
pub fn encode_enter(m: &EnterOrder) -> Vec<u8> {
    let mut b = Vec::with_capacity(ENTER_LEN);
    b.push(b'O');
    pad_alpha(&mut b, &m.token, 14);
    b.push(m.side);
    push_u32(&mut b, m.shares);
    pad_alpha(&mut b, &m.stock, 8);
    push_i32(&mut b, m.price);
    push_u32(&mut b, m.time_in_force);
    pad_alpha(&mut b, &m.firm, 4);
    b.push(m.display);
    b.push(m.capacity);
    b.push(m.intermarket_sweep);
    push_u32(&mut b, m.min_qty);
    b.push(m.cross_type);
    b.push(m.customer_type);
    b
}

/// Encode a Cancel Order message (19 bytes).
pub fn encode_cancel(m: &CancelOrder) -> Vec<u8> {
    let mut b = Vec::with_capacity(CANCEL_LEN);
    b.push(b'X');
    pad_alpha(&mut b, &m.token, 14);
    push_u32(&mut b, m.shares);
    b
}

/// Encode a Replace Order message (49 bytes).
pub fn encode_replace(m: &ReplaceOrder) -> Vec<u8> {
    let mut b = Vec::with_capacity(REPLACE_LEN);
    b.push(b'U');
    pad_alpha(&mut b, &m.existing_token, 14);
    pad_alpha(&mut b, &m.replacement_token, 14);
    push_u32(&mut b, m.shares);
    push_i32(&mut b, m.price);
    push_u32(&mut b, m.time_in_force);
    b.push(m.display);
    b.push(m.intermarket_sweep);
    push_u32(&mut b, m.min_qty);
    b.push(m.cross_type);
    b.push(m.customer_type);
    b
}

/// Encode an Accepted report (66 bytes).
pub fn encode_accepted(m: &Accepted) -> Vec<u8> {
    let mut b = Vec::with_capacity(ACCEPTED_LEN);
    b.push(b'A');
    push_u64(&mut b, m.timestamp);
    pad_alpha(&mut b, &m.token, 14);
    b.push(m.side);
    push_u32(&mut b, m.shares);
    pad_alpha(&mut b, &m.stock, 8);
    push_i32(&mut b, m.price);
    push_u32(&mut b, m.time_in_force);
    pad_alpha(&mut b, &m.firm, 4);
    b.push(m.display);
    push_u64(&mut b, m.ref_number);
    b.push(m.capacity_placeholder());
    b.push(m.ise_placeholder());
    push_u32(&mut b, 0); // min qty
    b.push(m.cross_placeholder());
    b.push(m.order_state);
    b.push(0); // BBO weight
    b
}

/// Encode an Executed report (35 bytes).
pub fn encode_executed(m: &Executed) -> Vec<u8> {
    let mut b = Vec::with_capacity(EXECUTED_LEN);
    b.push(b'E');
    push_u64(&mut b, m.timestamp);
    pad_alpha(&mut b, &m.token, 14);
    push_u32(&mut b, m.shares);
    push_u32(&mut b, m.match_number);
    push_i32(&mut b, m.price);
    b
}

/// Encode a Canceled report (28 bytes).
pub fn encode_canceled(m: &Canceled) -> Vec<u8> {
    let mut b = Vec::with_capacity(CANCELED_LEN);
    b.push(b'C');
    push_u64(&mut b, m.timestamp);
    pad_alpha(&mut b, &m.token, 14);
    push_u32(&mut b, m.decrement);
    b.push(m.reason);
    b
}

// ---- decode ----

fn get_str(b: &[u8], off: usize, len: usize) -> String {
    if off + len > b.len() {
        return String::new();
    }
    String::from_utf8_lossy(&b[off..off + len])
        .trim_end_matches(' ')
        .to_string()
}

fn get_u32(b: &[u8], off: usize) -> u32 {
    if off + 4 > b.len() {
        return 0;
    }
    u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn get_i32(b: &[u8], off: usize) -> i32 {
    if off + 4 > b.len() {
        return 0;
    }
    i32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn get_u64(b: &[u8], off: usize) -> u64 {
    if off + 8 > b.len() {
        return 0;
    }
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[off..off + 8]);
    u64::from_be_bytes(v)
}

/// Parse a single OUCH message by its type byte. Returns `None` for an
/// unknown/truncated type.
pub fn parse(buf: &[u8]) -> Option<OuchMessage> {
    let t = *buf.first()?;
    match t {
        b'O' if buf.len() >= ENTER_LEN => Some(OuchMessage::EnterOrder(EnterOrder {
            token: get_str(buf, 1, 14),
            side: buf[15],
            shares: get_u32(buf, 16),
            stock: get_str(buf, 20, 8),
            price: get_i32(buf, 28),
            time_in_force: get_u32(buf, 32),
            firm: get_str(buf, 36, 4),
            display: buf[40],
            capacity: buf[41],
            intermarket_sweep: buf[42],
            min_qty: get_u32(buf, 43),
            cross_type: buf[47],
            customer_type: buf[48],
        })),
        b'X' if buf.len() >= CANCEL_LEN => Some(OuchMessage::CancelOrder(CancelOrder {
            token: get_str(buf, 1, 14),
            shares: get_u32(buf, 15),
        })),
        b'U' if buf.len() >= REPLACE_LEN => Some(OuchMessage::ReplaceOrder(ReplaceOrder {
            existing_token: get_str(buf, 1, 14),
            replacement_token: get_str(buf, 15, 14),
            shares: get_u32(buf, 29),
            price: get_i32(buf, 33),
            time_in_force: get_u32(buf, 37),
            display: buf[41],
            intermarket_sweep: buf[42],
            min_qty: get_u32(buf, 43),
            cross_type: buf[47],
            customer_type: buf[48],
        })),
        b'A' if buf.len() >= ACCEPTED_LEN => Some(OuchMessage::Accepted(Accepted {
            timestamp: get_u64(buf, 1),
            token: get_str(buf, 9, 14),
            side: buf[23],
            shares: get_u32(buf, 24),
            stock: get_str(buf, 28, 8),
            price: get_i32(buf, 36),
            time_in_force: get_u32(buf, 40),
            firm: get_str(buf, 44, 4),
            display: buf[48],
            ref_number: get_u64(buf, 49),
            order_state: buf[64],
        })),
        b'E' if buf.len() >= EXECUTED_LEN => Some(OuchMessage::Executed(Executed {
            timestamp: get_u64(buf, 1),
            token: get_str(buf, 9, 14),
            shares: get_u32(buf, 23),
            match_number: get_u32(buf, 27),
            price: get_i32(buf, 31),
        })),
        b'C' if buf.len() >= CANCELED_LEN => Some(OuchMessage::Canceled(Canceled {
            timestamp: get_u64(buf, 1),
            token: get_str(buf, 9, 14),
            decrement: get_u32(buf, 23),
            reason: buf[27],
        })),
        _ => None,
    }
}

impl Accepted {
    fn capacity_placeholder(&self) -> u8 {
        b'O'
    }
    fn ise_placeholder(&self) -> u8 {
        b'N'
    }
    fn cross_placeholder(&self) -> u8 {
        b'N'
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_roundtrip() {
        let m = EnterOrder {
            token: "ORD-0001".into(),
            side: b'B',
            shares: 100,
            stock: "AAPL".into(),
            price: 195_5000, // $195.5000
            time_in_force: 99998,
            firm: "ABCD".into(),
            display: b'Y',
            capacity: b'A',
            intermarket_sweep: b'N',
            min_qty: 0,
            cross_type: b'N',
            customer_type: b' ',
        };
        let enc = encode_enter(&m);
        assert_eq!(enc.len(), 49);
        assert_eq!(parse(&enc), Some(OuchMessage::EnterOrder(m)));
    }

    #[test]
    fn cancel_roundtrip() {
        let m = CancelOrder {
            token: "ORD-0001".into(),
            shares: 0,
        };
        let enc = encode_cancel(&m);
        assert_eq!(enc.len(), 19);
        assert_eq!(parse(&enc), Some(OuchMessage::CancelOrder(m)));
    }

    #[test]
    fn accepted_roundtrip() {
        let m = Accepted {
            timestamp: 12_345_678_901u64,
            token: "ORD-0001".into(),
            side: b'B',
            shares: 100,
            stock: "AAPL".into(),
            price: 195_5000,
            time_in_force: 99998,
            firm: "ABCD".into(),
            display: b'Y',
            ref_number: 7_777_777,
            order_state: b'L',
        };
        let enc = encode_accepted(&m);
        assert_eq!(enc.len(), 66);
        assert_eq!(parse(&enc), Some(OuchMessage::Accepted(m)));
    }

    #[test]
    fn replace_roundtrip() {
        let m = ReplaceOrder {
            existing_token: "ORD-0001".into(),
            replacement_token: "ORD-0002".into(),
            shares: 200,
            price: 190_0000,
            time_in_force: 99998,
            display: b'Y',
            intermarket_sweep: b'N',
            min_qty: 5,
            cross_type: b'N',
            customer_type: b'R',
        };
        let enc = encode_replace(&m);
        assert_eq!(enc.len(), 49);
        assert_eq!(parse(&enc), Some(OuchMessage::ReplaceOrder(m)));
    }

    #[test]
    fn executed_roundtrip() {
        let m = Executed {
            timestamp: 12_345_678_902u64,
            token: "ORD-0001".into(),
            shares: 100,
            match_number: 88_888,
            price: 195_5000,
        };
        let enc = encode_executed(&m);
        assert_eq!(enc.len(), 35);
        assert_eq!(parse(&enc), Some(OuchMessage::Executed(m)));
    }

    #[test]
    fn canceled_roundtrip() {
        let m = Canceled {
            timestamp: 12_345_678_903u64,
            token: "ORD-0001".into(),
            decrement: 100,
            reason: b'U', // user requested
        };
        let enc = encode_canceled(&m);
        assert_eq!(enc.len(), 28);
        assert_eq!(parse(&enc), Some(OuchMessage::Canceled(m)));
    }

    #[test]
    fn parse_rejects_unknown_and_truncated() {
        assert_eq!(parse(b""), None);
        assert_eq!(parse(b"Q"), None, "unknown type");
        // Each type needs its full fixed length.
        assert_eq!(parse(&[b'O'; 48]), None, "enter truncated by 1");
        assert_eq!(parse(&[b'X'; 18]), None, "cancel truncated by 1");
        assert_eq!(parse(&[b'U'; 46]), None, "replace truncated by 1");
        assert_eq!(parse(&[b'A'; 65]), None, "accepted truncated by 1");
        assert_eq!(parse(&[b'E'; 33]), None, "executed truncated by 1");
        assert_eq!(parse(&[b'C'; 26]), None, "canceled truncated by 1");
        // Exact minimum length parses (all-zero fields).
        assert!(parse(&[b'X'; 19]).is_some());
        assert!(parse(&[b'C'; 28]).is_some());
        assert_eq!(parse(&[b'C'; 27]), None, "canceled still truncated at 27");
    }

    #[test]
    fn parse_tolerates_extra_trailing_bytes() {
        let mut enc = encode_cancel(&CancelOrder { token: "T".into(), shares: 3 });
        enc.push(b'x'); // trailing garbage beyond the fixed length
        let parsed = parse(&enc);
        assert!(matches!(parsed, Some(OuchMessage::CancelOrder(_))));
    }

    #[test]
    fn alpha_fields_truncate_and_decode_lossy() {
        // Over-long token is truncated to 14 on encode.
        let m = EnterOrder {
            token: "12345678901234567890".into(),
            side: b'S',
            shares: 10,
            stock: "VERYLONGSTOCK".into(), // 12 > 8
            price: 1_0000,
            time_in_force: 0,
            firm: "TOOLONG".into(), // 7 > 4
            display: b'Y',
            capacity: b'P',
            intermarket_sweep: b'Y',
            min_qty: 1,
            cross_type: b'N',
            customer_type: b'R',
        };
        let enc = encode_enter(&m);
        assert_eq!(enc.len(), 49);
        match parse(&enc).unwrap() {
            OuchMessage::EnterOrder(d) => {
                assert_eq!(d.token.len(), 14);
                assert_eq!(d.token, "12345678901234");
                assert_eq!(d.stock, "VERYLONG");
                assert_eq!(d.firm, "TOOL");
            }
            other => panic!("expected EnterOrder, got {other:?}"),
        }

        // Invalid UTF-8 in an alpha field decodes lossily instead of panicking.
        let mut raw = encode_enter(&EnterOrder {
            token: "T".into(),
            side: b'B',
            shares: 1,
            stock: "S".into(),
            price: 0,
            time_in_force: 0,
            firm: "F".into(),
            display: b'Y',
            capacity: b'A',
            intermarket_sweep: b'N',
            min_qty: 0,
            cross_type: b'N',
            customer_type: b' ',
        });
        raw[1] = 0xFF; // corrupt first token byte
        let parsed = parse(&raw).unwrap();
        if let OuchMessage::EnterOrder(d) = parsed {
            assert!(d.token.contains('\u{FFFD}'));
        }
    }

    #[test]
    fn side_and_code_values_roundtrip() {
        for side in [b'B', b'S', b'T', b'E'] {
            let m = EnterOrder {
                token: "T".into(),
                side,
                shares: 1,
                stock: "AAPL".into(),
                price: 0,
                time_in_force: 0,
                firm: "F".into(),
                display: b'Y',
                capacity: b'A',
                intermarket_sweep: b'N',
                min_qty: 0,
                cross_type: b'N',
                customer_type: b' ',
            };
            let enc = encode_enter(&m);
            let OuchMessage::EnterOrder(d) = parse(&enc).unwrap() else {
                panic!("parse failed");
            };
            assert_eq!(d.side, side);
        }
    }

    #[test]
    fn ouch_display_formats() {
        let e = EnterOrder { token: "T1".into(), side: b'B', shares: 10, stock: "AAPL".into(), price: 1_0000, time_in_force: 0, firm: "F".into(), display: b'Y', capacity: b'A', intermarket_sweep: b'N', min_qty: 0, cross_type: b'N', customer_type: b' ' };
        assert_eq!(OuchMessage::EnterOrder(e).to_string(), "O token=T1 side=B sh=10 AAPL px=10000");
        let c = CancelOrder { token: "T1".into(), shares: 0 };
        assert_eq!(OuchMessage::CancelOrder(c).to_string(), "X token=T1 sh=0");
        let r = ReplaceOrder { existing_token: "A".into(), replacement_token: "B".into(), shares: 1, price: 2, time_in_force: 0, display: b'Y', intermarket_sweep: b'N', min_qty: 0, cross_type: b'N', customer_type: b' ' };
        assert_eq!(OuchMessage::ReplaceOrder(r).to_string(), "U old=A new=B sh=1 px=2");
        let a = Accepted { timestamp: 1, token: "T1".into(), side: b'B', shares: 10, stock: "AAPL".into(), price: 1, time_in_force: 0, firm: "F".into(), display: b'Y', ref_number: 5, order_state: b'L' };
        assert_eq!(OuchMessage::Accepted(a).to_string(), "A token=T1 sh=10 ref=5 state=L");
        let ex = Executed { timestamp: 1, token: "T1".into(), shares: 5, match_number: 3, price: 1 };
        assert_eq!(OuchMessage::Executed(ex).to_string(), "E token=T1 sh=5 match=3 px=1");
        let ca = Canceled { timestamp: 1, token: "T1".into(), decrement: 2, reason: b'U' };
        assert_eq!(OuchMessage::Canceled(ca).to_string(), "C token=T1 dec=2 reason=U");
    }

    #[test]
    fn parse_recovers_all_six_types_in_one_buffer() {
        let all = [
            encode_enter(&EnterOrder { token: "T".into(), side: b'B', shares: 1, stock: "S".into(), price: 0, time_in_force: 0, firm: "F".into(), display: b'Y', capacity: b'A', intermarket_sweep: b'N', min_qty: 0, cross_type: b'N', customer_type: b' ' }),
            encode_cancel(&CancelOrder { token: "T".into(), shares: 0 }),
            encode_replace(&ReplaceOrder { existing_token: "A".into(), replacement_token: "B".into(), shares: 1, price: 0, time_in_force: 0, display: b'Y', intermarket_sweep: b'N', min_qty: 0, cross_type: b'N', customer_type: b' ' }),
            encode_accepted(&Accepted { timestamp: 1, token: "T".into(), side: b'B', shares: 1, stock: "S".into(), price: 0, time_in_force: 0, firm: "F".into(), display: b'Y', ref_number: 1, order_state: b'L' }),
            encode_executed(&Executed { timestamp: 1, token: "T".into(), shares: 1, match_number: 1, price: 0 }),
            encode_canceled(&Canceled { timestamp: 1, token: "T".into(), decrement: 1, reason: b'U' }),
        ];
        let kinds: Vec<&str> = all.iter().map(|b| match parse(b) {
            Some(OuchMessage::EnterOrder(_)) => "enter",
            Some(OuchMessage::CancelOrder(_)) => "cancel",
            Some(OuchMessage::ReplaceOrder(_)) => "replace",
            Some(OuchMessage::Accepted(_)) => "accepted",
            Some(OuchMessage::Executed(_)) => "executed",
            Some(OuchMessage::Canceled(_)) => "canceled",
            None => "none",
        }).collect();
        assert_eq!(kinds, ["enter", "cancel", "replace", "accepted", "executed", "canceled"]);
    }
}
