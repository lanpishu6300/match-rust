//! Hand-written DPDK FFI bindings (replaces bindgen; no clang needed).
//!
//! Layout facts verified against the headers shipped in Ubuntu noble's
//! `libdpdk-dev` (DPDK 23.11, aarch64, `RTE_IOVA_IN_MBUF=1`):
//!
//! ```text
//! struct rte_mbuf {                     offset
//!   RTE_MARKER cacheline0;                  0   (16B)
//!   void *buf_addr;                        16
//!   rte_iova_t buf_iova;                   24   (8B)
//!   RTE_MARKER64 rearm_data;               32
//!   uint16_t data_off;                     40
//!   uint16_t refcnt;                       42
//!   uint16_t nb_segs;                      44
//!   uint16_t port;                         46
//!   uint64_t ol_flags;                     48
//!   uint32_t packet_type;                  56
//!   uint32_t pkt_len;                      60
//!   uint16_t data_len;                     64
//!   ... (rest is opaque to us)
//! }
//! ```
//!
//! We only read `buf_addr`, `data_off`, `pkt_len`, `data_len` directly
//! (our own `mtod` / lengths); everything else goes through DPDK library
//! functions, so the struct is padded to the full 128-byte mbuf.

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::os::raw::{c_char, c_int, c_uint, c_void};

/// DPDK mbuf — only the fields we touch, padded to the real 128-byte size.
/// Offsets MEASURED on the Ubuntu noble DPDK 23.11.4 aarch64 headers
/// (offsetof: buf_addr=0, data_off=16, pkt_len=36, data_len=40 — this build
/// has no RTE_MARKER cacheline0 prefix; earlier hand-written layout was
/// x86_64-oriented and read garbage fields).
#[repr(C)]
pub struct rte_mbuf {
    pub buf_addr: *mut c_void, // @0
    _buf_iova: u64,            // @8
    pub data_off: u16,         // @16
    _refcnt: u16,              // @18
    _nb_segs: u16,             // @20
    _port: u16,                // @22
    _ol_flags: u64,            // @24
    _packet_type: u32,         // @32
    pub pkt_len: u32,          // @36
    pub data_len: u16,         // @40
    _pad: [u8; 86],            // 42..128
}

/// Opaque mempool handle.
#[repr(C)]
pub struct rte_mempool {
    _private: [u8; 0],
}

/// Device config — DPDK only reads it; a zeroed block means "all defaults".
/// Real size on DPDK 23.11 (aarch64) is 2280 bytes — measured against the
/// headers (sizeof(struct rte_eth_conf)=2280). Must be >= that or DPDK reads
/// past our buffer and misinterprets garbage (observed: -EINVAL + spurious
/// "does not support lsc").
#[repr(C)]
pub struct rte_eth_conf {
    _data: [u64; 285], // 2280 bytes
}

/// RX queue config — read-only, zeroed defaults are fine.
/// sizeof(struct rte_eth_rxconf)=80 (measured).
#[repr(C)]
pub struct rte_eth_rxconf {
    _data: [u64; 16], // 128 bytes >= 80
}

/// TX queue config — read-only, zeroed defaults are fine.
/// sizeof(struct rte_eth_txconf)=56 (measured).
#[repr(C)]
pub struct rte_eth_txconf {
    _data: [u64; 16], // 128 bytes >= 56
}

