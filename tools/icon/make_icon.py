"""Draw camless's icon and write gui/camless.ico and gui/icon.svg.

Usage: make_icon.py [OUT.ico] [--png DIR]
The mark is the logo in gui/ui/kit.js: a c and a minus, for cam-less. Here it is
white and orchid on a dark tile, so it reads on light and dark taskbars. At 24 px
and below the mark fills more of the tile, so its strokes stay two pixels wide.
Needs Pillow.
"""

import math
import os
import sys

from PIL import Image

SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]

# The mark in a 64-unit box, as in gui/ui/kit.js: an arc open to the right, and a bar.
ARC = ((25.0, 32.0), 15.0, 45.0, 315.0)  # centre, radius, from and to in degrees, clockwise
BAR = ((43.0, 32.0), (55.0, 32.0))
STROKE = 9.0
MARK_CENTRE = (32.5, 32.0)

TILE = (14.0, (0x12, 0x12, 0x14))  # corner radius, colour
C_COLOR = (0xFF, 0xFF, 0xFF)
BAR_COLOR = (0xBF, 0x4C, 0xBF)
SUB = 6  # subsamples per pixel side


def scale(size):
    return 0.92 if size <= 24 else 0.8


def segment_distance(p, a, b):
    (px, py), (ax, ay), (bx, by) = p, a, b
    dx, dy = bx - ax, by - ay
    t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
    return math.hypot(px - ax - t * dx, py - ay - t * dy)


def in_arc(u, v):
    (cx, cy), r, a0, a1 = ARC
    angle = math.degrees(math.atan2(v - cy, u - cx)) % 360
    if a0 <= angle <= a1:
        return abs(math.hypot(u - cx, v - cy) - r) <= STROKE / 2
    ends = [(cx + r * math.cos(math.radians(a)), cy + r * math.sin(math.radians(a))) for a in (a0, a1)]
    return min(math.hypot(u - x, v - y) for x, y in ends) <= STROKE / 2


def in_bar(u, v):
    return segment_distance((u, v), *BAR) <= STROKE / 2


def in_tile(x, y):
    r = TILE[0]
    qx, qy = max(abs(x - 32) - (32 - r), 0), max(abs(y - 32) - (32 - r), 0)
    return math.hypot(qx, qy) <= r


def draw(size):
    k = scale(size)
    unit = 64.0 / size
    img = Image.new("RGBA", (size, size))
    px = img.load()
    for y in range(size):
        for x in range(size):
            tile = c = bar = 0
            for j in range(SUB):
                for i in range(SUB):
                    tx, ty = (x + (i + 0.5) / SUB) * unit, (y + (j + 0.5) / SUB) * unit
                    if not in_tile(tx, ty):
                        continue
                    tile += 1
                    # Back from the tile to the mark's own box.
                    u, v = MARK_CENTRE[0] + (tx - 32) / k, MARK_CENTRE[1] + (ty - 32) / k
                    c += in_arc(u, v)
                    bar += in_bar(u, v)
            if tile == 0:
                px[x, y] = (0, 0, 0, 0)
                continue
            rgb = [
                (t * (tile - c - bar) + w * c + o * bar) / tile
                for t, w, o in zip(TILE[1], C_COLOR, BAR_COLOR)
            ]
            px[x, y] = (*(round(p) for p in rgb), round(255 * tile / SUB**2))
    return img


def svg():
    """The same drawing at the large-size scale, for the README."""
    k = scale(64)
    to_tile = lambda u, v: (round(32 + (u - MARK_CENTRE[0]) * k, 3), round(32 + (v - MARK_CENTRE[1]) * k, 3))
    (cx, cy), r, a0, a1 = ARC
    start = to_tile(cx + r * math.cos(math.radians(a0)), cy + r * math.sin(math.radians(a0)))
    end = to_tile(cx + r * math.cos(math.radians(a1)), cy + r * math.sin(math.radians(a1)))
    (b0, b1), rr, w = [to_tile(*p) for p in BAR], round(r * k, 3), round(STROKE * k, 3)
    hexc = lambda c: "#" + "".join(f"{p:02x}" for p in c)
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">'
        f'<rect width="64" height="64" rx="{TILE[0]:g}" fill="{hexc(TILE[1])}"/>'
        f'<path d="M{start[0]:g} {start[1]:g}A{rr:g} {rr:g} 0 1 1 {end[0]:g} {end[1]:g}" fill="none" '
        f'stroke="{hexc(C_COLOR)}" stroke-width="{w:g}" stroke-linecap="round"/>'
        f'<path d="M{b0[0]:g} {b0[1]:g}H{b1[0]:g}" stroke="{hexc(BAR_COLOR)}" stroke-width="{w:g}" '
        'stroke-linecap="round"/></svg>\n'
    )


def main():
    args = sys.argv[1:]
    png_dir = None
    if "--png" in args:
        i = args.index("--png")
        png_dir = args[i + 1]
        del args[i : i + 2]
    here = os.path.dirname(os.path.abspath(__file__))
    gui = os.path.join(here, "..", "..", "gui")
    out = args[0] if args else os.path.join(gui, "camless.ico")
    images = [draw(s) for s in SIZES]
    biggest = images[-1]
    biggest.save(out, format="ICO", sizes=[(s, s) for s in SIZES], append_images=images[:-1])
    with open(os.path.join(os.path.dirname(out), "icon.svg"), "w", newline="\n") as f:
        f.write(svg())
    if png_dir:
        os.makedirs(png_dir, exist_ok=True)
        for s, img in zip(SIZES, images):
            img.save(os.path.join(png_dir, f"camless-{s}.png"))
    print(f"wrote {os.path.normpath(out)}: {', '.join(map(str, SIZES))} px, and icon.svg beside it")


if __name__ == "__main__":
    main()
