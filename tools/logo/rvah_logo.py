#!/usr/bin/env python3
"""The RVAH logo: the letters as red lightning bolts.

Each letter is a few angular strokes; a long stroke runs full width to a
kink, steps sideways there like a bolt, and narrows to a sharp tip. The
strokes are polygons that overlap, all wound the same way, so a non-zero
fill joins them. Everything leans forward; the upper half of each letter
is lit.

    tools/logo/rvah_logo.py [--preview DIR]

writes the Android drawables (the wordmark, the TV banner and the app
icon); --preview also renders each as a PNG into DIR to look at.
"""

import argparse
import pathlib

RES = pathlib.Path(__file__).resolve().parents[2] / "android/app/src/main/res/drawable"

LEAN = 0.24  # how far the top of a letter sits right of its foot, per unit of height
GAP = 12  # between letters, in letter units (a letter is 100 high)
SPLIT = 50  # where the lit upper half ends, in letter units

BACKGROUND = "#0B0F14"
GLOW = ("#26FF1A1A", "#4DFF1A1A")  # an outer and an inner halo
DEEP = "#9E0010"
BRIGHT = "#E3121C"
HIGHLIGHT = "#FF4436"


def bolt(top_l, top_r, y0, foot, y1, kink, jog, shelf=5):
    """A stroke from a flat top [top_l, top_r] at y0 down to a tip at (foot, y1).

    Its full width runs down to height `kink`, where it steps `jog` to the
    right (negative: left), the right edge's shelf above the left edge's;
    from there it narrows to the tip, like a bolt.
    """
    half = (top_r - top_l) / 2

    def centre(y):
        mid = (top_l + top_r) / 2
        return mid + (foot - jog - mid) * (y - y0) / (y1 - y0)

    above, below = kink - shelf, kink + shelf
    return [
        (top_l, y0),
        (top_r, y0),
        (centre(above) + half, above),
        (centre(above) + half + jog, above),
        (foot, y1),
        (centre(below) - half + jog, below),
        (centre(below) - half, below),
    ]


def bar(x0, y0, x1, y1):
    return [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]


def letter_r():
    return 66, [
        bolt(0, 19, 0, 6, 100, kink=58, jog=5),
        # The bowl, with its counter cut in by running back along the inside.
        [(0, 0), (48, 0), (64, 15), (64, 33), (49, 52), (8, 52), (8, 36), (40, 36), (45, 30), (45, 19), (41, 16), (0, 16)],
        bolt(28, 48, 44, 66, 100, kink=73, jog=7),
    ]


def letter_v():
    return 66, [
        bolt(0, 20, 0, 34, 100, kink=50, jog=8),
        bolt(46, 66, 0, 36, 100, kink=44, jog=-6),
        # The arms meet in a solid point rather than a crack.
        [(24, 62), (46, 62), (35, 100)],
    ]


def letter_a():
    return 70, [
        bolt(24, 46, 0, 0, 100, kink=52, jog=-6),
        bolt(26, 46, 0, 70, 100, kink=56, jog=7),
        bar(12, 60, 58, 74),
    ]


def letter_h():
    return 66, [
        bolt(0, 19, 0, 4, 100, kink=54, jog=5),
        bolt(47, 66, 0, 52, 100, kink=46, jog=5),
        # The crossbar is a bolt lying down: it jogs down at its middle.
        [(8, 40), (34, 40), (34, 46), (58, 46), (58, 60), (30, 60), (30, 54), (8, 54)],
    ]


def letters(makers):
    """The letters set side by side, leaning, as polygons; and their width.

    The leftmost point is at x = 0, the top at y = 0 and the feet at y = 100.
    """
    polygons, x = [], 0
    for make in makers:
        width, parts = make()
        for part in parts:
            polygons.append([(x + px + (100 - py) * LEAN, py) for px, py in part])
        x += width + GAP
    left = min(px for poly in polygons for px, _ in poly)
    right = max(px for poly in polygons for px, _ in poly)
    return [[(px - left, py) for px, py in poly] for poly in polygons], right - left


def place(polygons, scale, dx, dy):
    return [[(px * scale + dx, py * scale + dy) for px, py in poly] for poly in polygons]


