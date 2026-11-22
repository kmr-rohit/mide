"""Regenerate assets/macos/mide.icns (needs Pillow; macOS for iconutil).

    python3 scripts/make_icon.py
"""
import os, subprocess, tempfile
from PIL import Image, ImageDraw, ImageFont

SIZE = 1024
OUT = os.path.join(os.path.dirname(__file__), "..", "assets", "macos")

def render():
    img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    # macOS icon grid: the artwork sits in an 824px square, centered.
    inset, radius = 100, 185
    grad = Image.new("RGBA", (SIZE, SIZE))
    gd = ImageDraw.Draw(grad)
    for y in range(SIZE):
        t = y / SIZE
        c = tuple(int(a + (b - a) * t) for a, b in zip((0x3d, 0x49, 0x54), (0x22, 0x2a, 0x31)))
        gd.line([(0, y), (SIZE, y)], fill=c + (255,))
    mask = Image.new("L", (SIZE, SIZE), 0)
    ImageDraw.Draw(mask).rounded_rectangle([inset, inset, SIZE - inset, SIZE - inset], radius, fill=255)
    img.paste(grad, (0, 0), mask)

    d = ImageDraw.Draw(img)
    font = ImageFont.truetype("/System/Library/Fonts/Menlo.ttc", 560, index=1)
    text = "m"
    l, t, r, b = d.textbbox((0, 0), text, font=font)
    w, h = r - l, b - t
    x = (SIZE - w) / 2 - l - 40
    y = (SIZE - h) / 2 - t + 10
    d.text((x, y), text, font=font, fill=(0x5f, 0xb3, 0xb3, 255))
    # A text-cursor bar after the letter: it's an editor.
    cx = x + l + w + 38
    d.rounded_rectangle([cx, y + t + 40, cx + 46, y + t + h + 20], 10, fill=(0xfa, 0xc8, 0x63, 255))
    return img

def main():
    img = render()
    with tempfile.TemporaryDirectory() as tmp:
        iconset = os.path.join(tmp, "mide.iconset")
        os.mkdir(iconset)
        for base in (16, 32, 128, 256, 512):
            img.resize((base, base), Image.LANCZOS).save(os.path.join(iconset, f"icon_{base}x{base}.png"))
            img.resize((base * 2, base * 2), Image.LANCZOS).save(os.path.join(iconset, f"icon_{base}x{base}@2x.png"))
        subprocess.run(["iconutil", "-c", "icns", iconset, "-o", os.path.join(OUT, "mide.icns")], check=True)
    img.save(os.path.join(OUT, "mide-1024.png"))

main()
