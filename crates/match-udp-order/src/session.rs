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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerEvent {
    /// (client_seq, OUCH 消息字节)
    Order(u32, Vec<u8>),
    Cancel(u32, Vec<u8>),
    Replace(u32, Vec<u8>),
    /// NAK_REQUEST(start_seq, count) — 客户端回报缺口
    NakRequest(u32, u32),
    /// HELLO(client_id, last_report_seq)
    Hello(u32, u32),
    Heartbeat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEvent {
    /// (report_seq, payload = [report_type u8][OUCH 回报])
    Report(u32, Vec<u8>),
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
}

impl ServerSession {
    pub fn new(store_cap: usize) -> Self {
        Self {
            next_report_seq: 1,
            expected_client_seq: 1,
            store: VecDeque::new(),
            store_start: 1,
            store_cap: store_cap.max(1),
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
    pub fn on_datagram(&mut self, dg: &[u8]) -> (Vec<ServerEvent>, Vec<Vec<u8>>) {
        let mut events = Vec::new();
        let mut out = Vec::new();
        let Some((h, payload)) = packet::decode(dg) else {
            return (events, out);
        };
        match h.mtype {
            t::ORDER | t::CANCEL | t::REPLACE => {
                let ev = if h.mtype == t::ORDER {
                    ServerEvent::Order(h.seq, payload.to_vec())
                } else if h.mtype == t::CANCEL {
                    ServerEvent::Cancel(h.seq, payload.to_vec())
                } else {
                    ServerEvent::Replace(h.seq, payload.to_vec())
                };
                let ack = if h.seq == self.expected_client_seq {
                    self.expected_client_seq = self.expected_client_seq.wrapping_add(1);
                    events.push(ev);
                    packet::ACK_OK
                } else if h.seq < self.expected_client_seq {
                    // 重复重发（client 超时重传）→ 幂等确认，不重处理
                    packet::ACK_OK
                } else {
                    // 乱序 → 要求重发
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
                    let mut p = Vec::with_capacity(13);
                    p.extend_from_slice(&self.report_high_water().to_be_bytes());
                    p.extend_from_slice(&self.store_start.to_be_bytes());
                    p.push(flags);
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
        let mut p = Vec::with_capacity(8);
        p.extend_from_slice(&start.to_be_bytes());
        p.extend_from_slice(&count.to_be_bytes());
        packet::encode(SESSION_MAGIC, 0, t::NAK_REQUEST, &p)
    }

    /// 重连：报出已收最高回报 seq。
    pub fn send_hello(&self, client_id: u32, last_report_seq: u32) -> Vec<u8> {
        let mut p = Vec::with_capacity(8);
        p.extend_from_slice(&client_id.to_be_bytes());
        p.extend_from_slice(&last_report_seq.to_be_bytes());
        packet::encode(SESSION_MAGIC, 0, t::HELLO, &p)
    }

    /// 处理一个入站 datagram。返回 (事件, 出站 datagrams)。
    pub fn on_datagram(&mut self, dg: &[u8]) -> (Vec<ClientEvent>, Vec<Vec<u8>>) {
        let mut events = Vec::new();
        let mut out = Vec::new();
        let Some((h, payload)) = packet::decode(dg) else {
            return (events, out);
        };
        match h.mtype {
            t::REPORT => {
                if h.seq == self.expected_report_seq {
                    self.expected_report_seq = self.expected_report_seq.wrapping_add(1);
                    events.push(ClientEvent::Report(h.seq, payload.to_vec()));
                } else if h.seq > self.expected_report_seq {
                    // 缺口 → NAK
                    let count = h.seq - self.expected_report_seq;
                    let mut p = Vec::with_capacity(8);
                    p.extend_from_slice(&self.expected_report_seq.to_be_bytes());
                    p.extend_from_slice(&count.to_be_bytes());
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
                    let body = payload[off..off + len].to_vec();
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

    fn drain_server(s: &mut ServerSession, dg: &[u8]) -> (Vec<ServerEvent>, Vec<Vec<u8>>) {
        s.on_datagram(dg)
    }

    #[test]
    fn order_ack_report_roundtrip() {
        let mut srv = ServerSession::new(64);
        let mut cli = ClientSession::new();
        let order = cli.send_order(b"OUCH-ENTER-49B.........");
        let (evs, srv_out) = drain_server(&mut srv, &order);
        assert_eq!(evs, vec![ServerEvent::Order(1, b"OUCH-ENTER-49B.........".to_vec())]);
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
        // 模拟直接发包 seq=2
        let dg = packet::encode(SESSION_MAGIC, 2, t::ORDER, b"x");
        let (evs, out) = drain_server(&mut srv, &dg);
        assert!(evs.is_empty());
        let (h, p) = packet::decode(&out[0]).unwrap();
        assert_eq!(h.mtype, t::ORDER_ACK);
        assert_eq!(p[4], packet::ACK_RETRY);
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
}
