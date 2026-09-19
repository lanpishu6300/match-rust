//! Session state machines (client & server) — login, sequencing, heartbeat,
//! and the "session snapshot" re-login/retransmission flow.
//!
//! Transport-free: the caller owns the socket and hands the session state
//! machine inbound packets; the machine returns outbound packets + actions.
//!
//! ```text
//! client                          server
//!   |--- Login Request (session,seq) ->|
//!   |<- Login Accepted (start_seq) ----|   (or Login Rejected -> close)
//!   |--- [Unsequenced Data]* -------->|   orders (OUCH inbound)
//!   |<- [Sequenced Data | Heartbeat]*--|   reports (OUCH/ITCH downstream)
//!   |--- Client Heartbeat (1s) ------>|   keepalive both directions
//!   |--- Logout Request ------------>|   (server closes)
//!   |<- End of Session ---------------|   (session terminated)
//! ```

use crate::packet::{
    self, LoginAccepted, LoginRequest, Packet, REJECT_SESSION_UNAVAILABLE, SESSION_LEN,
};

#[derive(Debug)]
pub enum SessionError {
    NotLoggedIn,
    AlreadyLoggedIn,
    LoginRejected(u8),
    SequenceGap { expected: u64, got: u64 },
    UnexpectedPacket(char),
    SessionTooLong,
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotLoggedIn => write!(f, "not logged in yet"),
            Self::AlreadyLoggedIn => write!(f, "already logged in"),
            Self::LoginRejected(r) => write!(f, "login rejected: reason={:?}", char::from(*r)),
            Self::SequenceGap { expected, got } => {
                write!(f, "sequence gap: expected={expected} got={got}")
            }
            Self::UnexpectedPacket(t) => write!(f, "unexpected packet type {t:?}"),
            Self::SessionTooLong => write!(f, "session too long (max {SESSION_LEN} chars)"),
        }
    }
}

impl std::error::Error for SessionError {}

/// Outcome of feeding one inbound packet into a session machine.
#[derive(Debug, Default)]
pub struct Actions {
    /// Packets the caller should write to the socket.
    pub outbound: Vec<Packet>,
    /// True when the server asks to close the connection (reject/logout/eos).
    pub close: bool,
    /// Heartbeat deadline hint (ns since epoch) if the link should be
    /// considered dead; `None` means keep-alive is healthy.
    pub heartbeat_deadline: Option<u64>,
}

/// Server-side session: one active session id, a message store for
/// retransmission ("session snapshot"), and per-connection start sequence.
#[derive(Debug)]
pub struct ServerSession {
    session_id: String,
    /// High-water mark: next sequence to assign.
    next_seq: u64,
    /// Message store (seq -> payload) for replay on re-login.
    store: std::collections::BTreeMap<u64, Vec<u8>>,
    store_capacity: usize,
    logged_in: bool,
}

impl ServerSession {
    pub fn new(session_id: &str, store_capacity: usize) -> Result<Self, SessionError> {
        if session_id.len() > SESSION_LEN {
            return Err(SessionError::SessionTooLong);
        }
        Ok(Self {
            session_id: session_id.to_string(),
            next_seq: 1,
            store: Default::default(),
            store_capacity,
            logged_in: false,
        })
    }

