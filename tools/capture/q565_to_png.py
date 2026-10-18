"""Decode a Q565 image (as captured from NZXT CAM) to PNG for a visual check.

Usage: q565_to_png.py IN.bin OUT.png
Follows the reference decoder from the q565 crate (seritools/q565).
"""

import struct
import sys
import zlib


def unpack(p):
    return (p >> 11) & 31, (p >> 5) & 63, p & 31


def pack(r, g, b):
    return ((r & 31) << 11) | ((g & 63) << 5) | (b & 31)


def add(p, dr, dg, db):
    r, g, b = unpack(p)
    return pack(r + dr, g + dg, b + db)


def h(p):
    return ((p & 0xFF) + (p >> 8)) & 63


def decode(data):
    if data[:4] != b"q565":
        raise ValueError("bad magic")
    w, hgt = struct.unpack_from("<HH", data, 4)
    out = []
    arr = [0] * 64
    prev = 0
    i = 8
    while True:
        b = data[i]
        i += 1
        op = b >> 6
        if op == 0:
            prev = arr[b]
            out.append(prev)
            continue
        if op == 1:
            prev = add(prev, ((b >> 4) & 3) - 2, ((b >> 2) & 3) - 2, (b & 3) - 2)
            out.append(prev)
            continue
        if op == 2:
            b2 = data[i]
            i += 1
            if b & 0x20 == 0:
                dg = (b & 31) - 16
                p = add(prev, (b2 >> 4) - 8 + dg, dg, (b2 & 15) - 8 + dg)
            else:
                p = add(arr[b2 & 63], (b & 3) - 2, ((b >> 2) & 7) - 4, (b2 >> 6) - 2)
        else:
            if b == 0xFF:
                break
            if b == 0xFE:
                p = data[i] | (data[i + 1] << 8)
                i += 2
            else:
                out.extend([prev] * ((b & 63) + 1))
                continue
        arr[h(p)] = p
        prev = p
        out.append(p)
    if len(out) != w * hgt:
        raise ValueError(f"decoded {len(out)} pixels, header says {w}x{hgt}")
    return w, hgt, out, i


def write_png(path, w, hgt, pixels):
    rows = bytearray()
    for y in range(hgt):
        rows.append(0)
        for p in pixels[y * w:(y + 1) * w]:
            r, g, b = unpack(p)
            rows += bytes(((r * 527 + 23) >> 6, (g * 259 + 33) >> 6, (b * 527 + 23) >> 6))

    def chunk(kind, body):
        c = kind + body
        return struct.pack(">I", len(body)) + c + struct.pack(">I", zlib.crc32(c))

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", w, hgt, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(rows), 6))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


data = open(sys.argv[1], "rb").read()
w, hgt, pixels, used = decode(data)
write_png(sys.argv[2], w, hgt, pixels)
print(f"{sys.argv[1]}: {w}x{hgt}, used {used} of {len(data)} bytes")
