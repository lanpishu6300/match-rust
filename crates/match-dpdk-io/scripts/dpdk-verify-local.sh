#!/bin/bash
# 本地 Docker 验证 MoldUDP64 × DPDK（pcap PMD，无物理网卡）全链路。
# 完全离线构建：先跑 scripts/prepare-offline-assets.py 准备 offline-assets/，
# 再 docker build（容器内零网络）。
# 用法：bash crates/match-dpdk-io/scripts/dpdk-verify-local.sh [--no-build]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT"

IMAGE=mold-dpdk-verify

if [[ "${1:-}" != "--no-build" ]]; then
    echo "==> docker build ($IMAGE) — 离线资产在 crates/match-dpdk-io/offline-assets"
    docker build -f crates/match-dpdk-io/Dockerfile -t "$IMAGE" .
fi

echo "==> docker run 验证（重复跑，快速）"
docker run --rm --cap-add=NET_RAW --cap-add=NET_ADMIN \
    -v "$ROOT"/crates/match-dpdk-io/scripts:/verify \
    -w /verify "$IMAGE" bash -c '
set -e
mkdir -p /data
echo "--- 生成 input.pcap (seq 1,2,3 + 心跳@4 + seq 5,6 gap@4) ---"
/app/target/release/mold_dpdk_gen_pcap /data/input.pcap
echo "--- DPDK rx: input.pcap (期望 5 帧/5 消息/seqs=[1,2,3,5,6]/gap=(4,1)/last=6) ---"
/app/target/release/mold_dpdk_rx /data/input.pcap input
echo "--- DPDK tx: 写 output.pcap (2 数据 + 1 心跳) ---"
/app/target/release/mold_dpdk_tx /data/output.pcap
echo "--- DPDK rx: output.pcap (期望 3 帧/3 消息/seqs=[1,2,3]/无 gap) ---"
/app/target/release/mold_dpdk_rx /data/output.pcap output
echo "DPDK_VERIFY_ALL: PASS"
'
