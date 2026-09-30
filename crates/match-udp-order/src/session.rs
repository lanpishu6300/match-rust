//! 双向可靠 UDP 会话。
//!
//! `ServerSession`：撮合/网关侧。维护 report seq（回报编号）+ 重传窗口，
//! 校验 client 订单 seq（乱序即拒），按 HELLO 水位应答重连。
//!
//! `ClientSession`：做市/交易侧。维护 order seq + pending 重发窗口，
//! 监测 report 缺口并发 NAK_REQUEST，处理 NAK_RESPONSE 补帧。

use crate::packet::{self, t, SESSION_MAGIC};
use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

pub const DEFAULT_STORE_CAP: usize = 8192;
pub const REPORT_ACCEPTED: u8 = 0x01;
pub const REPORT_EXECUTED: u8 = 0x02;
pub const REPORT_CANCELED: u8 = 0x03;
pub const REPORT_REJECTED: u8 = 0x04;

/// 服务端事件。`Order/Cancel/Replace` 的载荷**借用入站 datagram**（零拷贝），
/// 只在 `on_datagram` 返回后、调用方处理该事件期间有效；跨调用保留需自行
/// `to_vec`/共享引用（见 `docs/zero-copy-audit.md`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerEvent<'a> {
    /// (client_seq, OUCH 消息字节，借用入站缓冲)
    Order(u32, &'a [u8]),
    Cancel(u32, &'a [u8]),
    Replace(u32, &'a [u8]),
    /// NAK_REQUEST(start_seq, count) — 客户端回报缺口
    NakRequest(u32, u32),
    /// HELLO(client_id, last_report_seq)
    Hello(u32, u32),
    Heartbeat,
}

/// 客户端事件。`Report` 载荷借用入站 datagram（零拷贝），处理期间有效。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEvent<'a> {
    /// (report_seq, payload = [report_type u8][OUCH 回报]，借用入站缓冲)
    Report(u32, &'a [u8]),
    /// (client_seq, result: ACK_OK / ACK_REJECT / ACK_RETRY)
    OrderAck(u32, u8),
    /// (server_last_report_seq, store_start, flags)
    HelloAck(u32, u32, u8),
    Heartbeat,
}

pub struct ServerSession {
    next_report_seq: u32,
    expected_client_seq: u32,
    /// 重传窗口：(report_seq, REPORT payload)
    store: VecDeque<(u32, Vec<u8>)>,
    store_start: u32,
    store_cap: usize,
    /// 已发起 NAK_REQUEST 的订单 gap 起点（同 gap 只 NAK 一次，防风暴）
    last_nak_start: Option<u32>,
}

impl ServerSession {
    pub fn new(store_cap: usize) -> Self {
        Self {
            next_report_seq: 1,
            expected_client_seq: 1,
            store: VecDeque::new(),
            store_start: 1,
            store_cap: store_cap.max(1),
            last_nak_start: None,
        }
    }

    pub fn report_high_water(&self) -> u32 {
        self.next_report_seq.saturating_sub(1)
    }

    pub fn store_start(&self) -> u32 {
        self.store_start
    }

    pub fn store_len(&self) -> usize {
        self.store.len()
    }

    /// 处理一个入站 datagram。返回 (事件, 出站 datagrams)。
    /// 订单/撤单/改单只产生事件；回报由调用方通过 [`ack_and_report`] 生成。
    /// `Order/Cancel/Replace` 事件载荷借用 `dg`（零拷贝），须在本调用返回后、
    /// 下一次 `on_datagram` 之前消费完。
    pub fn on_datagram<'a>(&mut self, dg: &'a [u8]) -> (Vec<ServerEvent<'a>>, Vec<Vec<u8>>) {
        let mut events = Vec::new();
        let mut out = Vec::new();
        let Some((h, payload)) = packet::decode(dg) else {
            return (events, out);
        };
        match h.mtype {
            t::ORDER | t::CANCEL | t::REPLACE => {
                // 零拷贝：事件携带借用切片，不再 to_vec。
                let ev = if h.mtype == t::ORDER {
                    ServerEvent::Order(h.seq, payload)
                } else if h.mtype == t::CANCEL {
                    ServerEvent::Cancel(h.seq, payload)
                } else {
                    ServerEvent::Replace(h.seq, payload)
                };
                let ack = if h.seq == self.expected_client_seq {
                    self.expected_client_seq = self.expected_client_seq.wrapping_add(1);
                    events.push(ev);
                    packet::ACK_OK
                } else if h.seq < self.expected_client_seq {
                    // 重复重发（client 超时重传）→ 幂等确认，不重处理
                    packet::ACK_OK
                } else {
                    // 乱序 → 要求重发 + 主动 NAK_REQUEST（同 gap 只发一次，防风暴）
                    if self.last_nak_start != Some(self.expected_client_seq) {
                        self.last_nak_start = Some(self.expected_client_seq);
                        let count = h.seq - self.expected_client_seq;
                        let mut p = [0u8; 8];
                        p[..4].copy_from_slice(&self.expected_client_seq.to_be_bytes());
                        p[4..].copy_from_slice(&count.to_be_bytes());
                        out.push(packet::encode(SESSION_MAGIC, 0, t::NAK_REQUEST, &p));
                    }
                    packet::ACK_RETRY
                };
                out.push(packet::encode(SESSION_MAGIC, h.seq, t::ORDER_ACK, &[
                    h.seq.to_be_bytes()[0],
                    h.seq.to_be_bytes()[1],
                    h.seq.to_be_bytes()[2],
                    h.seq.to_be_bytes()[3],
                    ack,
                ]));
            }
            t::NAK_REQUEST => {
                if payload.len() >= 8 {
                    let start = packet::u32_be(&payload[0..4]);
                    let count = packet::u32_be(&payload[4..8]);
                    events.push(ServerEvent::NakRequest(start, count));
                    out.push(self.nak_response(start, count));
                }
            }
            t::HELLO => {
                if payload.len() >= 8 {
                    let client_id = packet::u32_be(&payload[0..4]);
                    let last = packet::u32_be(&payload[4..8]);
                    events.push(ServerEvent::Hello(client_id, last));
                    let flags = if last.wrapping_add(1) < self.store_start {
                        packet::HELLO_ACK_GAP_TOO_OLD
                    } else {
                        0
                    };
                    let mut p = [0u8; 13];
                    p[..4].copy_from_slice(&self.report_high_water().to_be_bytes());
                    p[4..8].copy_from_slice(&self.store_start.to_be_bytes());
                    p[8] = flags;
                    out.push(packet::encode(SESSION_MAGIC, 0, t::HELLO_ACK, &p));
                }
            }
            t::HEARTBEAT => {
                events.push(ServerEvent::Heartbeat);
                out.push(packet::encode(SESSION_MAGIC, 0, t::HEARTBEAT, &[]));
            }
            _ => {}
        }
        (events, out)
    }

    /// 订单处理成功：登记回报进重传窗口，回 ORDER_ACK(OK) + REPORT。
    pub fn ack_and_report(&mut self, client_seq: u32, report_payload: &[u8]) -> Vec<Vec<u8>> {
        let seq = self.next_report_seq;
        self.next_report_seq = self.next_report_seq.wrapping_add(1);
        self.store.push_back((seq, report_payload.to_vec()));
        if self.store.len() > self.store_cap {
            let (popped, _) = self.store.pop_front().expect("non-empty");
            self.store_start = popped.wrapping_add(1);
        }
        vec![
            packet::encode(SESSION_MAGIC, client_seq, t::ORDER_ACK, &[
                client_seq.to_be_bytes()[0],
                client_seq.to_be_bytes()[1],
                client_seq.to_be_bytes()[2],
                client_seq.to_be_bytes()[3],
                packet::ACK_OK,
            ]),
            packet::encode(SESSION_MAGIC, seq, t::REPORT, report_payload),
        ]
    }

    /// 订单被拒：不产生 REPORT，只回 ORDER_ACK(REJECT)。
    pub fn reject_order(&self, client_seq: u32) -> Vec<Vec<u8>> {
        vec![packet::encode(SESSION_MAGIC, client_seq, t::ORDER_ACK, &[
            client_seq.to_be_bytes()[0],
            client_seq.to_be_bytes()[1],
            client_seq.to_be_bytes()[2],
            client_seq.to_be_bytes()[3],
            packet::ACK_REJECT,
        ])]
    }

    /// NAK 重传响应：[start][count] + count × [seq][len u16][payload]
    fn nak_response(&self, start: u32, count: u32) -> Vec<u8> {
        let from = start.max(self.store_start);
        let to = (from.saturating_add(count)).min(self.report_high_water() + 1);
        let mut out = Vec::new();
        out.extend_from_slice(&from.to_be_bytes());
        out.extend_from_slice(&(to.saturating_sub(from)).to_be_bytes());
        for (seq, payload) in &self.store {
            if *seq >= from && *seq < to {
                out.extend_from_slice(&seq.to_be_bytes());
                out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
                out.extend_from_slice(payload);
            }
        }
        packet::encode(SESSION_MAGIC, 0, t::NAK_RESPONSE, &out)
    }
}

pub struct ClientSession {
    next_order_seq: u32,
    expected_report_seq: u32,
    /// 未 ACK 订单：seq → payload
    pending: BTreeMap<u32, Vec<u8>>,
    /// 未 ACK 订单发出时间
    pending_ts: BTreeMap<u32, Instant>,
}

impl ClientSession {
    pub fn new() -> Self {
        Self {
            next_order_seq: 1,
            expected_report_seq: 1,
            pending: BTreeMap::new(),
            pending_ts: BTreeMap::new(),
        }
    }

    pub fn expected_report_seq(&self) -> u32 {
        self.expected_report_seq
    }

    /// 发送订单（入 pending，等 ORDER_ACK 或超时重发）。
    pub fn send_order(&mut self, payload: &[u8]) -> Vec<u8> {
        let seq = self.next_order_seq;
        self.next_order_seq = self.next_order_seq.wrapping_add(1);
        self.pending.insert(seq, payload.to_vec());
        self.pending_ts.insert(seq, Instant::now());
        packet::encode(SESSION_MAGIC, seq, t::ORDER, payload)
    }

    /// 超时重发：返回需要重发的 datagrams（幂等，client_seq 去重）。
    pub fn retransmit_due(&mut self, timeout: std::time::Duration) -> Vec<Vec<u8>> {
        let now = Instant::now();
        let mut out = Vec::new();
        let due: Vec<u32> = self
            .pending_ts
            .iter()
            .filter(|(_, ts)| now.duration_since(**ts) >= timeout)
            .map(|(seq, _)| *seq)
            .collect();
        for seq in due {
            if let Some(p) = self.pending.get(&seq) {
                self.pending_ts.insert(seq, now);
                out.push(packet::encode(SESSION_MAGIC, seq, t::ORDER, p));
            }
        }
        out
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// 构造 NAK_REQUEST（回报缺口补帧请求）
    pub fn nak_request(&self, start: u32, count: u32) -> Vec<u8> {
        let mut p = [0u8; 8];
        p[..4].copy_from_slice(&start.to_be_bytes());
        p[4..].copy_from_slice(&count.to_be_bytes());
        packet::encode(SESSION_MAGIC, 0, t::NAK_REQUEST, &p)
    }

    /// 重连：报出已收最高回报 seq。
    pub fn send_hello(&self, client_id: u32, last_report_seq: u32) -> Vec<u8> {
        let mut p = [0u8; 8];
        p[..4].copy_from_slice(&client_id.to_be_bytes());
        p[4..].copy_from_slice(&last_report_seq.to_be_bytes());
        packet::encode(SESSION_MAGIC, 0, t::HELLO, &p)
    }

    /// 处理一个入站 datagram。返回 (事件, 出站 datagrams)。
    /// `Report` 事件载荷借用 `dg`（零拷贝），须在本调用返回后消费完。
    pub fn on_datagram<'a>(&mut self, dg: &'a [u8]) -> (Vec<ClientEvent<'a>>, Vec<Vec<u8>>) {
        let mut events = Vec::new();
        let mut out = Vec::new();
        let Some((h, payload)) = packet::decode(dg) else {
            return (events, out);
        };
        match h.mtype {
            t::REPORT => {
                if h.seq == self.expected_report_seq {
                    self.expected_report_seq = self.expected_report_seq.wrapping_add(1);
                    events.push(ClientEvent::Report(h.seq, payload));
                } else if h.seq > self.expected_report_seq {
                    // 缺口 → NAK
                    let count = h.seq - self.expected_report_seq;
                    let mut p = [0u8; 8];
                    p[..4].copy_from_slice(&self.expected_report_seq.to_be_bytes());
                    p[4..].copy_from_slice(&count.to_be_bytes());
                    out.push(packet::encode(SESSION_MAGIC, 0, t::NAK_REQUEST, &p));
                }
                // seq < expected：重传重复，忽略
            }
            t::NAK_RESPONSE => {
                // [start u32][count u32] + count × [seq u32][len u16][payload]
                if payload.len() < 8 {
                    return (events, out);
                }
                let start = packet::u32_be(&payload[0..4]);
                let count = packet::u32_be(&payload[4..8]);
                let mut off = 8usize;
                for _ in 0..count {
                    if off + 6 > payload.len() {
                        break;
                    }
                    let seq = packet::u32_be(&payload[off..off + 4]);
                    let len = u16::from_be_bytes([payload[off + 4], payload[off + 5]]) as usize;
                    off += 6;
                    if off + len > payload.len() {
                        break;
                    }
                    // 零拷贝：补帧载荷借用入站 datagram。
                    let body = &payload[off..off + len];
                    off += len;
                    if seq == self.expected_report_seq {
                        self.expected_report_seq = self.expected_report_seq.wrapping_add(1);
                        events.push(ClientEvent::Report(seq, body));
                    } else if seq > self.expected_report_seq {
                        events.push(ClientEvent::Report(seq, body));
                        self.expected_report_seq = seq.wrapping_add(1);
                    }
                }
                let _ = (start, count);
            }
            t::ORDER_ACK => {
                if payload.len() >= 5 {
                    let client_seq = packet::u32_be(&payload[0..4]);
                    let result = payload[4];
                    self.pending.remove(&client_seq);
                    self.pending_ts.remove(&client_seq);
                    events.push(ClientEvent::OrderAck(client_seq, result));
                }
            }
            t::HELLO_ACK => {
                if payload.len() >= 9 {
                    let server_last = packet::u32_be(&payload[0..4]);
                    let store_start = packet::u32_be(&payload[4..8]);
                    let flags = payload[8];
                    events.push(ClientEvent::HelloAck(server_last, store_start, flags));
                }
            }
            t::HEARTBEAT => {
                events.push(ClientEvent::Heartbeat);
            }
            _ => {}
        }
        (events, out)
    }
}

impl Default for ClientSession {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::t;

    fn drain_server<'a>(s: &mut ServerSession, dg: &'a [u8]) -> (Vec<ServerEvent<'a>>, Vec<Vec<u8>>) {
        s.on_datagram(dg)
    }

    #[test]
    fn order_ack_report_roundtrip() {
        let mut srv = ServerSession::new(64);
        let mut cli = ClientSession::new();
        let order = cli.send_order(b"OUCH-ENTER-49B.........");
        let (evs, srv_out) = drain_server(&mut srv, &order);
        assert!(matches!(&evs[0], ServerEvent::Order(1, p) if p == b"OUCH-ENTER-49B........."));
        // server 撮合后回报
        let srv_out2 = srv.ack_and_report(1, &[REPORT_ACCEPTED, b'a', b'c', b'c']);
        let mut all_out = srv_out.clone();
        all_out.extend(srv_out2);
        // client 收到 ORDER_ACK + REPORT
        let (cevs, _) = cli.on_datagram(&all_out[0]);
        assert!(matches!(cevs[0], ClientEvent::OrderAck(1, 0)));
        let (cevs2, _) = cli.on_datagram(&all_out[1]);
        assert!(matches!(cevs2[0], ClientEvent::OrderAck(1, 0)));
        let (cevs3, _) = cli.on_datagram(&all_out[2]);
        assert!(matches!(&cevs3[0], ClientEvent::Report(1, p) if p[0] == REPORT_ACCEPTED));
        assert_eq!(cli.expected_report_seq(), 2);
        assert_eq!(cli.pending_len(), 0);
    }

    #[test]
    fn out_of_order_order_retry() {
        let mut srv = ServerSession::new(64);
        // 模拟直接发包 seq=2（期望 1）→ 乱序
        let dg = packet::encode(SESSION_MAGIC, 2, t::ORDER, b"x");
        let (evs, out) = drain_server(&mut srv, &dg);
        assert!(evs.is_empty());
        // out = [NAK_REQUEST(1,1), ORDER_ACK(2, RETRY)]
        assert_eq!(out.len(), 2);
        let (h, p) = packet::decode(&out[0]).unwrap();
        assert_eq!(h.mtype, t::NAK_REQUEST);
        assert_eq!(packet::u32_be(&p[0..4]), 1); // start=expected
        assert_eq!(packet::u32_be(&p[4..8]), 1); // count=gap
        let (h, p) = packet::decode(&out[1]).unwrap();
        assert_eq!(h.mtype, t::ORDER_ACK);
        assert_eq!(p[4], packet::ACK_RETRY);
        // 同一 gap 的后续乱序帧不再重复 NAK（防风暴）
        let dg2 = packet::encode(SESSION_MAGIC, 3, t::ORDER, b"y");
        let (_, out2) = drain_server(&mut srv, &dg2);
        assert_eq!(out2.len(), 1); // 仅 ORDER_ACK，无第二个 NAK
        let (h, _) = packet::decode(&out2[0]).unwrap();
        assert_eq!(h.mtype, t::ORDER_ACK);
    }

    #[test]
    fn duplicate_order_idempotent_ack() {
        let mut srv = ServerSession::new(64);
        let dg1 = packet::encode(SESSION_MAGIC, 1, t::ORDER, b"a");
        let (evs1, out1) = drain_server(&mut srv, &dg1);
        assert_eq!(evs1.len(), 1);
        // 重发同一 seq
        let (evs2, out2) = drain_server(&mut srv, &dg1);
        assert!(evs2.is_empty());
        let (_, p) = packet::decode(&out2[0]).unwrap();
        assert_eq!(p[4], packet::ACK_OK); // 幂等确认
        let _ = out1;
    }

    #[test]
    fn server_routes_cancel_and_replace_events() {
        let mut srv = ServerSession::new(8);
        // CANCEL seq=1 (expected) => Cancel event + ORDER_ACK(OK).
        let c = packet::encode(SESSION_MAGIC, 1, t::CANCEL, b"cancel-payload");
        let (evs, out) = srv.on_datagram(&c);
        assert!(matches!(&evs[0], ServerEvent::Cancel(1, p) if p == b"cancel-payload"));
        let (h, p) = packet::decode(&out[0]).unwrap();
        assert_eq!(h.mtype, t::ORDER_ACK);
        assert_eq!(p[4], packet::ACK_OK);
        // REPLACE seq=2 => Replace event; expected advances to 3.
        let r = packet::encode(SESSION_MAGIC, 2, t::REPLACE, b"replace-payload");
        let (evs2, _) = srv.on_datagram(&r);
        assert!(matches!(&evs2[0], ServerEvent::Replace(2, p) if p == b"replace-payload"));
        // 乱序 REPLACE（期望 3，来 5）=> NAK + ACK_RETRY。
        let r2 = packet::encode(SESSION_MAGIC, 5, t::REPLACE, b"out-of-order");
        let (evs3, out3) = srv.on_datagram(&r2);
        assert!(evs3.is_empty());
        assert_eq!(out3.len(), 2);
        let (h2, p2) = packet::decode(&out3[0]).unwrap();
        assert_eq!(h2.mtype, t::NAK_REQUEST);
        assert_eq!(packet::u32_be(&p2[0..4]), 3);
        assert_eq!(packet::u32_be(&p2[4..8]), 2);
        let (_, p3) = packet::decode(&out3[1]).unwrap();
        assert_eq!(p3[4], packet::ACK_RETRY);
    }

    #[test]
    fn nak_replay_fills_gap() {
        let mut srv = ServerSession::new(64);
        // server 发 3 条回报：ack_and_report 每次返回 [ORDER_ACK, REPORT]
        let mut reports = Vec::new();
        for i in 1..=3u32 {
            reports.extend(srv.ack_and_report(i, &[REPORT_EXECUTED, i as u8]));
        }
        // reports: [0]=ACK1 [1]=REPORT1 [2]=ACK2 [3]=REPORT2 [4]=ACK3 [5]=REPORT3
        // 新 client，只喂 REPORT seq=2 → 缺口在 1
        let mut cli2 = ClientSession::new();
        let (_, out) = cli2.on_datagram(&reports[3]);
        assert!(out.len() == 1);
        let (h, p) = packet::decode(&out[0]).unwrap();
        assert_eq!(h.mtype, t::NAK_REQUEST);
        assert_eq!(packet::u32_be(&p[0..4]), 1);
        assert_eq!(packet::u32_be(&p[4..8]), 1);
        // server 回 NAK_RESPONSE
        let (evs, srv_out) = drain_server(&mut srv, &out[0]);
        assert!(matches!(evs[0], ServerEvent::NakRequest(1, 1)));
        let resp = &srv_out[0];
        let (h2, _) = packet::decode(resp).unwrap();
        assert_eq!(h2.mtype, t::NAK_RESPONSE);
        // client 补帧：expected 1→2
        let (cevs, _) = cli2.on_datagram(resp);
        assert!(matches!(&cevs[0], ClientEvent::Report(1, p) if p[0] == REPORT_EXECUTED && p[1] == 1));
        assert_eq!(cli2.expected_report_seq(), 2);
        // 之后 seq=2 重传到达 → expected 2→3
        let (cevs2, _) = cli2.on_datagram(&reports[3]);
        assert!(matches!(&cevs2[0], ClientEvent::Report(2, p) if p[0] == REPORT_EXECUTED && p[1] == 2));
        assert_eq!(cli2.expected_report_seq(), 3);
    }

    #[test]
    fn hello_reconnect_watermark() {
        let mut srv = ServerSession::new(64);
        for i in 1..=5u32 {
            srv.ack_and_report(i, &[REPORT_ACCEPTED, i as u8]);
        }
        let hello = ClientSession::new().send_hello(42, 3);
        let (evs, out) = drain_server(&mut srv, &hello);
        assert!(matches!(evs[0], ServerEvent::Hello(42, 3)));
        let (h, p) = packet::decode(&out[0]).unwrap();
        assert_eq!(h.mtype, t::HELLO_ACK);
        assert_eq!(packet::u32_be(&p[0..4]), 5); // server_last
        assert_eq!(packet::u32_be(&p[4..8]), 1); // store_start
        assert_eq!(p[8], 0); // 无 gap
    }

    #[test]
    fn hello_gap_too_old() {
        let mut srv = ServerSession::new(4);
        for i in 1..=10u32 {
            srv.ack_and_report(i, &[REPORT_ACCEPTED, i as u8]);
        }
        // store_start 已被挤到 7（cap=4，存 7,8,9,10）
        assert_eq!(srv.store_start(), 7);
        let hello = ClientSession::new().send_hello(1, 1); // 太旧
        let (_, out) = drain_server(&mut srv, &hello);
        let (_, p) = packet::decode(&out[0]).unwrap();
        assert_eq!(p[8], packet::HELLO_ACK_GAP_TOO_OLD);
    }

    #[test]
    fn retransmit_after_timeout() {
        let mut cli = ClientSession::new();
        cli.send_order(b"order-1");
        cli.send_order(b"order-2");
        let due = cli.retransmit_due(std::time::Duration::ZERO);
        assert_eq!(due.len(), 2);
        // 重发后 seq 不变（幂等）
        let (h, _) = packet::decode(&due[0]).unwrap();
        assert_eq!(h.seq, 1);
    }

    #[test]
    fn retransmit_due_no_pending_is_empty() {
        let mut cli = ClientSession::new();
        assert!(cli.retransmit_due(std::time::Duration::ZERO).is_empty());
    }

    #[test]
    fn short_control_payloads_are_ignored() {
        let mut srv = ServerSession::new(8);
        // NAK_REQUEST with < 8-byte payload => no event, no output.
        let dg = packet::encode(SESSION_MAGIC, 0, t::NAK_REQUEST, &[1, 2, 3]);
        let (evs, out) = srv.on_datagram(&dg);
        assert!(evs.is_empty());
        assert!(out.is_empty());
        // HELLO with < 8-byte payload => ignored.
        let dg = packet::encode(SESSION_MAGIC, 0, t::HELLO, &[1]);
        let (evs, out) = srv.on_datagram(&dg);
        assert!(evs.is_empty());
        assert!(out.is_empty());
        // Unknown message type => silent drop.
        let dg = packet::encode(SESSION_MAGIC, 1, 0xFF, b"x");
        let (evs, out) = srv.on_datagram(&dg);
        assert!(evs.is_empty());
        assert!(out.is_empty());
        // Heartbeat => event + echo.
        let dg = packet::encode(SESSION_MAGIC, 0, t::HEARTBEAT, &[]);
        let (evs, out) = srv.on_datagram(&dg);
        assert!(matches!(evs[0], ServerEvent::Heartbeat));
        let (h, _) = packet::decode(&out[0]).unwrap();
        assert_eq!(h.mtype, t::HEARTBEAT);
    }

    #[test]
    fn short_client_frames_are_ignored() {
        let mut cli = ClientSession::new();
        // ORDER_ACK < 5 bytes => ignored.
        let dg = packet::encode(SESSION_MAGIC, 0, t::ORDER_ACK, &[1, 2]);
        let (evs, out) = cli.on_datagram(&dg);
        assert!(evs.is_empty());
        assert!(out.is_empty());
        // HELLO_ACK < 9 bytes => ignored.
        let dg = packet::encode(SESSION_MAGIC, 0, t::HELLO_ACK, &[1, 2, 3]);
        let (evs, out) = cli.on_datagram(&dg);
        assert!(evs.is_empty());
        assert!(out.is_empty());
        // NAK_RESPONSE < 8 bytes => ignored.
        let dg = packet::encode(SESSION_MAGIC, 0, t::NAK_RESPONSE, &[1, 2]);
        let (evs, out) = cli.on_datagram(&dg);
        assert!(evs.is_empty());
        assert!(out.is_empty());
        // Unknown type => silent.
        let dg = packet::encode(SESSION_MAGIC, 1, 0xEE, b"x");
        let (evs, out) = cli.on_datagram(&dg);
        assert!(evs.is_empty());
        assert!(out.is_empty());
    }

    #[test]
    fn client_report_gap_triggers_nak_and_stale_reports_ignored() {
        let mut cli = ClientSession::new();
        // REPORT seq=5 while expecting 1 => NAK_REQUEST(1, 4).
        let dg = packet::encode(SESSION_MAGIC, 5, t::REPORT, &[REPORT_ACCEPTED]);
        let (evs, out) = cli.on_datagram(&dg);
        assert!(evs.is_empty(), "gap not yet filled");
        let (h, p) = packet::decode(&out[0]).unwrap();
        assert_eq!(h.mtype, t::NAK_REQUEST);
        assert_eq!(packet::u32_be(&p[0..4]), 1);
        assert_eq!(packet::u32_be(&p[4..8]), 4);
        // Server fills the gap via NAK_RESPONSE => expected advances to 2.
        let mut rb = Vec::new();
        rb.extend_from_slice(&1u32.to_be_bytes()); // start
        rb.extend_from_slice(&1u32.to_be_bytes()); // count
        rb.extend_from_slice(&1u32.to_be_bytes()); // seq
        rb.extend_from_slice(&1u16.to_be_bytes());
        rb.push(REPORT_ACCEPTED);
        let dg = packet::encode(SESSION_MAGIC, 0, t::NAK_RESPONSE, &rb);
        let (_, _) = cli.on_datagram(&dg);
        assert_eq!(cli.expected_report_seq(), 2);
        // Stale/duplicate report (seq < expected) is ignored, no NAK spam.
        let dg = packet::encode(SESSION_MAGIC, 1, t::REPORT, &[REPORT_ACCEPTED]);
        let (evs, out) = cli.on_datagram(&dg);
        assert!(evs.is_empty());
        assert!(out.is_empty());
    }

    #[test]
    fn nak_response_truncation_stops_gracefully() {
        let mut cli = ClientSession::new();
        // Well-formed header but a block whose len exceeds the buffer.
        let mut body = Vec::new();
        body.extend_from_slice(&1u32.to_be_bytes()); // start
        body.extend_from_slice(&1u32.to_be_bytes()); // count
        body.extend_from_slice(&1u32.to_be_bytes()); // seq
        body.extend_from_slice(&100u16.to_be_bytes()); // len=100 but no payload
        let dg = packet::encode(SESSION_MAGIC, 0, t::NAK_RESPONSE, &body);
        let (evs, out) = cli.on_datagram(&dg);
        assert!(evs.is_empty(), "truncated block dropped");
        assert!(out.is_empty());

        // Block header truncated: only 3 of the 6 header bytes present
        // (off+6 > len) => loop breaks with no event.
        let mut body2 = Vec::new();
        body2.extend_from_slice(&1u32.to_be_bytes()); // start
        body2.extend_from_slice(&1u32.to_be_bytes()); // count
        body2.extend_from_slice(&1u32.to_be_bytes()); // seq — 缺 len 字段 2 字节
        let dg = packet::encode(SESSION_MAGIC, 0, t::NAK_RESPONSE, &body2);
        let (evs, _) = cli.on_datagram(&dg);
        assert!(evs.is_empty());
    }

    #[test]
    fn nak_response_seq_jump_advances_watermark() {
        let mut cli = ClientSession::new();
        // Response claims seq=10 (skipping 1..9) => expected jumps to 11.
        let mut body = Vec::new();
        body.extend_from_slice(&1u32.to_be_bytes()); // start
        body.extend_from_slice(&1u32.to_be_bytes()); // count
        body.extend_from_slice(&10u32.to_be_bytes()); // seq
        body.extend_from_slice(&3u16.to_be_bytes());
        body.extend_from_slice(b"abc");
        let dg = packet::encode(SESSION_MAGIC, 0, t::NAK_RESPONSE, &body);
        let (evs, _) = cli.on_datagram(&dg);
        assert!(matches!(&evs[0], ClientEvent::Report(10, p) if p == b"abc"));
        assert_eq!(cli.expected_report_seq(), 11);
    }

    #[test]
    fn reject_order_only_acks_reject() {
        let mut srv = ServerSession::new(8);
        let out = srv.reject_order(3);
        assert_eq!(out.len(), 1);
        let (h, p) = packet::decode(&out[0]).unwrap();
        assert_eq!(h.mtype, t::ORDER_ACK);
        assert_eq!(p[4], packet::ACK_REJECT);
    }

    #[test]
    fn ack_and_report_evicts_store_when_full() {
        let mut srv = ServerSession::new(2);
        for i in 1..=5u32 {
            srv.ack_and_report(i, &[REPORT_ACCEPTED, i as u8]);
        }
        assert_eq!(srv.store_start(), 4, "only 4,5 retained");
        assert_eq!(srv.store_len(), 2);
        assert_eq!(srv.report_high_water(), 5);
    }

    #[test]
    fn sequence_wraparound_server() {
        // Force expected_client_seq near u32::MAX, then wrap.
        let mut srv = ServerSession {
            next_report_seq: 1,
            expected_client_seq: u32::MAX,
            store: std::collections::VecDeque::new(),
            store_start: 1,
            store_cap: 8,
            last_nak_start: None,
        };
        let dg = packet::encode(SESSION_MAGIC, u32::MAX, t::ORDER, b"x");
        let (evs, out) = srv.on_datagram(&dg);
        assert_eq!(evs.len(), 1, "seq==expected accepted");
        // expected wraps to 0.
        let dg0 = packet::encode(SESSION_MAGIC, 0, t::ORDER, b"y");
        let (evs0, _) = srv.on_datagram(&dg0);
        assert_eq!(evs0.len(), 1, "wrapped seq 0 == expected 0 accepted");
        // A NAK_REQUEST in this state still resolves.
        let _ = out;
    }

    #[test]
    fn sequence_wraparound_client() {
        let mut cli = ClientSession {
            next_order_seq: u32::MAX,
            expected_report_seq: 1,
            pending: std::collections::BTreeMap::new(),
            pending_ts: std::collections::BTreeMap::new(),
        };
        let dg = cli.send_order(b"o1");
        let (h, _) = packet::decode(&dg).unwrap();
        assert_eq!(h.seq, u32::MAX);
        let dg2 = cli.send_order(b"o2");
        let (h2, _) = packet::decode(&dg2).unwrap();
        assert_eq!(h2.seq, 0, "order seq wraps to 0");
    }

    #[test]
    fn hello_flags_and_wire_shapes() {
        let mut srv = ServerSession::new(8);
        for i in 1..=3u32 {
            srv.ack_and_report(i, &[REPORT_ACCEPTED, i as u8]);
        }
        // hello with last_report_seq == high water => flags 0.
        let hello = ClientSession::new().send_hello(7, 3);
        let (evs, out) = srv.on_datagram(&hello);
        assert!(matches!(evs[0], ServerEvent::Hello(7, 3)));
        let (_, p) = packet::decode(&out[0]).unwrap();
        assert_eq!(packet::u32_be(&p[0..4]), 3);
        assert_eq!(packet::u32_be(&p[4..8]), 1);
        assert_eq!(p[8], 0);

        // nak_request wire shape.
        let req = ClientSession::new().nak_request(2, 1);
        let (h, p) = packet::decode(&req).unwrap();
        assert_eq!(h.mtype, t::NAK_REQUEST);
        assert_eq!(packet::u32_be(&p[0..4]), 2);
        assert_eq!(packet::u32_be(&p[4..8]), 1);
    }
}
