//! DPDK runtime helpers: EAL init (hugepage-free, PCI-free, container-safe),
//! mbuf zero-copy access, and port setup for the `net_pcap` virtual device
//! used to validate the full rx/tx path inside Docker without physical NICs.

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_uint, c_void};

pub mod ffi;
pub use ffi::*;

pub mod port;

pub const MBUF_POOL_SIZE: c_uint = 8192;
pub const MBUF_CACHE_SIZE: c_uint = 256;
pub const MBUF_DATA_ROOM: u16 = 2048;
pub const BURST_SIZE: u16 = 32;

/// Initialize the EAL. Default container-friendly flags: no hugepages, no PCI
/// scan (pcap vdev is virtual), single lcore unless `cores` says otherwise.
///
/// Set `DPDK_EAL_NATIVE=1` to run against a real physical NIC: PCI scan is
/// enabled and more memory is requested. VFIO binding is done out-of-band by
/// scripts/dpdk-verify-realnic.sh. `DPDK_EAL_EXTRA` appends arbitrary EAL
/// flags (e.g. `-w 0000:03:00.0,--log-level=pmd:8`).
///
/// Returns the number of EAL arguments consumed (used to keep argv alive).
pub fn eal_init(cores: &[u32], extra: &[&str]) -> Result<usize, String> {
    let mut args: Vec<CString> = Vec::new();
    args.push(CString::new("mold_dpdk").unwrap());
    if !cores.is_empty() {
        let list: Vec<String> = cores.iter().map(u32::to_string).collect();
        args.push(CString::new("-l").unwrap());
        args.push(CString::new(list.join(",")).unwrap());
    }
    let native = std::env::var("DPDK_EAL_NATIVE").is_ok();
    if native {
        // Real-NIC mode: scan PCI, rely on hugepages when present; fall back
        // to malloc memory when DPDK_EAL_NO_HUGE=1 (e.g. a VM without them).
        args.push(CString::new("-m").unwrap());
        args.push(CString::new("1024").unwrap());
        if std::env::var("DPDK_EAL_NO_HUGE").is_ok() {
            args.push(CString::new("--no-huge").unwrap());
        }
    } else {
        for flag in ["--no-huge", "--no-pci", "-m", "512"] {
            args.push(CString::new(flag).unwrap());
        }
    }
    for e in extra {
        args.push(CString::new(*e).unwrap());
    }
    // Debug hook: `DPDK_EAL_EXTRA="--log-level=pmd:8,--log-level=eal:8"` etc.
    if let Ok(ext) = std::env::var("DPDK_EAL_EXTRA") {
        for e in ext.split(',') {
            if !e.is_empty() {
                args.push(CString::new(e).unwrap());
            }
        }
    }
    let mut argv: Vec<*mut c_char> = args.iter().map(|s| s.as_ptr() as *mut c_char).collect();
    // SAFETY: argv outlives rte_eal_init via `args` staying in scope.
    let rc = unsafe { rte_eal_init(argv.len() as c_int, argv.as_mut_ptr()) };
    if rc < 0 {
        return Err(format!("rte_eal_init failed: {rc}"));
    }
    Ok(rc as usize)
}

pub fn eal_cleanup() {
    // SAFETY: no DPDK objects in use at this point.
    unsafe { rte_eal_cleanup() };
}

/// Create the mbuf pool (hugepage-free build uses malloc-backed memory).
pub fn mbuf_pool(name: &str) -> Result<*mut rte_mempool, String> {
    let cname = CString::new(name).unwrap();
    // SAFETY: static pool params; `(null)` as priv size.
    let pool = unsafe {
        rte_pktmbuf_pool_create(
            cname.as_ptr(),
            MBUF_POOL_SIZE,
            MBUF_CACHE_SIZE,
            0,
            MBUF_DATA_ROOM,
            rte_socket_id() as c_int,
        )
    };
    if pool.is_null() {
        return Err("rte_pktmbuf_pool_create failed".into());
    }
    Ok(pool)
}

/// Get the segment data start of an mbuf (the `rte_pktmbuf_mtod` inline we
/// reimplement): `buf_addr + data_off`.
pub unsafe fn mtod(m: *const rte_mbuf) -> *mut u8 {
    let buf_addr = (*m).buf_addr as *const u8;
    buf_addr.add((*m).data_off as usize) as *mut u8
}

