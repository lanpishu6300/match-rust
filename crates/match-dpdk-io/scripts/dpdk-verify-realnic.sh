#!/usr/bin/env bash
# dpdk-verify-realnic.sh — run the MoldUDP64 × DPDK rx path against a REAL
# physical NIC (MLX5 ConnectX / Intel i40e / any DPDK-supported PCI device).
#
# Requires: Linux host, DPDK 23.11+ userspace libs + headers, root, and a NIC
# whose kernel driver can be unbound to vfio-pci. This script does NOT run on
# macOS (no VFIO / PCI passthrough in Docker Desktop) — run it on a bare-metal
# Linux box or a VM with PCI passthrough.
#
# Usage:  sudo DPDK_EAL_NATIVE=1 ./dpdk-verify-realnic.sh [pci-bdf]
#   pci-bdf defaults to the first device bound to vfio-pci.
#
# Flow:  1. environment checks (root, dpdk libs, vfio-pci)
#        2. bind the chosen NIC to vfio-pci (dangerous: down + unbind)
#        3. start mold_dpdk_rx on the real port and count frames
#        4. optional: publish MoldUDP64 from another host, verify seq continuity
#
# IMPORTANT: step 2 takes the interface away from the kernel — you lose SSH
# on that interface. Run over a console / second NIC.

set -euo pipefail

BIN_DIR="$(cd "$(dirname "$0")/.." && pwd)/target/release"
RX_BIN="$BIN_DIR/mold_dpdk_rx"

echo "=== 1. environment checks ==="
[ "$(id -u)" = "0" ] || { echo "FATAL: run as root (sudo)"; exit 1; }
command -v dpdk-devbind.py >/dev/null 2>&1 || \
  echo "WARN: dpdk-devbind.py not in PATH (look in /usr/share/dpdk or /usr/local/bin)"
[ -x "$RX_BIN" ] || { echo "FATAL: build first: cargo build --release -p match-dpdk-io (on Linux)"; exit 1; }
modprobe vfio-pci 2>/dev/null || echo "WARN: vfio-pci module missing (kernel must have VFIO + IOMMU)"

# Hugepages (recommended) — print state, don't fail hard so VMs with
# DPDK_EAL_NO_HUGE=1 can still proceed on malloc memory.
if [ -d /dev/hugepages ] && [ "$(cat /proc/sys/vm/nr_hugepages 2>/dev/null || echo 0)" -ge 512 ]; then
  echo "OK: hugepages configured ($(cat /proc/sys/vm/nr_hugepages) pages)"
else
  echo "WARN: fewer than 512 hugepages; either enable them or set DPDK_EAL_NO_HUGE=1 (malloc memory)"
fi

echo "=== 2. pick + bind NIC to vfio-pci ==="
BDF="${1:-}"
if [ -z "$BDF" ]; then
  # Find the first device already bound to vfio-pci (or currently not bound).
  BDF=$(ls /sys/bus/pci/drivers/vfio-pci/ 2>/dev/null | grep -E '^[0-9a-f]{4}:' | head -1 || true)
fi
if [ -z "$BDF" ]; then
  echo "ERROR: no vfio-pci bound device and no pci-bdf argument given."
  echo "       Bind one first: dpdk-devbind.py -b vfio-pci 0000:XX:00.0"
  echo "       Or pass the BDF as arg 1 (script will attempt the bind)."
  exit 1
fi
IFNAME=$(basename "$(readlink "/sys/bus/pci/devices/$BDF/net" 2>/dev/null || echo /dev/null)" || true)
echo "using PCI $BDF (kernel iface: ${IFNAME:-none})"
# If the NIC still has a kernel driver, take it down and bind vfio-pci.
if [ -n "$IFNAME" ] && [ -d "/sys/class/net/$IFNAME" ]; then
  echo "taking $IFNAME down and binding to vfio-pci..."
  ip link set "$IFNAME" down
  DRIVER=$(basename "$(readlink "/sys/bus/pci/devices/$BDF/driver" 2>/dev/null)" || true)
  if [ -n "$DRIVER" ] && [ "$DRIVER" != "vfio-pci" ]; then
    echo "$BDF" > "/sys/bus/pci/drivers/$DRIVER/unbind"
  fi
  if [ ! -d "/sys/bus/pci/devices/$BDF/driver" ] || \
     [ "$(basename "$(readlink "/sys/bus/pci/devices/$BDF/driver")")" != "vfio-pci" ]; then
    echo "$BDF" > /sys/bus/pci/drivers/vfio-pci/bind
  fi
fi

echo "=== 3. run rx on the real NIC ==="
echo "stream MoldUDP64 from a publisher to the NIC's L2/L3 address;"
echo "rx prints frames/messages + seq gaps. Ctrl-C to stop."
echo
# -w allows only this device; DPDK_EAL_EXTRA lets you add more flags.
# mold_dpdk_rx expects: <port> <pcap?> — see its --help / source for args.
# For real-NIC use, first arg is the DPDK port id (0) — no pcap file.
if [ "${1:-}" = "stream" ]; then
  # stream mode: rx from port 0 until killed; prints per-burst stats
  DPDK_EAL_NATIVE=1 \
  DPDK_EAL_EXTRA="${DPDK_EAL_EXTRA:--w,$BDF,--log-level=pmd:6}" \
  "$RX_BIN" --nic 0
else
  echo "dry-run mode: exports are set; run with first arg 'stream' to receive."
  echo "  sudo DPDK_EAL_NATIVE=1 ./dpdk-verify-realnic.sh stream $BDF"
fi
