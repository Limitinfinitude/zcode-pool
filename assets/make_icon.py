#!/usr/bin/env python3
"""生成 Z·POOL 的图标源图 icon-src.png。

自研标记：iOS 圆角方形（squircle）蓝渐变底，中央白色「池」状堆叠三横条
（上短下长，像账号/邮件越攒越多）。

跑完再用 `npx tauri icon assets/icon-src.png` 由它生成 src-tauri/icons/ 整套图标。
依赖：Pillow。
"""
from pathlib import Path
from PIL import Image, ImageDraw

SIZE = 1024
SS = 4  # 超采样，缩小后抗锯齿
N = SIZE * SS


def gradient(n, top, bottom):
    g = Image.new("RGB", (1, n))
    px = g.load()
    for y in range(n):
        t = y / (n - 1)
        px[0, y] = tuple(round(top[i] + (bottom[i] - top[i]) * t) for i in range(3))
    return g.resize((n, n))


def main():
    bg = gradient(N, (63, 160, 255), (10, 108, 255)).convert("RGBA")
    mask = Image.new("L", (N, N), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, N - 1, N - 1], radius=int(N * 0.2237), fill=255)
    bg.putalpha(mask)

    fg = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    d = ImageDraw.Draw(fg)
    white = (255, 255, 255, 255)
    bh = int(N * 0.078)
    gap = int(N * 0.075)
    mid = N // 2
    for y, w in [(mid - (bh + gap), 0.30), (mid, 0.44), (mid + (bh + gap), 0.58)]:
        width = int(N * w)
        x0 = (N - width) // 2
        d.rounded_rectangle([x0, y - bh // 2, x0 + width, y + bh // 2], radius=bh // 2, fill=white)

    img = Image.alpha_composite(bg, fg).resize((SIZE, SIZE), Image.LANCZOS)
    img.save(Path(__file__).parent / "icon-src.png")
    print("wrote icon-src.png")


if __name__ == "__main__":
    main()