pub unsafe fn pkt_len(m: *const rte_mbuf) -> usize {
    (*m).pkt_len as usize
}

pub unsafe fn data_len(m: *const rte_mbuf) -> usize {
    (*m).data_len as usize
}

/// Parse the IPv4 destination from an mbuf carrying Eth/IPv4/UDP. Returns
/// `None` for non-IPv4 frames.
pub unsafe fn udp_payload(m: *const rte_mbuf) -> Option<(*const u8, usize)> {
    let base = mtod(m);
    if pkt_len(m) < 14 + 20 + 8 {
        return None;
    }
    let ether_type = u16::from_be_bytes([*base.add(12), *base.add(13)]);
    if ether_type != 0x0800 {
        return None;
    }
    let ip = base.add(14);
    let proto = *ip.add(9);
    if proto != 17 {
        return None;
    }
    // IPv4 IHL in low nibble of byte 0.
    let ihl = (*ip & 0x0f) as usize * 4;
    let udp = ip.add(ihl);
    let total = pkt_len(m);
    let udp_off = 14 + ihl;
    let udp_len = u16::from_be_bytes([*udp.add(4), *udp.add(5)]) as usize;
    let payload_off = udp_off + 8;
    if payload_off > total || udp_len < 8 {
        return None;
    }
    let len = udp_len - 8;
    if payload_off + len > total {
        return None;
    }
    Some((base.add(payload_off), len))
}

/// Create a `net_pcap` vdev and return the port id it was attached as.
///
/// - rx mode: `rx_pcap=<path>` replays a pcap file through the full DPDK
///   receive path.
/// - tx mode: `tx_pcap=<path>` captures everything transmitted.
pub fn attach_pcap_vdev(name: &str, args: &str) -> Result<u16, String> {
    let cname = CString::new(name).unwrap();
    let cargs = CString::new(args).unwrap();
    // SAFETY: name/args are static strings kept alive for the call.
    let rc = unsafe { rte_vdev_init(cname.as_ptr(), cargs.as_ptr()) };
    if rc != 0 {
        return Err(format!("rte_vdev_init({name},{args}) failed: {rc}"));
    }
    let mut port: u16 = u16::MAX;
    // SAFETY: port written by DPDK; name outlives the call.
    let rc2 = unsafe { rte_eth_dev_get_port_by_name(cname.as_ptr(), &mut port as *mut u16) };
    if rc2 != 0 || port == u16::MAX {
        return Err(format!("rte_eth_dev_get_port_by_name({name}) failed: {rc2}"));
    }
    Ok(port)
}

/// Minimal Ethernet/IPv4/UDP header writers for tx frames.
pub mod frames {
    use super::*;

    pub fn eth_ip_udp_header(
        dst_mac: [u8; 6],
        src_mac: [u8; 6],
        dst_ip: [u8; 4],
        src_ip: [u8; 4],
        udp_dport: u16,
        udp_sport: u16,
        payload_len: usize,
    ) -> Vec<u8> {
        let udp_len = (8 + payload_len) as u16;
        let ip_total = (20 + udp_len as usize) as u16;
        let mut b = Vec::with_capacity(14 + 20 + 8 + payload_len);
        b.extend_from_slice(&dst_mac);
        b.extend_from_slice(&src_mac);
        b.extend_from_slice(&[0x08, 0x00]); // IPv4
        // IPv4 header
        b.push(0x45); // v4, IHL=5
        b.push(0x00); // DSCP
        b.extend_from_slice(&ip_total.to_be_bytes());
        b.extend_from_slice(&[0, 0]); // id
        b.extend_from_slice(&[0, 0]); // flags/frag
        b.push(64); // TTL
        b.push(17); // UDP
        b.extend_from_slice(&[0, 0]); // checksum (ignored by pcap PMD)
        b.extend_from_slice(&src_ip);
        b.extend_from_slice(&dst_ip);
        // UDP
        b.extend_from_slice(&udp_sport.to_be_bytes());
        b.extend_from_slice(&udp_dport.to_be_bytes());
        b.extend_from_slice(&udp_len.to_be_bytes());
        b.extend_from_slice(&[0, 0]); // checksum
        b
    }
}

/// SAFETY helper: read a NUL-terminated C string (for DPDK name lookups).
pub unsafe fn cstr(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    CStr::from_ptr(ptr).to_string_lossy().into_owned()
}

#[allow(dead_code)]
pub(crate) fn _keep_void(_: *mut c_void) {}
