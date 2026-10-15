#!/usr/bin/env python3
"""Builds the embedded app icons from the concept image.

    python3 -I crates/addict-app/icons/make-icons.py \
        assets/icon-concepts/gaudi/mosaic-a.png crates/addict-app/icons

Crops the source to its alpha box, centres it on a square with a margin and
writes one PNG per size. Each PNG carries the tEXt chunk `X-Addict` = `true`,
the marker `addict install` looks for before it overwrites or removes a file.
"""
import sys

from PIL import Image
from PIL.PngImagePlugin import PngInfo

SIZES = (64, 128, 256, 512)
MARGIN = 1.1  # the square is 10% wider than the content: 5% on each side

source, out = sys.argv[1], sys.argv[2]
image = Image.open(source).convert("RGBA")
image = image.crop(image.getchannel("A").getbbox())
side = round(max(image.size) * MARGIN)
square = Image.new("RGBA", (side, side), (0, 0, 0, 0))
square.paste(image, ((side - image.width) // 2, (side - image.height) // 2))
marker = PngInfo()
marker.add_text("X-Addict", "true")
for size in SIZES:
    square.resize((size, size), Image.LANCZOS).save(
        f"{out}/{size}.png", optimize=True, pnginfo=marker
    )
