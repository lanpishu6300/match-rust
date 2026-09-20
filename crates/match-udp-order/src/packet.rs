//! 帧编解码：9B 头 [session u32 BE][seq u32 BE][type u8] + payload。
//! UDP datagram 即完整消息，无 length 前缀、无粘包问题。

pub const SESSION_MAGIC: u32 = 0x4D_4F_4F_52; // "MOOR"
pub const HEADER_LEN: usize = 9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub session: u32,
    pub seq: u32,
    pub mtype: u8,
}

#[allow(dead_code)]
pub mod t {
    // client → server
    pub const HELLO: u8 = 0x30;
    pub const ORDER: u8 = 0x01;
    pub const CANCEL: u8 = 0x02;
    pub const REPLACE: u8 = 0x03;
    pub const NAK_REQUEST: u8 = 0x20;
    pub const HEARTBEAT: u8 = 0x40;
    // server → client
    pub const HELLO_ACK: u8 = 0x31;
    pub const ORDER_ACK: u8 = 0x12;
    pub const REPORT: u8 = 0x10;
    pub const NAK_RESPONSE: u8 = 0x21;
}

pub const HELLO_ACK_GAP_TOO_OLD: u8 = 0x01;

pub const ACK_OK: u8 = 0x00;
pub const ACK_REJECT: u8 = 0x01;
pub const ACK_RETRY: u8 = 0x02; // 乱序/重复，客户端应重发

/// 组装一帧。
pub fn encode(session: u32, seq: u32, mtype: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&session.to_be_bytes());
    out.extend_from_slice(&seq.to_be_bytes());
    out.push(mtype);
    out.extend_from_slice(payload);
    out
}

/// 拆一帧。magic 不匹配或长度不足返回 None。
pub fn decode(dg: &[u8]) -> Option<(Header, &[u8])> {
    if dg.len() < HEADER_LEN {
        return None;
    }
    let session = u32::from_be_bytes([dg[0], dg[1], dg[2], dg[3]]);
    if session != SESSION_MAGIC {
        return None;
    }
    let seq = u32::from_be_bytes([dg[4], dg[5], dg[6], dg[7]]);
    let mtype = dg[8];
    Some((Header { session, seq, mtype }, &dg[HEADER_LEN..]))
}

pub fn u32_be(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dg = encode(SESSION_MAGIC, 7, t::ORDER, b"hello-order");
        let (h, p) = decode(&dg).expect("decode");
        assert_eq!(h.seq, 7);
        assert_eq!(h.mtype, t::ORDER);
        assert_eq!(p, b"hello-order");
    }

    #[test]
    fn bad_magic_rejected() {
        let dg = encode(0xDEAD_BEEF, 1, t::ORDER, b"x");
        assert!(decode(&dg).is_none());
    }

    #[test]
    fn short_frame_rejected() {
        assert!(decode(&[0u8; 5]).is_none());
    }
}
