"""Synthetic PNG fixtures built purely with Pillow — Этап 1.5 requires this
(no external test images), and it gives every test exact, known-in-advance
boxes to assert against.
"""
from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageDraw

WHITE = (255, 255, 255)
PAGE_BG = (240, 240, 240)
HEADER_BLUE = (30, 64, 175)
CARD_HEADER_GRAY = (229, 231, 235)
BUTTON_BLUE = (59, 130, 246)
INK = (17, 24, 39)


def dashboard_image(path: Path, scale: int = 1) -> dict:
    """Header bar + card (with an inset header band + a button), on a page
    background — the layered, non-flat structure the `regions` pipeline is
    meant to recover. Returns the ground-truth boxes for the top-level
    blocks so tests can assert the segmenter's output against them.
    `scale` multiplies every dimension — used to build an oversized variant
    that forces the Этап 1.3 downscale path (test_downscale.py)."""

    def box(x: int, y: int, w: int, h: int) -> dict:
        return {"x": x * scale, "y": y * scale, "w": w * scale, "h": h * scale}

    width, height = 400 * scale, 300 * scale
    img = Image.new("RGB", (width, height), PAGE_BG)
    draw = ImageDraw.Draw(img)

    def fill(b: dict, color) -> None:
        draw.rectangle([b["x"], b["y"], b["x"] + b["w"] - 1, b["y"] + b["h"] - 1], fill=color)

    header_box = box(0, 0, 400, 50)
    fill(header_box, HEADER_BLUE)

    card_box = box(32, 96, 320, 180)
    fill(card_box, WHITE)

    card_header_box = box(32, 96, 320, 40)
    fill(card_header_box, CARD_HEADER_GRAY)

    button_box = box(60, 220, 120, 36)
    fill(button_box, BUTTON_BLUE)

    img.save(path, format="PNG")
    return {"header": header_box, "card": card_box, "card_header": card_header_box, "button": button_box}


def text_lines_image(path: Path, glyph_height: int = 16, line_pitch: int = 24, lines: int = 2) -> dict:
    """A background with `lines` rows of glyph-proxy ink strokes — narrow
    filled rectangles standing in for letters, at an EXACT known glyph
    height and line pitch. Real font rendering (`ImageFont.load_default`)
    has hinting/metric quirks that would make the "±1 px" acceptance check
    in Этап 1.5 depend on the Pillow/font version instead of the algorithm
    under test — synthetic strokes keep the ground truth exact."""
    width, height = 320, 20 + lines * line_pitch
    img = Image.new("RGB", (width, height), WHITE)
    draw = ImageDraw.Draw(img)
    stroke_width, stroke_gap, strokes_per_line = 8, 6, 6
    top = 20
    line_tops = []
    for line_index in range(lines):
        y0 = top + line_index * line_pitch
        line_tops.append(y0)
        for s in range(strokes_per_line):
            x0 = 20 + s * (stroke_width + stroke_gap)
            draw.rectangle([x0, y0, x0 + stroke_width - 1, y0 + glyph_height - 1], fill=INK)
    img.save(path, format="PNG")
    return {"glyph_height": glyph_height, "line_pitch": line_pitch, "lines": lines, "line_tops": line_tops}


def diff_pair_shifted_bar(dir_path: Path, shift: int = 10, bar_w: int = 10, bar_h: int = 40) -> dict:
    """Two otherwise-identical images with one bar shifted horizontally by
    exactly `shift` px. `bar_w == shift` so the old and new positions are
    adjacent with no overlap and no gap — the diff is one contiguous region,
    matching the Этап 1.5 "shift by 10px -> exactly one diff_region" case."""
    width, height = 200, 200
    x0, y0 = 60, 60

    a = Image.new("RGB", (width, height), WHITE)
    ImageDraw.Draw(a).rectangle([x0, y0, x0 + bar_w - 1, y0 + bar_h - 1], fill=INK)
    a_path = dir_path / "diff_a.png"
    a.save(a_path, format="PNG")

    b = Image.new("RGB", (width, height), WHITE)
    ImageDraw.Draw(b).rectangle([x0 + shift, y0, x0 + shift + bar_w - 1, y0 + bar_h - 1], fill=INK)
    b_path = dir_path / "diff_b.png"
    b.save(b_path, format="PNG")

    return {"a": a_path, "b": b_path, "expected_box": {"x": x0, "y": y0, "w": bar_w + shift, "h": bar_h}}


def solid_image(path: Path, size: tuple[int, int], rgb: tuple[int, int, int]) -> None:
    Image.new("RGB", size, rgb).save(path, format="PNG")
