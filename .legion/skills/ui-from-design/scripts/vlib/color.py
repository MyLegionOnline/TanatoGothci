"""Color quantization, palette extraction, WCAG contrast math.

No OCR anywhere in this package (design_vision_toolkit_plan.md Р3) — this
module only ever deals with pixel colors, never glyph content.
"""
from __future__ import annotations

import numpy as np


def to_hex(rgb) -> str:
    r, g, b = (int(c) for c in rgb[:3])
    return f"#{r:02X}{g:02X}{b:02X}"


def hex_to_rgb(hex_str: str) -> tuple[int, int, int]:
    h = hex_str.lstrip("#")
    if len(h) == 3:
        h = "".join(c * 2 for c in h)
    if len(h) != 6:
        raise ValueError(f"not a hex color: {hex_str!r}")
    return (int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16))


def relative_luminance(rgb) -> float:
    def chan(c: float) -> float:
        c = c / 255.0
        return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4

    r, g, b = (chan(c) for c in rgb[:3])
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contrast_ratio(rgb_a, rgb_b) -> float:
    la, lb = relative_luminance(rgb_a), relative_luminance(rgb_b)
    lighter, darker = max(la, lb), min(la, lb)
    return round((lighter + 0.05) / (darker + 0.05), 2)


def wcag_levels(ratio: float, font_px: float | None = None, bold: bool = False) -> dict:
    """WCAG 2.1 pass/fail at a given ratio. `large text` = >=24px, or >=18.66px bold."""
    is_large = bool(font_px and (font_px >= 24 or (font_px >= 18.66 and bold)))
    aa_threshold = 3.0 if is_large else 4.5
    aaa_threshold = 4.5 if is_large else 7.0
    return {
        "aa": ratio >= aa_threshold,
        "aa_large": ratio >= 3.0,
        "aaa": ratio >= aaa_threshold,
    }


def suggest_foreground(background_rgb) -> str:
    """Whichever of pure black/white contrasts more against this background."""
    black_ratio = contrast_ratio((0, 0, 0), background_rgb)
    white_ratio = contrast_ratio((255, 255, 255), background_rgb)
    return to_hex((0, 0, 0)) if black_ratio >= white_ratio else to_hex((255, 255, 255))


def quantize(array: np.ndarray, bits_per_channel: int = 5) -> np.ndarray:
    """Reduce each channel to `bits_per_channel` bits of precision (still uint8)."""
    if bits_per_channel >= 8:
        return array
    shift = 8 - bits_per_channel
    q = (array[..., :3].astype(np.uint16) >> shift) << shift
    return q.astype(np.uint8)


def dominant_colors(rgb: np.ndarray, top: int = 2) -> list[tuple[tuple[int, int, int], int]]:
    """Most frequent exact colors in an (H, W, 3) array, most-common first."""
    flat = rgb[..., :3].reshape(-1, 3)
    if flat.size == 0:
        return []
    values, counts = np.unique(flat, axis=0, return_counts=True)
    order = np.argsort(-counts)[:top]
    return [(tuple(int(c) for c in values[i]), int(counts[i])) for i in order]


def dominant_palette(array: np.ndarray, palette_size: int = 8, bits_per_channel: int = 5) -> list[dict]:
    """Top-N quantized colors by pixel coverage — the `probe` palette."""
    rgb = array[..., :3].reshape(-1, 3)
    if rgb.shape[0] == 0:
        return []
    quantized = quantize(rgb, bits_per_channel=bits_per_channel)
    packed = (
        (quantized[:, 0].astype(np.uint32) << 16)
        | (quantized[:, 1].astype(np.uint32) << 8)
        | quantized[:, 2].astype(np.uint32)
    )
    values, counts = np.unique(packed, return_counts=True)
    order = np.argsort(-counts)[:palette_size]
    total = rgb.shape[0]
    out = []
    for idx in order:
        v = int(values[idx])
        r, g, b = (v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF
        out.append(
            {
                "hex": to_hex((r, g, b)),
                "rgb": [r, g, b],
                "coverage": round(float(counts[idx]) / total, 4),
            }
        )
    return out


def background_color(array: np.ndarray) -> tuple[int, int, int]:
    """Most common color along the image's 1px border — a page background heuristic."""
    h, w = array.shape[:2]
    if h < 2 or w < 2:
        return tuple(int(c) for c in array[0, 0, :3])
    border = np.concatenate(
        [array[0, :, :3], array[-1, :, :3], array[:, 0, :3], array[:, -1, :3]]
    )
    values, counts = np.unique(border.reshape(-1, 3), axis=0, return_counts=True)
    return tuple(int(c) for c in values[np.argmax(counts)])
