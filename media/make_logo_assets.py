# ABOUTME: Reproducible generator for the app window icon and in-app splash.
# ABOUTME: Crops the source logo PNG and composites it onto a white rounded plate.

# Run: uv run --with pillow python media/make_logo_assets.py
# Source : media/Level Editing App Logo for Doombuilder Port.png (1920x1080 RGBA, black line-art on transparent)
# Outputs: crates/doombuilder-gui/assets/icon.png   (256x256, badge-only on white plate)
#          crates/doombuilder-gui/assets/splash.png (full lockup on white plate)
#          media/logo_full.png, media/logo_badge.png (tight crops for README)

import pathlib

from PIL import Image, ImageDraw

ROOT = pathlib.Path(__file__).resolve().parent.parent
SRC = ROOT / "media" / "Level Editing App Logo for Doombuilder Port.png"
ASSETS = ROOT / "crates" / "doombuilder-gui" / "assets"
ASSETS.mkdir(parents=True, exist_ok=True)

PLATE = (255, 255, 255, 255)  # white: keeps black line-art legible on any theme/dock


def content_bbox(img, region):
    """Tight bbox of opaque non-near-white pixels inside region (x0,y0,x1,y1)."""
    px = img.load()
    x0, y0, x1, y1 = region
    minx, miny, maxx, maxy = x1, y1, x0, y0
    for y in range(y0, y1):
        for x in range(x0, x1):
            r, g, b, a = px[x, y]
            if a > 30 and (r < 235 or g < 235 or b < 235):
                minx, maxx = min(minx, x), max(maxx, x)
                miny, maxy = min(miny, y), max(maxy, y)
    return (minx, miny, maxx + 1, maxy + 1)


def rounded_plate(size, radius):
    plate = Image.new("RGBA", size, (0, 0, 0, 0))
    mask = Image.new("L", size, 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, size[0] - 1, size[1] - 1], radius=radius, fill=255)
    solid = Image.new("RGBA", size, PLATE)
    plate.paste(solid, (0, 0), mask)
    return plate


def place_centered(plate, art, scale):
    pw, ph = plate.size
    target = int(min(pw, ph) * scale)
    aw, ah = art.size
    ratio = min(target / aw, target / ph if ah == 0 else target / ah)
    art = art.resize((max(1, int(aw * ratio)), max(1, int(ah * ratio))), Image.LANCZOS)
    out = plate.copy()
    out.alpha_composite(art, ((pw - art.size[0]) // 2, (ph - art.size[1]) // 2))
    return out


def main():
    src = Image.open(SRC).convert("RGBA")

    # The lockup is mark (top) over "DoomBuilder" / "LEVEL EDITOR".
    # Hexagon mark ends at y~575; wordmark cap tops begin at y~576, so the
    # badge scan region must stop at 575 to avoid bleeding in letter tops.
    badge_box = content_bbox(src, (770, 420, 1150, 575))
    full_box = content_bbox(src, (770, 420, 1150, 660))

    badge = src.crop(badge_box)
    full = src.crop(full_box)

    # README crops (tight, transparent background, no plate).
    badge.save(ROOT / "media" / "logo_badge.png")
    full.save(ROOT / "media" / "logo_full.png")

    # Window icon: 256x256, badge centered on a white squircle-ish plate.
    icon_plate = rounded_plate((256, 256), radius=56)
    icon = place_centered(icon_plate, badge, scale=0.66)
    icon.save(ASSETS / "icon.png")

    # In-app splash: full lockup on a wide white rounded plate, padded.
    fw, fh = full.size
    pad = int(fw * 0.12)
    splash_w = fw + 2 * pad
    splash_h = fh + 2 * pad
    splash_plate = rounded_plate((splash_w, splash_h), radius=int(splash_h * 0.16))
    splash = splash_plate.copy()
    splash.alpha_composite(full, (pad, pad))
    splash.save(ASSETS / "splash.png")

    print("badge_box", badge_box, "-> icon", icon.size)
    print("full_box", full_box, "-> splash", splash.size)


if __name__ == "__main__":
    main()