class Art:
    """A drawable: a viewport of layers, each written as vector XML or rendered for a preview."""

    def __init__(self, width_dp, height_dp, vw, vh):
        self.size_dp, self.viewport = (width_dp, height_dp), (vw, vh)
        self.layers = []

    def card(self, radius):
        self.layers.append(("card", BACKGROUND, radius))

    def word(self, polygons, scale, dx, dy):
        """Lettering from `letters`, scaled and moved."""
        placed = place(polygons, scale, dx, dy)
        self.layers.append(("glow", GLOW[0], placed, 9 * scale))
        self.layers.append(("glow", GLOW[1], placed, 4.5 * scale))
        self.layers.append(("fill", DEEP, place(polygons, scale, dx + 1.6 * scale, dy + 1.6 * scale), None))
        self.layers.append(("fill", BRIGHT, placed, None))
        self.layers.append(("fill", HIGHLIGHT, placed, (dy, dy + SPLIT * scale)))

    def xml(self):
        vw, vh = self.viewport
        out = []
        for layer in self.layers:
            kind, colour = layer[0], layer[1]
            if kind == "card":
                r = layer[2]
                d = (f"M{r},0 H{vw - r} A{r},{r} 0 0 1 {vw},{r} V{vh - r} A{r},{r} 0 0 1 {vw - r},{vh} "
                     f"H{r} A{r},{r} 0 0 1 0,{vh - r} V{r} A{r},{r} 0 0 1 {r},0 Z")
                out.append(path(colour, d))
            elif kind == "glow":
                out.append(path(colour, path_data(layer[2]), stroke=layer[3]))
            elif layer[3] is None:
                out.append(path(colour, path_data(layer[2])))
            else:
                top, bottom = layer[3]
                out.append(
                    "    <group>\n"
                    f'        <clip-path android:pathData="M0,{top:.2f} H{vw} V{bottom:.2f} H0 Z" />\n'
                    + path(colour, path_data(layer[2]), indent=8)
                    + "    </group>\n"
                )
        w, h = self.size_dp
        return (
            '<?xml version="1.0" encoding="utf-8"?>\n'
            "<!-- Generated by tools/logo/rvah_logo.py: edit that and run it again. -->\n"
            '<vector xmlns:android="http://schemas.android.com/apk/res/android"\n'
            f'    android:width="{w}dp"\n    android:height="{h}dp"\n'
            f'    android:viewportWidth="{vw:g}"\n    android:viewportHeight="{vh:g}">\n'
            + "".join(out)
            + "</vector>\n"
        )

    def render(self, pixels_high):
        from PIL import Image, ImageChops, ImageDraw

        vw, vh = self.viewport
        ss = 4
        k = pixels_high * ss / vh
        size = (round(vw * k), round(vh * k))
        img = Image.new("RGBA", size, (0, 0, 0, 0))

        def mask(polygons):
            m = Image.new("L", size, 0)
            for poly in polygons:
                layer = Image.new("L", size, 0)
                ImageDraw.Draw(layer).polygon([(x * k, y * k) for x, y in poly], fill=255)
                m = ImageChops.lighter(m, layer)
            return m

        for layer in self.layers:
            kind, colour = layer[0], rgba(layer[1])
            solid = Image.new("RGBA", size, colour[:3] + (255,))
            if kind == "card":
                m = Image.new("L", size, 0)
                ImageDraw.Draw(m).rounded_rectangle([0, 0, size[0] - 1, size[1] - 1], radius=layer[2] * k, fill=255)
            elif kind == "glow":
                m = mask(layer[2])
                draw = ImageDraw.Draw(m)
                for poly in layer[2]:
                    draw.line([(x * k, y * k) for x, y in poly + poly[:1]], fill=255, width=round(layer[3] * k), joint="curve")
            else:
                m = mask(layer[2])
                if layer[3] is not None:
                    band = Image.new("L", size, 0)
                    ImageDraw.Draw(band).rectangle([0, layer[3][0] * k, size[0], layer[3][1] * k], fill=255)
                    m = ImageChops.multiply(m, band)
            m = m.point(lambda v: v * colour[3] // 255)
            img.alpha_composite(Image.composite(solid, Image.new("RGBA", size, (0, 0, 0, 0)), m))
        return img.resize((size[0] // ss, size[1] // ss), Image.LANCZOS)


def rgba(colour):
    hex_digits = colour.lstrip("#")
    alpha = int(hex_digits[:2], 16) if len(hex_digits) == 8 else 255
    rgb = hex_digits[-6:]
    return tuple(int(rgb[i:i + 2], 16) for i in (0, 2, 4)) + (alpha,)


def path_data(polygons):
    return " ".join("M" + " L".join(f"{x:.2f},{y:.2f}" for x, y in poly) + " Z" for poly in polygons)


def path(colour, d, stroke=None, indent=4):
    pad = " " * indent
    attrs = [f'android:fillColor="{colour}"']
    if stroke:
        attrs += [f'android:strokeColor="{colour}"', f'android:strokeWidth="{stroke:.2f}"', 'android:strokeLineJoin="round"']
    attrs.append(f'android:pathData="{d}"')
    return f"{pad}<path\n" + "".join(f"{pad}    {a}\n" for a in attrs[:-1]) + f"{pad}    {attrs[-1]} />\n"


ALL = (letter_r, letter_v, letter_a, letter_h)


def drawables():
    word, word_w = letters(ALL)

    # The wordmark alone, for the app's top bar, with room round it for the glow.
    pad = 4
    vw, vh = round(word_w + 2 * pad + 2), 100 + 2 * pad + 2
    mark = Art(round(vw * 24 / vh), 24, vw, vh)
    mark.word(word, 1, pad, pad)
    yield "rvah_wordmark.xml", mark

    # The TV launcher's banner, 320 × 180: the wordmark across the middle.
    banner = Art(320, 180, 320, 180)
    banner.card(8)
    scale = 264 / word_w
    banner.word(word, scale, (320 - 264) / 2, (180 - 100 * scale) / 2)
    yield "tv_banner.xml", banner

    # The app icon: R V over A H.
    icon = Art(108, 108, 108, 108)
    icon.card(18)
    rows = [letters(ALL[:2]), letters(ALL[2:])]
    scale = min(80 / max(w for _, w in rows), 0.4)
    for row, (polygons, width) in enumerate(rows):
        icon.word(polygons, scale, (108 - width * scale) / 2, 12 + row * (100 * scale + 4))
    yield "ic_launcher.xml", icon


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--preview", metavar="DIR")
    args = parser.parse_args()
    for name, art in drawables():
        (RES / name).write_text(art.xml())
        print("wrote", RES / name)
        if args.preview:
            out = pathlib.Path(args.preview)
            out.mkdir(parents=True, exist_ok=True)
            art.render(540 if name == "tv_banner.xml" else 216).save(out / name.replace(".xml", ".png"))


if __name__ == "__main__":
    main()