    /// Append a downstream business message (e.g. an OUCH execution report)
    /// and return its assigned sequence number.
    pub fn enqueue(&mut self, payload: &[u8]) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        if self.store.len() >= self.store_capacity {
            if let Some(oldest) = self.store.keys().next().copied() {
                self.store.remove(&oldest);
            }
        }
        self.store.insert(seq, payload.to_vec());
        seq
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Sequence of the next downstream message (heartbeat semantics:
    /// MoldUDP64-style "next to be sent").
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Handle a Login Request. On success returns the start sequence for the
    /// client (requested seq if valid, else current high-water mark).
    pub fn handle_login(&mut self, req: &LoginRequest) -> Result<Actions, SessionError> {
        let mut a = Actions::default();
        // Authentication hook is the caller's job (username/password
        // validation); here we only enforce the session + sequence contract.
        if !req.requested_session.is_empty() && req.requested_session != self.session_id {
            a.outbound.push(packet::login_rejected(REJECT_SESSION_UNAVAILABLE));
            a.close = true;
            return Ok(a);
        }
        let start = if req.requested_sequence == 0 || req.requested_sequence > self.next_seq {
            self.next_seq
        } else {
            req.requested_sequence
        };
        // Clamp to the store window: a request older than what we still hold
        // can only be served from the oldest retained sequence onward (same
        // "too old → serve from earliest available" behavior as MoldUDP64
        // when the NAK target fell off the retransmission window).
        let store_start = self.store.keys().next().copied().unwrap_or(self.next_seq);
        let start = start.max(store_start);
        self.logged_in = true;
        a.outbound.push(packet::login_accepted(&self.session_id, start));
        // Replay from the store ("session snapshot") if the client asked for
        // an earlier sequence and we still have it.
        if start < self.next_seq {
            for (seq, msg) in self.store.range(start..) {
                let _ = seq;
                a.outbound.push(Packet::new(packet::SEQUENCED_DATA, msg.clone()));
            }
        }
        Ok(a)
    }

    /// Handle a client message (Unsequenced Data = order entry; heartbeats;
    /// logout). The caller supplies an authentication verdict for login.
    pub fn handle_packet(&mut self, pkt: &Packet) -> Result<Actions, SessionError> {
        let mut a = Actions::default();
        match pkt.packet_type {
            packet::LOGIN_REQUEST => {
                let req = pkt.parse_login_request();
                return self.handle_login(&req);
            }
            packet::UNSEQUENCED_DATA => {
                // Order-entry payload (e.g. OUCH Enter/Cancel/Replace).
                // The caller decides how to respond; outbound reports are
                // appended via `enqueue` + send_sequenced below.
            }
            packet::CLIENT_HEARTBEAT => {}
            packet::LOGOUT_REQUEST => {
                a.close = true;
            }
            packet::DEBUG => {}
            other => return Err(SessionError::UnexpectedPacket(char::from(other))),
        }
        Ok(a)
    }

    /// Wrap a downstream payload as a Sequenced Data packet and remove it from
    /// the pending queue (caller must have obtained it via `enqueue`).
    pub fn sequenced_packet(&self, payload: &[u8]) -> Packet {
        Packet::new(packet::SEQUENCED_DATA, payload.to_vec())
    }
}

/// Client-side session: tracks expected sequence, handles login response,
/// heartbeats, and re-login ("session snapshot") requests.
#[derive(Debug)]
pub struct ClientSession {
    pub session_id: String,
    /// Next downstream sequence the client expects.
    pub expected_seq: u64,
    /// Sequence the server granted at login (from Login Accepted).
    pub start_seq: u64,
    logged_in: bool,
    /// Time (ns) of last inbound activity, for heartbeat deadline checks.
    pub last_rx_ns: u64,
}

impl ClientSession {
    pub fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            expected_seq: 1,
            start_seq: 0,
            logged_in: false,
            last_rx_ns: 0,
        }
    }

    pub fn is_logged_in(&self) -> bool {
        self.logged_in
    }

    /// Build the initial Login Request (`requested_sequence = 0` → "most
    /// recent", or the client's expected seq for re-login).
    pub fn login_request(&self, username: &str, password: &str) -> Packet {
        packet::login_request(
            username,
            password,
            &self.session_id,
            self.expected_seq,
        )
    }

    /// Feed a server packet. Returns actions (heartbeats to send etc.).
    pub fn handle_packet(&mut self, pkt: &Packet, now_ns: u64) -> Result<Actions, SessionError> {
        self.last_rx_ns = now_ns;
        let mut a = Actions::default();
        match pkt.packet_type {
            packet::LOGIN_ACCEPTED => {
                let acc: LoginAccepted = pkt.parse_login_accepted();
                self.start_seq = acc.sequence;
                self.expected_seq = acc.sequence;
                self.logged_in = true;
            }
            packet::LOGIN_REJECTED => {
                return Err(SessionError::LoginRejected(pkt.parse_login_rejected()));
            }
            packet::SEQUENCED_DATA => {
                if !self.logged_in {
                    return Err(SessionError::NotLoggedIn);
                }
                // Server increments the implied sequence per Sequenced Data
                // packet. A gap means the stream broke — the client should
                // re-login with expected_seq to trigger the snapshot replay.
                if pkt.payload.is_empty() {
                    // End of Session marker (zero-length message).
                    self.logged_in = false;
                    return Ok(a);
                }
                if self.expected_seq > self.start_seq + 1 {
                    // unreachable in practice; kept for clarity
                }
                self.expected_seq += 1;
            }
            packet::SERVER_HEARTBEAT => {}
            packet::END_OF_SESSION => {
                self.logged_in = false;
                a.close = true;
            }
            packet::DEBUG => {}
            other => return Err(SessionError::UnexpectedPacket(char::from(other))),
        }
        Ok(a)
    }
}
