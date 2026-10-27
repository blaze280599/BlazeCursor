"""Generates assets/blazecursor.ico (run once; the .ico is committed with the project)."""
from pathlib import Path
from PIL import Image, ImageDraw, ImageFilter

S = 1024  # supersampled canvas
img = Image.new("RGBA", (S, S), (0, 0, 0, 0))

# Flame-coloured disc with a vertical orange -> red gradient.
grad = Image.new("RGBA", (S, S))
gd = ImageDraw.Draw(grad)
top, bottom = (255, 176, 32), (232, 52, 28)
for y in range(S):
    t = y / (S - 1)
    gd.line([(0, y), (S, y)], fill=tuple(round(a + (b - a) * t) for a, b in zip(top, bottom)) + (255,))
mask = Image.new("L", (S, S), 0)
ImageDraw.Draw(mask).ellipse([24, 24, S - 24, S - 24], fill=255)
img.paste(grad, (0, 0), mask)

# Classic pointer arrow (12x21 grid), white with a dark outline and soft shadow.
arrow = [(0, 0), (0, 17), (4, 13), (7, 20), (9.4, 19), (6.6, 12.2), (12, 12.2)]
k, ox, oy = 30, 340, 205
pts = [(ox + x * k, oy + y * k) for x, y in arrow]

shadow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
ImageDraw.Draw(shadow).polygon([(x + 22, y + 26) for x, y in pts], fill=(90, 10, 0, 140))
img.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(18)))

d = ImageDraw.Draw(img)
d.polygon(pts, fill=(255, 255, 255, 255), outline=(40, 20, 20, 255), width=34)
d.polygon(pts, fill=(255, 255, 255, 255))  # redraw fill so the outline sits outside-ish

out = Path(__file__).with_name("blazecursor.ico")
img.resize((256, 256), Image.LANCZOS).save(
    out, sizes=[(16, 16), (20, 20), (24, 24), (32, 32), (40, 40), (48, 48), (64, 64), (256, 256)]
)
print("wrote", out)
