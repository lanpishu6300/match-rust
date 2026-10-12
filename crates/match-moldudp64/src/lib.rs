//! # match-moldudp64
//!
//! MoldUDP64 (Nasdaq 1.00) multicast market-data transport for the match-rust
//! workspace — a **zero-dependency, big-endian, UDP-based** publisher /
//! subscriber / retransmit-server implementation.
//!
//! ```text
//!  matching engine ──► MoldPublisher ──► UDP multicast ──► MoldSubscriber(s)
//!                           │  ▲                                   │
//!                     ring cache │                          gap detected?
//!                           │  └─────── unicast Downstream ◄────┘
//!                           ▼
//!               MoldRetransmitServer ◄── NAK Request (unicast) ──┘
//! ```
//!
//! ## Modules
//!
//! - [`types`]: wire format codecs (big-endian, no external deps).
//! - [`publisher`]: production side — seq allocation, packet packing,
//!   heartbeat, bounded retransmission cache.
//! - [`subscriber`]: consumer side — parse, seq tracking, gap detection, NAK.
//! - [`retransmit`]: NAK server replying from the shared publisher cache.
//! - [`ring_buf`]: the shared bounded message history.
//!
//! ## Usage sketch
//!
//! ```rust,no_run
//! use match_moldudp64::{MoldPublisher, MoldSubscriber, MoldRetransmitServer};
//! use std::net::SocketAddr;
//!
//! let group: SocketAddr = "239.255.0.1:50000".parse().unwrap();
//! let rt_addr: SocketAddr = "127.0.0.1:50001".parse().unwrap();
//!
//! let pubr = MoldPublisher::new("MATCH_RUST", group, 4096).unwrap();
//! pubr.publish_tagged(MoldPublisher::TAG_FILL, b"fill-payload");
//! pubr.send_heartbeat().unwrap();
//!
//! // Retransmit server shares the publisher's cache.
//! let server = MoldRetransmitServer::new("MATCH_RUST", rt_addr, pubr.shared_cache()).unwrap();
//!
//! let mut sub = MoldSubscriber::new(
//!     "MATCH_RUST",
//!     "0.0.0.0:50000".parse().unwrap(),
//!     Some("239.255.0.1".parse().unwrap()),
//!     Some(rt_addr),
//! )
//! .unwrap();
//! let mut buf = [0u8; 1500];
//! while let Some(out) = sub.recv(&mut buf).unwrap() {
//!     if let Some(gap) = out.gap {
//!         sub.send_nak(gap.expected, gap.lost.min(100) as u16).unwrap();
//!     }
//! }
//! # let _ = (&pubr, &server);
//! ```

pub mod publisher;
pub mod retransmit;
pub mod ring_buf;
pub mod subscriber;
pub mod types;

pub use publisher::MoldPublisher;
pub use retransmit::MoldRetransmitServer;
pub use ring_buf::{CachedMessage, MessageRingBuf, SharedRingBuf};
pub use subscriber::{GapInfo, MoldSubscriber, ParseOutcome};
pub use types::{
    get_u16_be, get_u64_be, pack_session, put_u16_be, put_u64_be, DownstreamHeader,
    RequestHeader, MSG_TAG_DEPTH, MSG_TAG_FILL_ORDER, MOLD_BLOCK_HEADER_LEN,
    MOLD_DOWNSTREAM_HEADER_LEN, MOLD_MAX_DATAGRAM, MOLD_REQUEST_HEADER_LEN, MOLD_SESSION_LEN,
};