extern "C" {
    pub fn rte_eal_init(argc: c_int, argv: *mut *mut c_char) -> c_int;
    pub fn rte_eal_cleanup();
    pub fn rte_pktmbuf_pool_create(
        name: *const c_char,
        n: c_uint,
        cache_size: c_uint,
        priv_size: u16,
        data_room_size: u16,
        socket_id: c_int,
    ) -> *mut rte_mempool;
    pub fn rte_socket_id() -> c_uint;
    pub fn rte_eth_dev_configure(
        port_id: u16,
        nb_rx_queue: u16,
        nb_tx_queue: u16,
        eth_conf: *const rte_eth_conf,
    ) -> c_int;
    pub fn rte_eth_rx_queue_setup(
        port_id: u16,
        rx_queue_id: u16,
        nb_rx_desc: u16,
        socket_id: c_uint,
        rx_conf: *const rte_eth_rxconf,
        mb_pool: *mut rte_mempool,
    ) -> c_int;
    pub fn rte_eth_tx_queue_setup(
        port_id: u16,
        tx_queue_id: u16,
        nb_tx_desc: u16,
        socket_id: c_uint,
        tx_conf: *const rte_eth_txconf,
    ) -> c_int;
    pub fn rte_eth_dev_start(port_id: u16) -> c_int;
    pub fn rte_eth_dev_socket_id(port_id: u16) -> c_int;
    pub fn rte_vdev_init(name: *const c_char, args: *const c_char) -> c_int;
    pub fn rte_eth_dev_get_port_by_name(name: *const c_char, port_id: *mut u16) -> c_int;

    // ---- bridge.c: DPDK 23.11 data-plane APIs are header inline functions
    // with NO exported symbols in Ubuntu's librte_*.so.24.0. The C shim
    // (src/dpdk/bridge.c) compiled with -I dpdk headers exposes them. ----
    pub fn bridge_rx_burst(
        port_id: u16,
        queue_id: u16,
        rx_pkts: *mut *mut rte_mbuf,
        nb_pkts: u16,
    ) -> u16;
    pub fn bridge_tx_burst(
        port_id: u16,
        queue_id: u16,
        tx_pkts: *mut *mut rte_mbuf,
        nb_pkts: u16,
    ) -> u16;
    pub fn bridge_pktmbuf_alloc(mp: *mut rte_mempool) -> *mut rte_mbuf;
    /// Returns a pointer to the appended payload (C `char *`), NULL on failure.
    pub fn bridge_pktmbuf_append(m: *mut rte_mbuf, len: u16) -> *mut u8;
    pub fn bridge_pktmbuf_free(m: *mut rte_mbuf);
}

// Rust-side wrappers with the DPDK names so call sites stay unchanged.
pub unsafe fn rte_eth_rx_burst(
    port_id: u16,
    queue_id: u16,
    rx_pkts: *mut *mut rte_mbuf,
    nb_pkts: u16,
) -> u16 {
    bridge_rx_burst(port_id, queue_id, rx_pkts, nb_pkts)
}

pub unsafe fn rte_eth_tx_burst(
    port_id: u16,
    queue_id: u16,
    tx_pkts: *mut *mut rte_mbuf,
    nb_pkts: u16,
) -> u16 {
    bridge_tx_burst(port_id, queue_id, tx_pkts, nb_pkts)
}

pub unsafe fn rte_pktmbuf_alloc(mp: *mut rte_mempool) -> *mut rte_mbuf {
    bridge_pktmbuf_alloc(mp)
}

pub unsafe fn rte_pktmbuf_append(m: *mut rte_mbuf, len: u16) -> *mut u8 {
    bridge_pktmbuf_append(m, len)
}

pub unsafe fn rte_pktmbuf_free(m: *mut rte_mbuf) {
    bridge_pktmbuf_free(m)
}

// DPDK 23.11 rte_eth_stats 完整布局（前 7 个 u64 标量 + 5×RTE_ETHDEV_QUEUE_STAT_CNTRS(32) 数组）
// 尺寸必须与头文件一致，否则 rte_eth_stats_get 写越界破坏栈（SEGV）
#[repr(C)]
#[derive(Clone, Copy)]
pub struct rte_eth_stats {
    pub ipackets: u64,
    pub opackets: u64,
    pub ibytes: u64,
    pub obytes: u64,
    pub imissed: u64,
    pub oerrors: u64,
    pub rx_nombuf: u64,
    pub q_ipackets: [u64; 32],
    pub q_opackets: [u64; 32],
    pub q_ibytes: [u64; 32],
    pub q_obytes: [u64; 32],
    pub q_errors: [u64; 32],
}

impl Default for rte_eth_stats {
    fn default() -> Self {
        rte_eth_stats {
            ipackets: 0,
            opackets: 0,
            ibytes: 0,
            obytes: 0,
            imissed: 0,
            oerrors: 0,
            rx_nombuf: 0,
            q_ipackets: [0; 32],
            q_opackets: [0; 32],
            q_ibytes: [0; 32],
            q_obytes: [0; 32],
            q_errors: [0; 32],
        }
    }
}

extern "C" {
    fn rte_eth_stats_get(port_id: u16, stats: *mut rte_eth_stats) -> c_int;
}

pub unsafe fn eth_stats_get(port_id: u16) -> rte_eth_stats {
    let mut st = rte_eth_stats::default();
    unsafe { rte_eth_stats_get(port_id, &mut st) };
    st
}
