#!/usr/bin/env python3
"""Regenerate the known-text region images (verification resources, never bundled).

Requires Pillow and Liberation Sans 2.1.5 (font SHA256 is printed).
"""
import hashlib
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont
FONT = Path('/usr/share/fonts/liberation-sans-fonts/LiberationSans-Regular.ttf')
if not FONT.exists(): FONT = Path('/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf')
FONT_SHA256 = '76d04c18ea243f426b7de1f3ad208e927008f961dc5945e5aad352d0dfde8ee8'
assert hashlib.sha256(FONT.read_bytes()).hexdigest() == FONT_SHA256, 'unexpected fixture font'
print('font SHA256:', FONT_SHA256)
TEXTS = {'eng': 'The bank is beside the river.',
         'spa': 'El niño está cerca del río.',
         'cat': 'El camí és a prop del riu.'}
for language, text in TEXTS.items():
    image = Image.new('RGB', (1200, 150), 'white')
    ImageDraw.Draw(image).text((30, 40), text, font=ImageFont.truetype(str(FONT), 48), fill='black')
    image.save(Path(__file__).parent/(language+'.png'))
