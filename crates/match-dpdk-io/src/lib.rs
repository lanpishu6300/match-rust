//! match-dpdk-io — DPDK I/O backend for `match-moldudp64`.
//!
//! Linux-only. On macOS this crate compiles to an empty shell (all modules
//! behind `cfg(target_os = "linux")`), so the workspace never breaks on the
//! dev machine. See `docs/moldudp64-dpdk-integration.md` for the design.

#[cfg(target_os = "linux")]
pub mod dpdk;

#[cfg(target_os = "linux")]
pub use dpdk::*;

/// Version marker for the DPDK backend (not the DPDK library version).
pub const BACKEND: &str = "dpdk-pcap";
