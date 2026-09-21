#!/usr/bin/env python3
"""fault_inject.py — 在 pcap 回放输入中注入故障（丢帧/坏校验和），验证会话层容错。

用法:
  python3 fault_inject.py drop <in> <out> <frame_idx>     # 删除第 idx 帧（0-based，含 HELLO=0）
  python3 fault_inject.py corrupt <in> <out> <frame_idx>  # 把第 idx 帧 UDP payload 前 4B 改为 0xDEADBEEF
"""
import struct, sys

def read_pcap(path):
    with open(path, "rb") as f:
        gh = f.read(24)
        magic = gh[:4]
        us = magic in (b"\xd4\xc3\xb2\xa1", b"\x4d\x3c\xb2\xa1")
        frames = []
        while True:
            h = f.read(16)
            if not h:
                break
            ts_sec, ts_frac, incl, orig = struct.unpack("<IIII", h)
            data = f.read(incl)
            frames.append((ts_sec, ts_frac, incl, orig, data))
        return gh, us, frames

def write_pcap(path, gh, frames):
    with open(path, "wb") as f:
        f.write(gh)
        for ts_sec, ts_frac, incl, orig, data in frames:
            f.write(struct.pack("<IIII", ts_sec, ts_frac, len(data), len(data)))
            f.write(data)

def main():
    mode, inp, outp, idx = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
    gh, us, frames = read_pcap(inp)
    print(f"[fault] {len(frames)} frames, injecting {mode} at frame {idx}")
    if mode == "drop":
        assert 0 <= idx < len(frames)
        frames.pop(idx)
    elif mode == "corrupt":
        assert 0 <= idx < len(frames)
        ts, tf, incl, orig, data = frames[idx]
        # payload 前 4B = SESSION magic (UDP 头后), 帧 = eth14 + ip20 + udp8 + payload
        off = 14 + 20 + 8
        payload = bytearray(data)
        payload[off:off+4] = b"\xde\xad\xbe\xef"
        frames[idx] = (ts, tf, incl, orig, bytes(payload))
    write_pcap(outp, gh, frames)
    print(f"[fault] wrote {len(frames)} frames -> {outp}")

if __name__ == "__main__":
    main()
