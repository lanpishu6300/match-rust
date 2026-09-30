//! `match-udp-order` — 私有 UDP 下单协议（做市/高频链路）
//!
//! 设计动机：SoupBinTCP/OUCH 走内核 TCP，单条下单实测 RTT 54.9µs，瓶颈在
//! syscall。DPDK 全链路不提供 TCP 栈，故下单改为私有 UDP 可靠会话：
//! 每个方向独立 seq + 显式 ACK/NAK，无 length 前缀（UDP datagram 即消息边界）。
//!
//! 帧格式（9B 头，全 BE）：
//! ```text
//! +----------+----------+------+---------+
//! | session  |  seq     | type | payload |
//! |   4B     |   4B     |  1B  |  ...    |
//! +----------+----------+------+---------+
//! ```
//!
//! 消息类型：
//! - client→server: HELLO(0x30, client_id+last_report_seq) · ORDER(0x01, OUCH Enter)
//!   · CANCEL(0x02, OUCH Cancel) · REPLACE(0x03, OUCH Replace) · NAK_REQUEST(0x20)
//!   · HEARTBEAT(0x40)
//! - server→client: HELLO_ACK(0x31, server_last+store_start+flags) · ORDER_ACK(0x12)
//!   · REPORT(0x10, report_type+OUCH 回报) · NAK_RESPONSE(0x21, 嵌套报告帧) · HEARTBEAT(0x40)
//!
//! 可靠性语义：
//! - 下单侧：client seq 按序，server 乱序即拒；server 对每笔回 ORDER_ACK(client_seq)；
//!   client 保留未 ACK pending 窗口，超时重发（client_seq 幂等去重）。
//! - 回报侧：server report seq 按序 + 重传窗口（默认 8192 条）；client 发现缺口
//!   发 NAK_REQUEST(start,count)，server 从窗口回 NAK_RESPONSE（嵌套帧）。
//! - 重连：client 发 HELLO(last_report_seq)，server 回 HELLO_ACK(server_last, store_start)；
//!   last < store_start → flags=GAP_TOO_OLD，client 必须全量重建（本实现直接重设水位）。

pub mod packet;
pub mod session;

pub use packet::{decode, encode, Header, SESSION_MAGIC};
pub use session::{ClientEvent, ClientSession, ServerEvent, ServerSession};
