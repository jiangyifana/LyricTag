#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""生成 LyricTag 应用图标（纯 stdlib：zlib + struct，不依赖 PIL）。

产出 icons/icon.ico（含 16/32/48/64/128/256 六种尺寸）、icons/icon.png、
以及 Tauri 打包所需的 icons/32x32.png、icons/128x128.png、
icons/128x128@2x.png、icons/icon.png。

图形：圆角方块 + 蓝紫渐变底 + 白色音符（圆头 + 符干 + 符尾）。
"""
import math
import os
import struct
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "src-tauri", "icons")

# 品牌渐变端点，取自交互原型的 --accent 与 --st-writing
C0 = (0x0F, 0x6C, 0xBD)
C1 = (0x84, 0x30, 0xCE)


def _rounded_alpha(x, y, size, radius):
    """圆角矩形的覆盖率（4x4 超采样抗锯齿），返回 0.0–1.0。"""
    hits = 0
    for sy in range(4):
        for sx in range(4):
            px = x + (sx + 0.5) / 4.0
            py = y + (sy + 0.5) / 4.0
            cx = min(max(px, radius), size - radius)
            cy = min(max(py, radius), size - radius)
            if (px - cx) ** 2 + (py - cy) ** 2 <= radius * radius:
                hits += 1
    return hits / 16.0


def _note_coverage(x, y, size):
    """八分音符的覆盖率。头部为实心圆，符干为矩形，符尾为一段弧。"""
    u = size / 32.0  # 以 32px 为设计基准等比缩放
    px, py = (x + 0.5) / u, (y + 0.5) / u

    # 符头：圆心 (11.5, 23)，半径 5.2
    if (px - 11.5) ** 2 + (py - 23.0) ** 2 <= 5.2 ** 2:
        return 1.0
    # 符干：从符头右侧向上到 y=6.5，宽 2.0
    if 14.6 <= px <= 16.6 and 6.5 <= py <= 23.0:
        return 1.0
    # 符尾：右上角一段四分之一圆弧（外半径 6.2 / 内半径 3.8）
    dx, dy = px - 10.4, py - 12.7
    r = math.hypot(dx, dy)
    if 3.8 <= r <= 6.2 and dx >= 0 and dy <= 0:
        return 1.0
    return 0.0


def render(size):
    """渲染一张 size×size 的 RGBA 图像，返回逐行像素字节。"""
    radius = size * 0.22
    rows = []
    for y in range(size):
        row = bytearray()
        for x in range(size):
            a = _rounded_alpha(x, y, size, radius)
            if a <= 0.0:
                row += b"\x00\x00\x00\x00"
                continue
            t = (x + y) / (2.0 * size)          # 左上 → 右下的对角渐变
            r = int(C0[0] + (C1[0] - C0[0]) * t)
            g = int(C0[1] + (C1[1] - C0[1]) * t)
            b = int(C0[2] + (C1[2] - C0[2]) * t)
            n = _note_coverage(x, y, size)
            if n > 0:
                r = int(r + (255 - r) * n)
                g = int(g + (255 - g) * n)
                b = int(b + (255 - b) * n)
            row += bytes((r, g, b, int(a * 255)))
        rows.append(bytes(row))
    return rows


def to_png(size):
    rows = render(size)
    raw = b"".join(b"\x00" + r for r in rows)   # 每行前置 filter type 0

    def chunk(tag, data):
        c = tag + data
        return struct.pack(">I", len(data)) + c + struct.pack(">I", zlib.crc32(c) & 0xFFFFFFFF)

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def to_ico(sizes):
    """ICO 容器，PNG 载荷（Windows Vista+ 支持）。"""
    entries, blobs, offset = [], [], 6 + 16 * len(sizes)
    for s in sizes:
        data = to_png(s)
        entries.append(struct.pack("<BBBBHHII", s if s < 256 else 0, s if s < 256 else 0,
                                   0, 0, 1, 32, len(data), offset))
        blobs.append(data)
        offset += len(data)
    return struct.pack("<HHH", 0, 1, len(sizes)) + b"".join(entries) + b"".join(blobs)


def main():
    os.makedirs(OUT, exist_ok=True)
    ico_sizes = [16, 32, 48, 64, 128, 256]
    with open(os.path.join(OUT, "icon.ico"), "wb") as f:
        f.write(to_ico(ico_sizes))
    for name, size in [("icon.png", 512), ("128x128.png", 128),
                       ("128x128@2x.png", 256), ("32x32.png", 32)]:
        with open(os.path.join(OUT, name), "wb") as f:
            f.write(to_png(size))
    print("icons written to", OUT)


if __name__ == "__main__":
    main()
