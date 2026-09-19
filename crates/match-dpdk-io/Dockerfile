# MoldUDP64 × DPDK local verification image — FULLY OFFLINE build.
#
# The BuildKit container network on this machine is unreliable for large
# transfers (apt index / deb downloads time out), so every network artifact
# is pre-downloaded on the host by scripts/prepare-offline-assets.py into
# ./offline-assets/ (debs + rust toolchain). Nothing in this Dockerfile
# touches the network except `docker build` itself.
#
# Uses DPDK's net_pcap virtual PMD — no physical NIC, no hugepages, no PCI —
# so the full rx/tx path runs safely inside a container.

FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive

# --- offline apt: pcap PMD runtime closure + gcc/libc6-dev/pkg-config/curl/ca ---
COPY crates/match-dpdk-io/offline-assets/debs/ /debs/
RUN apt-get install -y --no-install-recommends /debs/*.deb \
    && for f in /usr/lib/aarch64-linux-gnu/librte_*.so.*; do \
         ln -sf "$f" "${f%.so.*}.so"; \
       done \
    && rm -rf /debs /var/lib/apt/lists/*

# --- offline Rust 1.97.1 (aarch64-unknown-linux-gnu, matches rust-toolchain.toml) ---
COPY crates/match-dpdk-io/offline-assets/rust/rust-1.97.1-aarch64-unknown-linux-gnu.tar.gz /tmp/rust.tar.gz
RUN mkdir -p /opt/rust \
    && tar -xzf /tmp/rust.tar.gz -C /opt/rust --strip-components=1 \
    && cp -a /opt/rust/rust-std-aarch64-unknown-linux-gnu/lib/rustlib/. /opt/rust/rustc/lib/rustlib/ \
    && rm -rf /opt/rust/rust-std-* /opt/rust/rust-analysis-* /opt/rust/llvm-* \
    && rm -f /tmp/rust.tar.gz \
    && /opt/rust/cargo/bin/cargo --version \
    && /opt/rust/rustc/bin/rustc --version \
    && echo 'fn main(){println!("rustc-ok")}' | /opt/rust/rustc/bin/rustc -o /tmp/hello - && /tmp/hello

ENV PATH=/opt/rust/cargo/bin:/opt/rust/rustc/bin:$PATH \
    RUSTC=/opt/rust/rustc/bin/rustc \
    CARGO_HOME=/opt/cargo \
    RUSTUP_HOME=/opt/rustup

# --- DPDK headers (from libdpdk-dev; needed to compile src/dpdk/bridge.c) ---
COPY crates/match-dpdk-io/offline-assets/dpdk-headers/usr/include/ /usr/include/

# --- build the dpdk bins (zero external crates: FFI is hand-written) ---
WORKDIR /app
COPY . .
RUN ls crates/match-dpdk-io/src/bin/ \
    && rm -rf offline-assets \
    && cargo build --release -p match-dpdk-io \
    && ls -lh target/release/mold_*
