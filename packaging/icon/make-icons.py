# SPDX-License-Identifier: AGPL-3.0-or-later
# Regenerates locus-1024.png and locus.ico from the same geometry as locus.svg
# (packaging #244-#249). Only needed when the icon changes; outputs are committed.
# Usage: python packaging/icon/make-icons.py   (needs Pillow)
from pathlib import Path
from PIL import Image, ImageDraw

S = 4096  # draw large, then downsample for anti-aliasing
BG, FG = (31, 58, 95, 255), (255, 255, 255, 255)

img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw.Draw(img)
d.rounded_rectangle((0, 0, S - 1, S - 1), radius=int(S * 0.22), fill=BG)
c, r, w = S // 2, int(S * 0.27), int(S * 0.045)
d.ellipse((c - r, c - r, c + r, c + r), outline=FG, width=w)
d.ellipse((c - int(S * 0.07),) * 2 + (c + int(S * 0.07),) * 2, fill=FG)
for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):  # crosshair ticks
    a, b = int(S * 0.20), int(S * 0.38)
    x0, x1 = c + dx * a, c + dx * b
    y0, y1 = c + dy * a, c + dy * b
    d.line((x0, y0, x1, y1), fill=FG, width=w)

out = Path(__file__).parent
big = img.resize((1024, 1024), Image.LANCZOS)
big.save(out / "locus-1024.png", optimize=True)
big.save(out / "locus.ico", sizes=[(s, s) for s in (16, 24, 32, 48, 64, 128, 256)])
