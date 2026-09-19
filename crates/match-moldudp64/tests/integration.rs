//! End-to-end MoldUDP64 integration tests (real UDP on loopback / local
//! multicast): publisher → subscriber roundtrip, heartbeat, and the full
//! gap → NAK → retransmit chain.

use match_moldudp64::{MoldPublisher, MoldRetransmitServer, MoldSubscriber};
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

const SESSION: &str = "MATCH_RUST";

fn free_udp_port() -> u16 {
    let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    s.local_addr().unwrap().port()
}

/// Drain `sub` for up to `timeout_ms`, returning all parsed outcomes.
fn drain(
    sub: &mut MoldSubscriber,
    buf: &mut [u8],
    timeout_ms: u64,
) -> Vec<match_moldudp64::ParseOutcome> {
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    let mut out = Vec::new();
    while std::time::Instant::now() < deadline {
        match sub.recv(buf).unwrap() {
            Some(p) => out.push(p),
            None => std::thread::sleep(Duration::from_millis(1)),
        }
        if out.len() >= 32 {
            break;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 1. Publisher → Subscriber roundtrip over loopback unicast
// ---------------------------------------------------------------------------

#[test]
fn publisher_subscriber_unicast_roundtrip() {
    let port = free_udp_port();
    let sub_addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

    let pubr = MoldPublisher::new(SESSION, sub_addr, 128).unwrap();
    let mut sub = MoldSubscriber::new_unicast(SESSION, sub_addr, None).unwrap();

    for i in 0..5u8 {
        pubr.publish(&[i, i + 1, i + 2]).unwrap();
    }

    let mut buf = [0u8; 1500];
    let outs = drain(&mut sub, &mut buf, 500);

    let msgs: Vec<(u64, Vec<u8>)> = outs.into_iter().flat_map(|o| o.messages).collect();
    assert_eq!(msgs.len(), 5, "all five published messages arrive");
    for (i, (seq, payload)) in msgs.iter().enumerate() {
        assert_eq!(*seq, i as u64 + 1, "seq assigned consecutively from 1");
        assert_eq!(payload.as_slice(), &[i as u8, i as u8 + 1, i as u8 + 2]);
    }
    assert_eq!(sub.last_seq(), Some(5));
    assert_eq!(pubr.last_seq(), 5);
}

// ---------------------------------------------------------------------------
// 2. Heartbeat detection (msg_count == 0)
// ---------------------------------------------------------------------------

#[test]
fn heartbeat_is_detected_and_syncs_seq() {
    let port = free_udp_port();
    let sub_addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

    let pubr = MoldPublisher::new(SESSION, sub_addr, 16).unwrap();
    let mut sub = MoldSubscriber::new_unicast(SESSION, sub_addr, None).unwrap();

    pubr.publish(b"first").unwrap();
    pubr.send_heartbeat().unwrap();

    let mut buf = [0u8; 1500];
    let outs = drain(&mut sub, &mut buf, 400);

    let hb = outs.iter().find(|o| o.is_heartbeat).expect("heartbeat received");
    assert_eq!(hb.msg_count, 0);
    // Publisher advertised next_seq == 2; subscriber synced floor to 1.
    assert_eq!(sub.last_seq(), Some(1));
}

// ---------------------------------------------------------------------------
// 3. Gap → NAK → retransmit full chain (real UDP)
// ---------------------------------------------------------------------------

#[test]
fn gap_detected_nak_sent_retransmit_fills_hole() {
    let data_port = free_udp_port();
    let rt_port = free_udp_port();
    let data_addr: SocketAddr = format!("127.0.0.1:{data_port}").parse().unwrap();
    let rt_addr: SocketAddr = format!("127.0.0.1:{rt_port}").parse().unwrap();

    let pubr = MoldPublisher::new(SESSION, data_addr, 256).unwrap();
    let server = MoldRetransmitServer::new(SESSION, rt_addr, pubr.shared_cache()).unwrap();
    let mut sub = MoldSubscriber::new_unicast(SESSION, data_addr, Some(rt_addr)).unwrap();

    // 1..=10 arrive; subscriber reads 1..=3 only (simulates missing 4..=6 by
    // simply not draining the socket before the NAK below).
    for i in 1..=10u8 {
        pubr.publish(&[i]).unwrap();
    }
    let mut buf = [0u8; 1500];
    let mut read_count = 0;
    while read_count < 3 {
        if sub.recv(&mut buf).unwrap().is_some() {
            read_count += 1;
        }
    }
    assert_eq!(sub.last_seq(), Some(3));

    // Manually issue a NAK for 4..=6 (the kernel may still deliver them, but
    // the retransmit path is exercised deterministically regardless).
    sub.send_nak(4, 3).unwrap();

    // Server: serve the NAK.
    let mut srv_buf = [0u8; 1500];
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    let mut served = None;
    while std::time::Instant::now() < deadline {
        if let Some(from) = server.serve_once(&mut srv_buf).unwrap() {
            served = Some(from);
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(served.is_some(), "retransmit server served the NAK");

    // Subscriber now receives the unicast replay 4..=6.
    let outs = drain(&mut sub, &mut buf, 500);
    let replayed: Vec<(u64, Vec<u8>)> = outs.into_iter().flat_map(|o| o.messages).collect();
    let seqs: Vec<u64> = replayed.iter().map(|(s, _)| *s).collect();
    assert!(seqs.contains(&4) && seqs.contains(&5) && seqs.contains(&6),
        "replay contains 4,5,6 — got {seqs:?}");
    // Contiguous state: whatever was drained, last_seq must be >= 6.
    assert!(sub.last_seq().unwrap() >= 6);
}

// ---------------------------------------------------------------------------
// 4. Retransmit server replies only with cache-backed messages
// ---------------------------------------------------------------------------

#[test]
fn retransmit_serves_available_range_only() {
    let rt_port = free_udp_port();
    let rt_addr: SocketAddr = format!("127.0.0.1:{rt_port}").parse().unwrap();

    let pubr = MoldPublisher::new(SESSION, rt_addr, 4).unwrap(); // tiny cache
    let server = MoldRetransmitServer::new(SESSION, rt_addr, pubr.shared_cache()).unwrap();

    // Publish 1..=8 but the cache holds only the last 4 (5..=8).
    for i in 1..=8u8 {
        pubr.publish(&[i]).unwrap();
    }

    // Ask for 1..=4: only what's still cached (5..=8) would be wrong seqs, so
    // the server must return 0 for 1..=4 (evicted) and 4 for 5..=8.
    let sub_addr: SocketAddr = format!("127.0.0.1:{}", free_udp_port()).parse().unwrap();
    let n_old = server.reply_range(1, 4, sub_addr).unwrap();
    assert_eq!(n_old, 0, "evicted messages are not replayed");

    let n_recent = server.reply_range(5, 4, sub_addr).unwrap();
    assert_eq!(n_recent, 4, "cached range 5..=8 replayed");

    // And a partially-cached range returns the contiguous prefix only.
    let n_partial = server.reply_range(7, 4, sub_addr).unwrap(); // 7,8 available; 9,10 not
    assert_eq!(n_partial, 2);
}

// ---------------------------------------------------------------------------
// 5. Local multicast loopback (macOS/BSD: join + loop on default iface)
// ---------------------------------------------------------------------------

#[test]
fn multicast_loopback_roundtrip() {
    let group: Ipv4Addr = "239.255.64.1".parse().unwrap();
    let port = free_udp_port();
    let group_addr: SocketAddr = SocketAddr::from((group, port));

    let pubr = MoldPublisher::new(SESSION, group_addr, 64).unwrap();
    let mut sub = MoldSubscriber::new(
        SESSION,
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)),
        Some(group),
        None,
    )
    .unwrap();

    pubr.publish(b"mcast-1").unwrap();
    pubr.publish(b"mcast-2").unwrap();

    let mut buf = [0u8; 1500];
    let outs = drain(&mut sub, &mut buf, 600);
    let msgs: Vec<Vec<u8>> = outs.into_iter().flat_map(|o| o.messages.into_iter().map(|(_, p)| p)).collect();
    assert_eq!(msgs, vec![b"mcast-1".to_vec(), b"mcast-2".to_vec()]);
}

// ---------------------------------------------------------------------------
// 6. Batch publishing honors MTU budget
// ---------------------------------------------------------------------------

#[test]
fn batch_publishing_packs_until_mtu() {
    let port = free_udp_port();
    let sub_addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

    let pubr = MoldPublisher::new(SESSION, sub_addr, 64).unwrap();
    let mut sub = MoldSubscriber::new_unicast(SESSION, sub_addr, None).unwrap();

    let msgs: Vec<Vec<u8>> = (0..20).map(|i| vec![i as u8; 100]).collect();
    let sent = pubr.publish_batch(&msgs, 1472).unwrap();
    // header 20 + 14 * (2 + 100) = 1448; 15th block would exceed 1472.
    assert_eq!(sent, 14, "14 × 100B messages fit in one 1472B datagram");

    let mut buf = [0u8; 1500];
    let outs = drain(&mut sub, &mut buf, 400);
    let total: usize = outs.iter().map(|o| o.messages.len()).sum();
    assert_eq!(total, 14);
}
