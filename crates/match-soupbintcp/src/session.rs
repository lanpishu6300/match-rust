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
    self, LoginAccepted, LoginRequest, Packet, REJECT_NOT_AUTHORIZED, REJECT_SESSION_UNAVAILABLE,
    SESSION_LEN,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet;

    fn lr(session: &str, seq: u64) -> LoginRequest {
        LoginRequest {
            username: "u1".into(),
            password: "p1".into(),
            requested_session: session.into(),
            requested_sequence: seq,
        }
    }

    // ---- ServerSession ----

    #[test]
    fn session_id_length_boundaries() {
        assert!(matches!(
            ServerSession::new("12345678901", 8),
            Err(SessionError::SessionTooLong)
        ));
        // Exactly SESSION_LEN (10) is allowed.
        let s = ServerSession::new("1234567890", 8).unwrap();
        assert_eq!(s.session_id(), "1234567890");
        // Empty session id is allowed (matches "any" acceptance).
        assert!(ServerSession::new("", 8).is_ok());
    }

    #[test]
    fn enqueue_assigns_sequential_seqs_and_evicts_oldest() {
        let mut s = ServerSession::new("S", 3).unwrap();
        assert_eq!(s.next_seq(), 1);
        assert_eq!(s.enqueue(b"m1"), 1);
        assert_eq!(s.enqueue(b"m2"), 2);
        assert_eq!(s.enqueue(b"m3"), 3);
        assert_eq!(s.next_seq(), 4);
        // 4th enqueue evicts seq=1 (oldest).
        assert_eq!(s.enqueue(b"m4"), 4);
        assert_eq!(s.next_seq(), 5);
        // store_capacity = 3 => only 2,3,4 retained.
        let mut s2 = ServerSession::new("S", 3).unwrap();
        for i in 1..=6u64 {
            s2.enqueue(&[i as u8]);
        }
        assert_eq!(s2.next_seq(), 7);
    }

    #[test]
    fn login_rejects_foreign_session() {
        let mut s = ServerSession::new("SESS_A", 8).unwrap();
        let a = s.handle_login(&lr("SESS_B", 0)).unwrap();
        assert!(a.close);
        assert_eq!(a.outbound.len(), 1);
        let p = &a.outbound[0];
        assert_eq!(p.packet_type, packet::LOGIN_REJECTED);
        assert_eq!(p.parse_login_rejected(), REJECT_SESSION_UNAVAILABLE);
    }

    #[test]
    fn login_accepts_matching_or_empty_session() {
        let mut s = ServerSession::new("SESS_A", 8).unwrap();
        let a = s.handle_login(&lr("SESS_A", 0)).unwrap();
        assert!(!a.close);
        assert_eq!(a.outbound.len(), 1);
        let acc = a.outbound[0].parse_login_accepted();
        assert_eq!(acc.session, "SESS_A");
        assert_eq!(acc.sequence, 1); // requested 0 => current high-water

        // Empty requested session is treated as "any".
        let mut s2 = ServerSession::new("SESS_A", 8).unwrap();
        let a2 = s2.handle_login(&lr("", 0)).unwrap();
        assert!(!a2.close);
        assert_eq!(a2.outbound.len(), 1);
    }

    #[test]
    fn login_sequence_clamping_and_snapshot_replay() {
        // Seed the store with seq 1..=4.
        let mut s = ServerSession::new("S", 64).unwrap();
        for i in 1..=4u64 {
            s.enqueue(&[i as u8]);
        }
        assert_eq!(s.next_seq(), 5);

        // requested seq beyond high-water => clamp to next_seq (5).
        let a = s.handle_login(&lr("S", 100)).unwrap();
        let acc = a.outbound[0].parse_login_accepted();
        assert_eq!(acc.sequence, 5);
        // No replay since start == next_seq.
        assert_eq!(a.outbound.len(), 1);

        // requested seq within store => replay snapshot from that seq.
        let mut s2 = ServerSession::new("S", 64).unwrap();
        for i in 1..=4u64 {
            s2.enqueue(&[i as u8]);
        }
        let a2 = s2.handle_login(&lr("S", 3)).unwrap();
        let acc2 = a2.outbound[0].parse_login_accepted();
        assert_eq!(acc2.sequence, 3);
        // 1 accept + replay 3,4 = 3 outbound packets.
        assert_eq!(a2.outbound.len(), 3);
        assert_eq!(a2.outbound[1].packet_type, packet::SEQUENCED_DATA);
        assert_eq!(a2.outbound[1].payload, vec![3u8]);
        assert_eq!(a2.outbound[2].payload, vec![4u8]);

        // requested seq older than store window => clamp to oldest retained.
        let mut s3 = ServerSession::new("S", 2).unwrap();
        for i in 1..=4u64 {
            s3.enqueue(&[i as u8]);
        }
        // store now holds 3,4 (oldest = 3).
        let a3 = s3.handle_login(&lr("S", 1)).unwrap();
        let acc3 = a3.outbound[0].parse_login_accepted();
        assert_eq!(acc3.sequence, 3, "clamped to oldest retained seq");
        // accept + replay of the whole retained window (3,4) = 3 outbound.
        assert_eq!(a3.outbound.len(), 3);
        assert_eq!(a3.outbound[1].payload, vec![3u8]);
        assert_eq!(a3.outbound[2].payload, vec![4u8]);
    }

    #[test]
    fn handle_packet_routes_types() {
        let mut s = ServerSession::new("S", 8).unwrap();
        // UNSEQUENCED_DATA => no actions, no close.
        let a = s.handle_packet(&Packet::new(packet::UNSEQUENCED_DATA, b"order".to_vec())).unwrap();
        assert!(a.outbound.is_empty());
        assert!(!a.close);
        // CLIENT_HEARTBEAT => no actions.
        let a = s.handle_packet(&Packet::new(packet::CLIENT_HEARTBEAT, vec![])).unwrap();
        assert!(a.outbound.is_empty());
        assert!(!a.close);
        // DEBUG => ignored.
        let a = s.handle_packet(&Packet::new(packet::DEBUG, vec![])).unwrap();
        assert!(a.outbound.is_empty());
        // LOGOUT => close.
        let a = s.handle_packet(&Packet::new(packet::LOGOUT_REQUEST, vec![])).unwrap();
        assert!(a.close);
        // LOGIN_REQUEST routes to handle_login.
        let mut s2 = ServerSession::new("S", 8).unwrap();
        let req = packet::login_request("u", "p", "S", 0);
        let a2 = s2.handle_packet(&req).unwrap();
        assert_eq!(a2.outbound[0].packet_type, packet::LOGIN_ACCEPTED);
        // Unknown type => error.
        let a3 = s.handle_packet(&Packet::new(b'Q', vec![]));
        assert!(matches!(a3, Err(SessionError::UnexpectedPacket('Q'))));
    }

    #[test]
    fn sequenced_packet_wraps_payload() {
        let s = ServerSession::new("S", 8).unwrap();
        let p = s.sequenced_packet(b"report");
        assert_eq!(p.packet_type, packet::SEQUENCED_DATA);
        assert_eq!(p.payload, b"report");
    }

    #[test]
    fn server_error_display() {
        assert_eq!(SessionError::NotLoggedIn.to_string(), "not logged in yet");
        assert_eq!(SessionError::AlreadyLoggedIn.to_string(), "already logged in");
        assert_eq!(
            SessionError::LoginRejected(REJECT_NOT_AUTHORIZED).to_string(),
            "login rejected: reason='A'"
        );
        assert_eq!(
            SessionError::SequenceGap { expected: 5, got: 8 }.to_string(),
            "sequence gap: expected=5 got=8"
        );
        assert_eq!(
            SessionError::UnexpectedPacket('X').to_string(),
            "unexpected packet type 'X'"
        );
        assert_eq!(
            SessionError::SessionTooLong.to_string(),
            "session too long (max 10 chars)"
        );
    }

    // ---- ClientSession ----

    #[test]
    fn client_initial_state() {
        let c = ClientSession::new("SESS_A");
        assert_eq!(c.expected_seq, 1);
        assert_eq!(c.start_seq, 0);
        assert!(!c.is_logged_in());
    }

    #[test]
    fn client_login_request_uses_expected_seq() {
        let mut c = ClientSession::new("SESS_A");
        c.expected_seq = 77;
        let p = c.login_request("user", "pass");
        assert_eq!(p.packet_type, packet::LOGIN_REQUEST);
        let lr = p.parse_login_request();
        assert_eq!(lr.requested_session, "SESS_A");
        assert_eq!(lr.requested_sequence, 77);
        assert_eq!(lr.username, "user");
        assert_eq!(lr.password, "pass");
    }

    #[test]
    fn client_login_accepted_sets_watermarks() {
        let mut c = ClientSession::new("SESS_A");
        let acc = packet::login_accepted("SESS_A", 42);
        let a = c.handle_packet(&acc, 1_000).unwrap();
        assert!(c.is_logged_in());
        assert_eq!(c.start_seq, 42);
        assert_eq!(c.expected_seq, 42);
        assert_eq!(c.last_rx_ns, 1_000);
        assert!(a.outbound.is_empty());
        assert!(!a.close);
        assert!(a.heartbeat_deadline.is_none());
    }

    #[test]
    fn client_login_rejected_raises_error() {
        let mut c = ClientSession::new("SESS_A");
        let rej = packet::login_rejected(REJECT_NOT_AUTHORIZED);
        let err = c.handle_packet(&rej, 0).unwrap_err();
        assert!(matches!(err, SessionError::LoginRejected(r) if r == REJECT_NOT_AUTHORIZED));
        assert!(!c.is_logged_in());
    }

    #[test]
    fn client_sequenced_data_requires_login() {
        let mut c = ClientSession::new("SESS_A");
        let p = Packet::new(packet::SEQUENCED_DATA, b"x".to_vec());
        assert!(matches!(c.handle_packet(&p, 0), Err(SessionError::NotLoggedIn)));
    }

    #[test]
    fn client_sequenced_data_advances_seq_and_empty_means_eos() {
        let mut c = ClientSession::new("SESS_A");
        c.handle_packet(&packet::login_accepted("SESS_A", 10), 0).unwrap();
        assert_eq!(c.expected_seq, 10);
        c.handle_packet(&Packet::new(packet::SEQUENCED_DATA, b"r1".to_vec()), 0).unwrap();
        assert_eq!(c.expected_seq, 11);
        c.handle_packet(&Packet::new(packet::SEQUENCED_DATA, b"r2".to_vec()), 0).unwrap();
        assert_eq!(c.expected_seq, 12);
        // Zero-length sequenced data = End of Session marker => logged out.
        let a = c.handle_packet(&Packet::new(packet::SEQUENCED_DATA, vec![]), 0).unwrap();
        assert!(!c.is_logged_in());
        assert!(!a.close);
    }

    #[test]
    fn client_heartbeat_and_debug_ignored() {
        let mut c = ClientSession::new("S");
        let a = c.handle_packet(&Packet::new(packet::SERVER_HEARTBEAT, vec![]), 5).unwrap();
        assert!(a.outbound.is_empty());
        assert!(!a.close);
        c.handle_packet(&Packet::new(packet::DEBUG, vec![]), 6).unwrap();
        assert_eq!(c.last_rx_ns, 6);
    }

    #[test]
    fn client_end_of_session_closes_and_logs_out() {
        let mut c = ClientSession::new("S");
        c.handle_packet(&packet::login_accepted("S", 1), 0).unwrap();
        let a = c.handle_packet(&packet::end_of_session(), 0).unwrap();
        assert!(a.close);
        assert!(!c.is_logged_in());
    }

    #[test]
    fn client_unknown_packet_raises() {
        let mut c = ClientSession::new("S");
        // b'Z' is END_OF_SESSION (a valid type), so use b'Q' for the
        // genuinely-unknown path.
        assert!(matches!(
            c.handle_packet(&Packet::new(b'Q', vec![]), 0),
            Err(SessionError::UnexpectedPacket('Q'))
        ));
    }
}
