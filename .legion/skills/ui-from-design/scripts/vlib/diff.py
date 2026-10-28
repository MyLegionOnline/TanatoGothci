"""Pixel diff between a mockup and an implementation screenshot."""
from __future__ import annotations

import numpy as np
from PIL import Image

from . import segment as segment_lib


def resize_to_match(a: np.ndarray, b: np.ndarray, fit: str = "scale") -> tuple[np.ndarray, np.ndarray]:
    """Resize `b` onto `a`'s canvas so they're diffable pixel-for-pixel.
    `a` is treated as the size of record (`--image`, the screenshot)."""
    ha, wa = a.shape[:2]
    hb, wb = b.shape[:2]
    if (ha, wa) == (hb, wb) or fit == "none":
        return a, b
    resized = Image.fromarray(b).resize((wa, ha), Image.LANCZOS)
    return a, np.asarray(resized, dtype=np.uint8)


def pixel_diff(a: np.ndarray, b: np.ndarray, tolerance: int = 8) -> tuple[np.ndarray, float]:
    """Boolean diff mask + overall diff_ratio. `tolerance` is a per-channel
    delta threshold; the sum-of-abs-channel-diff must exceed tolerance*3 to
    count as differing, so small anti-aliasing noise doesn't dominate."""
    a3 = a[..., :3].astype(np.int16)
    b3 = b[..., :3].astype(np.int16)
    delta = np.abs(a3 - b3).sum(axis=-1)
    mask = delta > tolerance * 3
    ratio = float(mask.mean()) if mask.size else 0.0
    return mask, ratio


def _classify_kind(a_crop: np.ndarray, b_crop: np.ndarray) -> str:
    a_std = float(a_crop[..., :3].reshape(-1, 3).std())
    b_std = float(b_crop[..., :3].reshape(-1, 3).std())
    a_has_content = a_std > 6
    b_has_content = b_std > 6
    if a_has_content and not b_has_content:
        return "missing"
    if b_has_content and not a_has_content:
        return "extra"
    a_mean = a_crop[..., :3].reshape(-1, 3).mean(axis=0)
    b_mean = b_crop[..., :3].reshape(-1, 3).mean(axis=0)
    color_dist = float(np.abs(a_mean - b_mean).sum())
    return "color" if color_dist > 60 else "position"


def diff_regions(
    mask: np.ndarray, a: np.ndarray, b: np.ndarray, min_area: int = 25, max_regions: int = 20
) -> tuple[list[dict], bool]:
    """Connected diff blobs, classified and ranked largest-first. Reuses
    `segment`'s gap-cutting on the binary mask reinterpreted as a 3-channel
    image (diff pixels = white, matching pixels = black) instead of a
    separate connected-components implementation."""
    if not mask.any():
        return [], False
    mask_rgb = np.repeat((mask.astype(np.uint8) * 255)[..., None], 3, axis=-1)
    tree = segment_lib.segment(mask_rgb, min_area=1, max_depth=8, min_gap=1)
    # Only LEAF nodes are real diff blobs. `iter_nodes` also yields the
    # intermediate row/column-band nodes the recursive cut passes through on
    # its way to a leaf (see segment.py) — those still "contain" the leaf's
    # diff pixels, so including them would double-report the same blob at a
    # looser, less useful bounding box.
    boxes = [n.box for n in segment_lib.iter_nodes(tree) if not n.children and _node_is_diff(mask, n.box)]
    boxes.sort(key=lambda box: box[2] * box[3], reverse=True)
    truncated = len(boxes) > max_regions
    boxes = boxes[:max_regions]

    regions = []
    for x, y, w, h in boxes:
        if w * h < min_area:
            continue
        sub_mask = mask[y : y + h, x : x + w]
        region_ratio = float(sub_mask.mean()) if sub_mask.size else 0.0
        kind = _classify_kind(a[y : y + h, x : x + w], b[y : y + h, x : x + w])
        regions.append(
            {
                "box": {"x": int(x), "y": int(y), "w": int(w), "h": int(h)},
                "diff_ratio": round(region_ratio, 4),
                "kind": kind,
            }
        )
    return regions, truncated


def _node_is_diff(mask: np.ndarray, box: tuple[int, int, int, int]) -> bool:
    x, y, w, h = box
    return bool(mask[y : y + h, x : x + w].any())


def render_heatmap(base: np.ndarray, mask: np.ndarray) -> Image.Image:
    """Grayscale `base` with diff pixels tinted red — written straight to a
    PNG artifact by the caller, never returned as pixels (Р4/S1)."""
    gray = np.asarray(Image.fromarray(base[..., :3]).convert("L").convert("RGB"), dtype=np.uint8)
    out = gray.copy()
    out[mask, 0] = 255
    out[mask, 1] = (gray[mask, 1] * 0.35).astype(np.uint8)
    out[mask, 2] = (gray[mask, 2] * 0.35).astype(np.uint8)
    return Image.fromarray(out)
