#!/usr/bin/env python3
"""Prepare offline assets for the DPDK verify Docker image.

Because the container (BuildKit) network is unreliable for large transfers,
we download everything on the host (fast: ~2 MB/s to Tsinghua mirror) and
let the Dockerfile install purely offline:

  * apt .debs (dpdk pcap runtime closure + gcc/libc6-dev/pkg-config/curl/ca)
  * DPDK headers (extracted from libdpdk-dev, for reference / FFI authoring)
  * Rust 1.97.1 aarch64-unknown-linux-gnu toolchain (for the image)

Usage: prepare-offline-assets.py <out_dir>
Writes: <out_dir>/debs/*.deb, <out_dir>/dpdk-headers/, <out_dir>/rust/...
"""

import gzip
import os
import re
import subprocess
import sys
import urllib.request
from functools import cmp_to_key

MIRROR = "http://mirrors.tuna.tsinghua.edu.cn/ubuntu-ports"
SUITES = ["noble", "noble-updates", "noble-backports", "noble-security"]
COMPS = ["main", "universe"]
ARCH = "arm64"
ROOTS = [
    "librte-net-pcap24",
    "librte-mempool-ring24",  # "ring_mp_mc" ops plugin — rte_mbuf_best_mempool_ops() resolves to it
    "gcc",
    "libc6-dev",
    "pkg-config",
    "curl",
    "ca-certificates",
    "libpcap-dev",  # provides libpcap.so link symlink for the DPDK link step
    "libnuma-dev",  # provides libnuma.so link symlink
    "libbsd-dev",   # bsd/string.h, pulled in by DPDK trace-point headers via rte_ethdev.h
]
OUT = os.path.abspath(sys.argv[1]) if len(sys.argv) > 1 else os.path.abspath(".")
DEBS = os.path.join(OUT, "debs")
HDRS = os.path.join(OUT, "dpdk-headers")
DEVDIR = os.path.join(OUT, "dpdk-dev")  # libdpdk-dev deb (headers only, not installed)


def http(url: str) -> bytes:
    req = urllib.request.Request(url, headers={"User-Agent": "curl/8.5.0"})
    with urllib.request.urlopen(req, timeout=300) as r:
        return r.read()


def fetch_index(suite: str, comp: str) -> bytes:
    url = f"{MIRROR}/dists/{suite}/{comp}/binary-{ARCH}/Packages.gz"
    print(f"  index {suite}/{comp}", flush=True)
    return gzip.decompress(http(url))


def parse_stanzas(data: bytes):
    text = data.decode("utf-8", "replace")
    cur = {}
    for line in text.splitlines():
        if not line.strip():
            if cur:
                yield cur
                cur = {}
            continue
        if line.startswith(" "):
            continue  # continuation; ignore (no wrapped fields we need)
        if ":" in line:
            k, v = line.split(":", 1)
            cur[k.strip()] = v.strip()
    if cur:
        yield cur


def parse_depends(s: str):
    """Return dependency package names (first alternative only, :arch stripped)."""
    out = []
    for group in s.split(","):
        group = group.strip()
        if not group:
            continue
        name = group.split("|")[0].split("(")[0].strip()
        name = name.split(":")[0]
        if name:
            out.append(name)
    return out


def deb_version_cmp(a: str, b: str) -> int:
    """Debian version ordering: epoch:upstream[-revision], where '~' < empty
    < letters < digits, and digit runs compare numerically. Returns <0, 0, >0."""

    def cmp_part(a: str, b: str) -> int:
        i = j = 0
        while True:
            ca = a[i] if i < len(a) else None
            cb = b[j] if j < len(b) else None
            if ca is None and cb is None:
                return 0
            if ca == "~" or cb == "~":
                if ca == "~" and cb != "~":
                    return -1
                if cb == "~" and ca != "~":
                    return 1
                i += 1
                j += 1
                continue
            if ca is None:
                return -1  # a ended first (no '~' pending) -> a smaller
            if cb is None:
                return 1
            da = ca.isdigit()
            db = cb.isdigit()
            if da and db:
                si, sj = i, j
                while i < len(a) and a[i].isdigit():
                    i += 1
                while j < len(b) and b[j].isdigit():
                    j += 1
                na, nb = a[si:i].lstrip("0") or "0", b[sj:j].lstrip("0") or "0"
                if len(na) != len(nb):
                    return -1 if len(na) < len(nb) else 1
                if na != nb:
                    return -1 if na < nb else 1
                continue
            if da and not db:
                return -1
            if db and not da:
                return 1
            ca2, cb2 = ca, cb
            if ca2 != cb2:
                return -1 if ca2 < cb2 else 1
            i += 1
            j += 1

    ea = a.split(":", 1)[0] if ":" in a else "0"
    eb = b.split(":", 1)[0] if ":" in b else "0"
    if ea != eb:
        return -1 if int(ea) < int(eb) else 1
    ra = a.split(":", 1)[1] if ":" in a else a
    rb = b.split(":", 1)[1] if ":" in b else b
    ua, va = (ra.split("-", 1) + [""])[:2]
    ub, vb = (rb.split("-", 1) + [""])[:2]
    r = cmp_part(ua, ub)
    if r:
        return r
    return cmp_part(va, vb)


