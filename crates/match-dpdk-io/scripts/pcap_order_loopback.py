#!/usr/bin/env python3
"""pcap_order_loopback.py — 用 pcap vdev 回放验证 DPDK 用户态下单全链路。

1. 生成 in.pcap：HELLO + N 笔交替 B/S 订单（Eth/IPv4/UDP/9B 帧，带微秒时间戳）
2. 调用方以 DPDK_VDEV=net_pcap0,rx_pcap=in.pcap,tx_pcap=out.pcap 跑 dpdk_tap_order
3. 解析 out.pcap：HELLO_ACK / ORDER_ACK / REPORT，统计处理延迟（pcap 时间戳差）

用法: python3 pcap_order_loopback.py gen <orders> <out_pcap> [--delay-ms X]
      python3 pcap_order_loopback.py parse <pcap>
"""
import socket, struct, sys, time

SESSION = 0x4D4F4F52
T_HELLO, T_ORDER = 0x30, 0x01
T_HELLO_ACK, T_ORDER_ACK, T_REPORT = 0x31, 0x12, 0x10
SRC_MAC = bytes([0x02, 0xaa, 0xbb, 0xcc, 0xdd, 0x01])
DST_MAC = bytes([0x02, 0xdd, 0xcc, 0xbb, 0xaa, 0x01])
SRC_IP, DST_IP = bytes([10, 9, 0, 2]), bytes([10, 9, 0, 1])
SRC_PORT, DST_PORT = 55041, 55040

def checksum(data: bytes) -> int:
    if len(data) % 2:
        data += b"\x00"
    s = sum(struct.unpack("!%dH" % (len(data) // 2), data))
    while s >> 16:
        s = (s & 0xFFFF) + (s >> 16)
    return (~s) & 0xFFFF

def frame(payload: bytes) -> bytes:
    eth = DST_MAC + SRC_MAC + b"\x08\x00"
    udp_len = 8 + len(payload)
    ip_total = 20 + udp_len
    ip = b"\x45\x00" + struct.pack("!H", ip_total) + b"\x00\x00\x00\x00\x40\x11" + b"\x00\x00" + SRC_IP + DST_IP
    ip = ip[:10] + struct.pack("!H", checksum(ip)) + ip[12:]
    udp = struct.pack("!HHHH", SRC_PORT, DST_PORT, udp_len, 0)
    return eth + ip + udp + payload

def pcap_header():
    return struct.pack("<IHHiIII", 0xA1B2C3D4, 2, 4, 0, 0, 65535, 1)  # LINKTYPE_ETHERNET=1

def pcap_packet(ts_us, data: bytes):
    return struct.pack("<IIII", ts_us // 1000000, ts_us % 1000000, len(data), len(data)) + data

def gen(orders: int, path: str, delay_ms: float = 0.1, body_size: int = 0):
    pkts = []
    ts = 100_000_000  # 起始时间戳 100s（微秒）
    # HELLO
    hello = struct.pack("!IIB", SESSION, 0, T_HELLO) + struct.pack("!II", 7, 0)
    pkts.append((ts, frame(hello)))
    ts += int(delay_ms * 1000)
    for i in range(orders):
        side = "B" if i % 2 == 0 else "S"
        price = "100.00" if side == "B" else "99.99"
        body = f"{side}|btcusdt|{price}|1|o{i}".encode()
        if body_size and len(body) < body_size:
            # padding 追加为第 6 段（parse_order 只取前 5 段，忽略多余段）
            body = body + b"|" + b"x" * (body_size - len(body) - 1)
        pkt = struct.pack("!IIB", SESSION, i + 1, T_ORDER) + body
        pkts.append((ts, frame(pkt)))
        ts += int(delay_ms * 1000)
    with open(path, "wb") as f:
        f.write(pcap_header())
        for t, d in pkts:
            f.write(pcap_packet(t, d))
    print(f"[gen] {orders+1} frames -> {path} (body={len(body)}B)")

def parse(path: str):
    with open(path, "rb") as f:
        data = f.read()
    # 支持微秒(d4c3b2a1)与纳秒(4d3cb2a1)精度、大小端
    magic = data[:4]
    if magic == b"\xd4\xc3\xb2\xa1":
        endian, nano = "<", False
    elif magic == b"\x4d\x3c\xb2\xa1":
        endian, nano = "<", True
    elif magic == b"\xa1\xb2\xc3\xd4":
        endian, nano = ">", False
    elif magic == b"\xa1\xb2\x3c\x4d":
        endian, nano = ">", True
    else:
        raise AssertionError(f"bad pcap magic {magic.hex()}")
    fmt = endian + "IIII"
    off = 24
    frames = []
    while off + 16 <= len(data):
        ts_sec, ts_usec, incl, orig = struct.unpack_from(fmt, data, off)
        off += 16
        fr = data[off:off + incl]
        off += incl
        ts_us = ts_sec * 1_000_000 + (ts_usec // 1000 if nano else ts_usec)
        frames.append((ts_us, fr))
    if not frames:
        print("[parse] no frames")
        return
    # 按类型归类
    stats = {"hello_ack": 0, "order_ack": 0, "report": 0, "exec": 0, "other": 0}
    delays = []
    last_order_ts = None
    for ts, fr in frames:
        if len(fr) < 42 + 9:
            stats["other"] += 1
            continue
        pay = fr[42:]
        if struct.unpack("!I", pay[:4])[0] != SESSION:
            stats["other"] += 1
            continue
        mt = pay[8]
        if mt == T_HELLO_ACK:
            stats["hello_ack"] += 1
        elif mt == T_ORDER_ACK:
            stats["order_ack"] += 1
            last_order_ts = ts
        elif mt == T_REPORT:
            stats["report"] += 1
            if len(pay) > 10 and pay[9:10] == b"E":
                stats["exec"] += 1
            if last_order_ts is not None:
                delays.append(ts - last_order_ts)
        else:
            stats["other"] += 1
    print(f"[parse] {len(frames)} frames: {stats}")
    if delays:
        delays.sort()
        avg = sum(delays) / len(delays)
        p50 = delays[len(delays) // 2]
        p99 = delays[int(len(delays) * 0.99) - 1]
        print(f"[parse] report-delay avg={avg:.1f}us p50={p50:.1f}us p99={p99:.1f}us min={delays[0]:.1f}us n={len(delays)}")
    # 打印前 3 帧 payload 摘要
    for ts, fr in frames[:3]:
        print(f"[parse] frame ts={ts} len={len(fr)} pay={fr[42:42+24].hex()}")

if __name__ == "__main__":
    if sys.argv[1] == "gen":
        gen(
            int(sys.argv[2]),
            sys.argv[3],
            float(sys.argv[4]) if len(sys.argv) > 4 else 0.1,
            int(sys.argv[5]) if len(sys.argv) > 5 else 0,
        )
    elif sys.argv[1] == "parse":
        parse(sys.argv[2])
