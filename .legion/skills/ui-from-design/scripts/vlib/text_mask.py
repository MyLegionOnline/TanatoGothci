"""Text-region metrics from pixel masks — never OCR, never glyph content.

design_vision_toolkit_plan.md Р3: the vision-provider already returns text
*content* better than Tesseract would on stylized fonts/icons/Cyrillic. What
it does not reliably give is exact geometry, and geometry here needs no
letter recognition at all — glyph height, line pitch, edges and color are
all measurable straight off a high-contrast-vs-flat-background pixel mask.
"""
from __future__ import annotations

import numpy as np

from . import color as color_lib


def _foreground_mask(rgb: np.ndarray, bg_rgb, threshold: int = 24) -> np.ndarray:
    diff = np.abs(rgb[..., :3].astype(np.int16) - np.array(bg_rgb, dtype=np.int16)).sum(axis=-1)
    return diff > threshold


def detect_text_lines(
    rgb: np.ndarray, bg_rgb=None, min_row_coverage: float = 0.01, min_gap_rows: int = 2
) -> tuple[list[tuple[int, int]], np.ndarray]:
    """Row bands with meaningful foreground coverage — candidate text lines."""
    if bg_rgb is None:
        dominant = color_lib.dominant_colors(rgb, top=1)
        bg_rgb = dominant[0][0] if dominant else (255, 255, 255)
    mask = _foreground_mask(rgb, bg_rgb)
    if mask.shape[0] == 0:
        return [], mask
    row_coverage = mask.mean(axis=1)
    active = row_coverage > min_row_coverage
    bands: list[tuple[int, int]] = []
    y, h = 0, len(active)
    while y < h:
        if not active[y]:
            y += 1
            continue
        start = y
        while y < h and active[y]:
            y += 1
        bands.append((start, y - 1))
    merged: list[tuple[int, int]] = []
    for band in bands:
        if merged and band[0] - merged[-1][1] - 1 < min_gap_rows:
            merged[-1] = (merged[-1][0], band[1])
        else:
            merged.append(band)
    return merged, mask


def measure_text_region(rgb: np.ndarray, bg_rgb=None) -> dict | None:
    """`rgb` is a crop covering roughly one region. Returns text metrics, or
    None if no meaningful foreground ink was found (not actually text)."""
    bands, mask = detect_text_lines(rgb, bg_rgb=bg_rgb)
    if not bands:
        return None
    w = rgb.shape[1]

    def col_extent(y0: int, y1: int) -> tuple[int, int]:
        cols = np.where(mask[y0 : y1 + 1].any(axis=0))[0]
        if len(cols) == 0:
            return 0, w - 1
        return int(cols.min()), int(cols.max())

    glyph_heights = [b[1] - b[0] + 1 for b in bands]
    if len(bands) >= 2:
        pitches = [bands[i + 1][0] - bands[i][0] for i in range(len(bands) - 1)]
        line_height = round(sum(pitches) / len(pitches))
    else:
        line_height = round(glyph_heights[0] * 1.4)

    left_edges, right_edges = [], []
    for y0, y1 in bands:
        c0, c1 = col_extent(y0, y1)
        left_edges.append(c0)
        right_edges.append(c1)

    align = "left"
    if len(set(right_edges)) == 1 and len(set(left_edges)) > 1:
        align = "right"
    elif len(bands) > 1:
        centers = [(l + r) / 2 for l, r in zip(left_edges, right_edges)]
        if max(centers) - min(centers) < 2:
            align = "center"

    colors = color_lib.dominant_colors(rgb, top=2)
    background = colors[0][0] if colors else (255, 255, 255)
    foreground = colors[1][0] if len(colors) > 1 else (0, 0, 0)

    stroke_density = float(mask[bands[0][0] : bands[-1][1] + 1].mean())
    if stroke_density > 0.35:
        weight_guess = "bold"
    elif stroke_density > 0.2:
        weight_guess = "medium"
    else:
        weight_guess = "regular"

    glyph_height = max(glyph_heights)
    return {
        "box": {
            "x": min(left_edges),
            "y": bands[0][0],
            "w": max(right_edges) - min(left_edges) + 1,
            "h": bands[-1][1] - bands[0][0] + 1,
        },
        "glyph_height_px": glyph_height,
        "cap_height_px": round(glyph_height * 0.72),
        "line_height_px": line_height,
        "lines": len(bands),
        "align": align,
        "color_hex": color_lib.to_hex(foreground),
        "background_hex": color_lib.to_hex(background),
        "contrast_ratio": color_lib.contrast_ratio(foreground, background),
        "weight_guess": weight_guess,
    }
