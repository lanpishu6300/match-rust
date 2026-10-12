#!/usr/bin/env python3
"""tap_udp_client — AF_PACKET 客户端：经 tap0 走 DPDK 用户态下单链路测往返。

用法: python3 scripts/tap_udp_client.py <tap_mac> [orders] [--ping N]
链路: AF_PACKET send → tap0 → DPDK PMD 收 → ServerSession+Engine → PMD 发 → tap0 → AF_PACKET recv
"""
import socket, struct, sys, time, statistics

TAP_MAC = sys.argv[1] if len(sys.argv) > 1 else None
ORDERS = int(sys.argv[2]) if len(sys.argv) > 2 else 1000
PING = True if "--ping" in sys.argv else False
IFACE = "tap0"
DST_PORT = 55040
SRC_PORT = 55041
SESSION = 0x4D4F4F52
T_HELLO, T_ORDER, T_NAK = 0x30, 0x01, 0x20
T_HELLO_ACK, T_ORDER_ACK, T_REPORT, T_NAK_RESP = 0x31, 0x12, 0x10, 0x21

# 源 mac 用 tap0 自身 mac：回包 dst=本机 mac，无需混杂模式
SRC_MAC = bytes.fromhex(TAP_MAC.replace(":", "")) if TAP_MAC else bytes([0x02, 0xaa, 0xbb, 0xcc, 0xdd, 0x01])
SRC_IP = bytes([10, 9, 0, 2])
DST_IP = bytes([10, 9, 0, 1])

def checksum(data: bytes) -> int:
    if len(data) % 2:
        data += b"\x00"
    s = sum(struct.unpack("!%dH" % (len(data) // 2), data))
    while s >> 16:
        s = (s & 0xFFFF) + (s >> 16)
    return (~s) & 0xFFFF

def frame(payload: bytes, src_port: int = SRC_PORT) -> bytes:
    """构造 Eth/IPv4/UDP/9B帧 包。"""
    eth = bytes.fromhex(TAP_MAC.replace(":", "")) + SRC_MAC + b"\x08\x00"
    udp_len = 8 + len(payload)
    ip_total = 20 + udp_len
    ip = b"\x45\x00" + struct.pack("!H", ip_total) + b"\x00\x00\x00\x00\x40\x11" + b"\x00\x00" + SRC_IP + DST_IP
    ip = ip[:10] + struct.pack("!H", checksum(ip)) + ip[12:]
    udp = struct.pack("!HHHH", src_port, DST_PORT, udp_len, 0)
    return eth + ip + udp + payload

def pkt_head(seq: int, mtype: int) -> bytes:
    return struct.pack("!IIB", SESSION, seq, mtype)

def main():
    if not TAP_MAC:
        print("usage: tap_udp_client.py <tap_mac> [orders]")
        sys.exit(1)
    sock = socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(0x0800))
    # bind 协议必须与创建一致（0 不匹配任何帧，收不到任何包）
    sock.bind((IFACE, socket.htons(0x0800)))
    # tap 驱动按 dst mac 过滤（回包 dst=本 client 假 mac 非 tap0 mac），
    # 需 tap0 混杂模式：ip link set tap0 promisc on
    sock.settimeout(0.5)

    # HELLO 登录（seq=0）
    hello = pkt_head(0, T_HELLO) + struct.pack("!II", 7, 0)
    sock.send(frame(hello))
    acked = False
    t0 = time.monotonic()
    while time.monotonic() - t0 < 2:
        try:
            data = sock.recv(2048)
        except socket.timeout:
            break
        if len(data) < 14 + 20 + 8 + 9:
            continue
        pay = data[14 + 20 + 8:]
        if pay[8] == T_HELLO_ACK and struct.unpack("!I", pay[:4])[0] == SESSION:
            acked = True
            break
    if not acked:
        print("HELLO_ACK timeout")
        sys.exit(1)
    print("HELLO_ACK ok")

    # 下单：交替 B/S 保证成交
    rtts = []
    reports = 0
    executed = 0
    for i in range(ORDERS):
        side = "B" if i % 2 == 0 else "S"
        price = "100.00" if side == "B" else "99.99"
        body = f"{side}|btcusdt|{price}|1|o{i}".encode()
        pkt = pkt_head(i + 1, T_ORDER) + body
        t = time.monotonic()
        sock.send(frame(pkt))
        # 等本笔回报（ORDER_ACK + REPORT）
        got_report = False
        deadline = time.monotonic() + 1.0
        while time.monotonic() < deadline:
            try:
                data = sock.recv(4096)
            except socket.timeout:
                continue
            if len(data) < 14 + 20 + 8 + 9:
                continue
            pay = data[14 + 20 + 8:]
            if len(pay) < 9 or struct.unpack("!I", pay[:4])[0] != SESSION:
                continue
            mt = pay[8]
            if mt == T_REPORT:
                rtts.append((time.monotonic() - t) * 1e6)
                reports += 1
                if pay[9:10] == b"E":
                    executed += 1
                got_report = True
                break
        if not got_report:
            print(f"REPORT timeout at order {i}")
            break
    if rtts:
        rtts.sort()
        avg = statistics.mean(rtts)
        p50 = rtts[len(rtts) // 2]
        p99 = rtts[int(len(rtts) * 0.99) - 1]
        elapsed = time.monotonic() - t0
        print(f"[tap-client] orders={reports} executed={executed} rate={reports/elapsed:.0f}/s "
              f"RTT avg={avg:.1f}us p50={p50:.1f}us p99={p99:.1f}us min={rtts[0]:.1f}us")

if __name__ == "__main__":
    main()