def main():
    os.makedirs(DEBS, exist_ok=True)
    os.makedirs(HDRS, exist_ok=True)

    print("== 1/5 fetch indexes ==", flush=True)
    pkgs: dict[str, dict] = {}
    for suite in SUITES:
        for comp in COMPS:
            try:
                data = fetch_index(suite, comp)
            except Exception as e:
                print(f"  !! {suite}/{comp} failed: {e}", flush=True)
                continue
            for st in parse_stanzas(data):
                name = st.get("Package")
                if not name:
                    continue
                cur = pkgs.setdefault(name, {})
                v = st.get("Version", "")
                if v not in cur or deb_version_cmp(v, cur[v]["version"]) > 0:
                    cur[v] = {
                        "version": v,
                        "filename": st.get("Filename", ""),
                        "depends": parse_depends(st.get("Depends", "")),
                    }
    print(f"  {len(pkgs)} packages indexed", flush=True)

    print("== 2/5 base image installed set ==", flush=True)
    installed = set()
    try:
        r = subprocess.run(
            ["docker", "run", "--rm", "ubuntu:24.04", "dpkg-query", "-W", "-f=${Package}\\n"],
            capture_output=True, text=True, timeout=120,
        )
        installed = {l.strip() for l in r.stdout.splitlines() if l.strip()}
        print(f"  base has {len(installed)} packages", flush=True)
    except Exception as e:
        print(f"  !! could not query base image: {e}", flush=True)

    print("== 3/5 resolve dependency closure ==", flush=True)
    chosen: dict[str, dict] = {}
    todo = list(ROOTS)
    missing = []
    while todo:
        name = todo.pop()
        if name in chosen:
            continue
        entry = pkgs.get(name)
        if not entry:
            missing.append(name)
            continue
        best_v = max(entry, key=cmp_to_key(deb_version_cmp))
        best = entry[best_v]
        chosen[name] = best
        todo.extend(d for d in best["depends"] if d not in chosen)
    # NOTE: base-image packages are intentionally NOT pruned here. The base
    # image ships old gcc-14 runtimes (libgcc-s1, libstdc++6, libasan8, ...)
    # pinned to gcc-14-base (= old). Installing a newer gcc-14-base from the
    # closure without upgrading those siblings breaks dpkg resolution, so the
    # full dependency closure (including updated base siblings) is installed.
    print(f"  closure: {len(chosen)} packages to download", flush=True)
    if missing:
        print(f"  WARN missing packages: {sorted(set(missing))}", flush=True)

    total = 0
    print("== 4/5 download debs ==", flush=True)
    for name in sorted(chosen):
        entry = chosen[name]
        url = f"{MIRROR}/{entry['filename']}"
        dest = os.path.join(DEBS, f"{name}.deb")
        if os.path.exists(dest):
            continue
        try:
            data = http(url)
        except Exception as e:
            print(f"  !! {name} failed: {e}", flush=True)
            continue
        with open(dest, "wb") as f:
            f.write(data)
        total += len(data)
        print(f"  {name} {entry['version']} {len(data)//1024}kB", flush=True)
    print(f"  total {total // (1024*1024)} MB", flush=True)

    print("== 5/5 extract dpdk headers (from libdpdk-dev) ==", flush=True)
    os.makedirs(DEVDIR, exist_ok=True)
    hdr_deb = os.path.join(DEVDIR, "libdpdk-dev.deb")
    if not os.path.exists(hdr_deb):
        # fetch the dev package for its headers only
        try:
            data = http(f"{MIRROR}/pool/universe/d/dpdk/libdpdk-dev_23.11-1build3_arm64.deb")
            with open(hdr_deb, "wb") as f:
                f.write(data)
        except Exception as e:
            print(f"  !! libdpdk-dev fetch failed: {e}", flush=True)
    if os.path.exists(hdr_deb):
        import tarfile
        import tempfile
        with tempfile.TemporaryDirectory() as td:
            subprocess.run(["ar", "-x", hdr_deb], cwd=td, check=True)
            data_tar = os.path.join(td, "data.tar.zst")
            if os.path.exists(data_tar):
                plain = os.path.join(td, "data.tar")
                subprocess.run(["zstd", "-d", "-f", data_tar, "-o", plain], check=True)
                with tarfile.open(plain) as tf:
                    for m in tf.getmembers():
                        if m.name.startswith("./usr/include/"):
                            tf.extract(m, HDRS, set_attrs=False)
        print(f"  headers -> {HDRS}", flush=True)
    else:
        print("  !! no libdpdk-dev.deb, headers skipped", flush=True)

    print("== done ==", flush=True)


if __name__ == "__main__":
    main()
