//! SoupBinTCP — NASDAQ's TCP session layer, the "TCP sibling" of MoldUDP64.
//!
//! Where MoldUDP64 sequences UDP multicast downstream packets, SoupBinTCP
//! sequences the same style of downstream messages over a single TCP
//! connection and adds a login/session layer. It carries business protocols:
//! - **OUCH** (order entry: new/replace/cancel + executions) — inbound
//!   messages are Unsequenced Data, outbound reports are Sequenced Data;
//! - **ITCH** (market data) — outbound only, same Sequenced Data envelope.
//!
//! # Wire format (binary, SoupBinTCP 3.x/4.x — confirmed against two
//! independent implementations: jvirtanen/node-soupbintcp 3.00 and
//! markwinter/go-finproto 4.1, plus the SoupTCP 2.00 official PDF)
//!
//! Every logical packet on the wire is:
//!
//! ```text
//! +--------+------+---------+
//! | Length | Type | Payload |
//! |  2B BE | 1 B  |  ...    |
//! +--------+------+---------+
//! ```
//! `Length = 1 + len(payload)`. TCP streams may split/coalesce packets
//! arbitrarily; the length prefix makes framing unambiguous.
//!
//! | Type | Name             | Payload                                  |
//! |------|------------------|------------------------------------------|
//! | 'L'  | Login Request    | user(6) pass(10) session(10) seq(20 ascii) |
//! | 'A'  | Login Accepted   | session(10) seq(20 ascii)                 |
//! | 'J'  | Login Rejected   | reason(1)                                 |
//! | 'S'  | Sequenced Data   | business message(s)                       |
//! | 'H'  | Server Heartbeat | (empty)                                   |
//! | 'Z'  | End of Session   | (optional trailing messages)              |
//! | 'U'  | Unsequenced Data | business message(s)                       |
//! | 'R'  | Client Heartbeat | (empty)                                   |
//! | 'O'  | Logout Request   | (empty)                                   |
//! | '+'  | Debug            | text                                      |
//!
//! # Sequencing & "session snapshot" semantics
//!
//! - The first downstream message of a session is sequence 1; each
//!   Sequenced Data packet increments it.
//! - `Login Accepted` tells the client which sequence the server will start
//!   sending from (echoes the requested one when valid).
//! - On re-login the client sends `requested_session` + `requested_sequence`;
//!   the server replays from its message store — this is the TCP-side
//!   equivalent of MoldUDP64's NAK/retransmission ("session snapshot").
//! - `requested_sequence == 0` (or beyond the server high-water mark) means
//!   "start from the most recent".
//! - Heartbeats: both sides must send *something* at least every 1 s;
//!   a client may assume the link dead after ~15 s of silence (RxTimeout).
//!
//! # Transport decoupling
//!
//! This crate is pure bytes-in/bytes-out — it never opens a socket. The same
//! parser/session state machine runs over kernel TCP (production order entry)
//! or over DPDK-received frames in a user-space TCP-stack scenario.

pub mod ouch;
pub mod packet;
pub mod session;

pub use packet::{
    Packet, CLIENT_HEARTBEAT, DEBUG, END_OF_SESSION, LOGIN_ACCEPTED, LOGIN_REJECTED,
    LOGIN_REQUEST, LOGOUT_REQUEST, SEQUENCED_DATA, SERVER_HEARTBEAT, UNSEQUENCED_DATA,
};
pub use session::{ClientSession, ServerSession, SessionError};
