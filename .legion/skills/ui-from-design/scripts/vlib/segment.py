"""Axis-aligned recursive segmentation (XY-cut) for flat-fill UI screenshots.

design_vision_toolkit_plan.md Р3: screenshots are not photographs — shapes
are axis-aligned and fills are flat, so alternating row/column "gap" cuts
plus a color-band fallback are enough to recover a block tree. Deliberately
not general connected-component labelling (no scipy/OpenCV in the base
image) and not a Hough transform — see Р3 for why those are overkill here.
"""
from __future__ import annotations

from dataclasses import dataclass, field

import numpy as np

from . import budget as budget_lib
from . import color as color_lib


@dataclass
class Node:
    box: tuple[int, int, int, int]  # x, y, w, h — in the array's own coordinate space
    depth: int
    children: list["Node"] = field(default_factory=list)
    leaf_kind: str = "flat"  # "flat" | "texty" — set only once a node stops subdividing


def _row_matches(row: np.ndarray, bg_rgb, bits: int = 5) -> bool:
    """True if an entire row/column (an (N, 3) slice) is — within quantized
    tolerance — a single specific color: the crop's own local background.

    Matching against "any <=2 colors" (the first version of this check) was
    wrong: it also matched a row split cleanly between two flat *content*
    blocks (e.g. a header bar next to a card), which made `_find_gap_bands`
    treat block boundaries as whitespace and erase the blocks. A gap must
    match a *specific* background color, not just "few colors".
    """
    quant = color_lib.quantize(row.reshape(-1, 3), bits_per_channel=bits)
    bg_quant = color_lib.quantize(np.array([[bg_rgb]], dtype=np.uint8), bits_per_channel=bits)[0, 0]
    return bool(np.all(quant == bg_quant))


def _find_gap_bands(rgb: np.ndarray, axis: int, min_gap: int, bg_rgb) -> list[tuple[int, int]]:
    """axis=0 -> rows (finds horizontal whitespace bands); axis=1 -> columns."""
    scan = rgb if axis == 0 else np.swapaxes(rgb, 0, 1)
    n = scan.shape[0]
    matches = np.fromiter((_row_matches(scan[i], bg_rgb) for i in range(n)), dtype=bool, count=n)
    gaps = []
    i = 0
    while i < n:
        if not matches[i]:
            i += 1
            continue
        start = i
        while i < n and matches[i]:
            i += 1
        if i - start >= min_gap:
            gaps.append((start, i - 1))
    return gaps


def _split_by_gaps(rgb: np.ndarray, axis: int, min_gap: int, bg_rgb) -> list[tuple[int, int]]:
    """Non-gap (start, end) index ranges, inclusive, along `axis`."""
    n = rgb.shape[axis]
    gaps = _find_gap_bands(rgb, axis, min_gap, bg_rgb)
    segments = []
    cursor = 0
    for g0, g1 in gaps:
        if g0 > cursor:
            segments.append((cursor, g0 - 1))
        cursor = g1 + 1
    if cursor < n:
        segments.append((cursor, n - 1))
    return segments


def _row_run_lengths(mask_row: np.ndarray) -> np.ndarray:
    if not mask_row.any():
        return np.array([], dtype=int)
    padded = np.concatenate(([False], mask_row, [False]))
    diffs = np.diff(padded.astype(np.int8))
    starts = np.where(diffs == 1)[0]
    ends = np.where(diffs == -1)[0]
    return ends - starts


def _looks_texty(
    crop: np.ndarray, bg_rgb, min_runs: int = 6, max_avg_run_width: float = 20.0, row_fraction: float = 0.6
) -> bool:
    """True when most rows are made of *many, narrow* foreground runs — the
    signature of individual glyph strokes, as opposed to a handful of *wide*
    runs (a row of a few UI blocks). Gates the column-cut specifically:
    without it, the gap-cutter happily separates "T", "e", "x", "t" into
    four regions instead of recognizing one text line (see the module
    docstring in `vision.py` and Этап 1.5's text_mask test for why this
    matters). Known false-negative: a row of many small *icons* looks the
    same by this metric and gets swept into one "texty" blob too — accepted
    for v1, revisit if it turns out to matter in practice."""
    diff = np.abs(crop[..., :3].astype(np.int16) - np.array(bg_rgb, dtype=np.int16)).sum(axis=-1) > 24
    if diff.shape[0] == 0:
        return False
    texty_rows = 0
    for row in diff:
        runs = _row_run_lengths(row)
        if len(runs) >= min_runs and runs.mean() <= max_avg_run_width:
            texty_rows += 1
    return (texty_rows / diff.shape[0]) >= row_fraction


def _is_flat(rgb: np.ndarray, tolerance: int = 12, flat_fraction: float = 0.92) -> bool:
    flat = rgb[..., :3].reshape(-1, 3)
    if flat.shape[0] == 0:
        return True
    mean = flat.mean(axis=0)
    dist = np.abs(flat.astype(np.int16) - mean.astype(np.int16)).sum(axis=1)
    return float((dist <= tolerance).mean()) > flat_fraction


def _color_bands(rgb: np.ndarray, axis: int, min_band: int, color_bits: int) -> list[tuple[int, int]]:
    """Contiguous bands of stable *dominant* color along `axis` — catches blocks
    that abut directly (e.g. a card header sitting flush on its body) where
    there is no whitespace gap to cut on."""
    scan = rgb if axis == 0 else np.swapaxes(rgb, 0, 1)
    n = scan.shape[0]
    if n == 0:
        return []
    quant = color_lib.quantize(scan, bits_per_channel=color_bits)
    mid = quant.shape[1] // 2
    reps = quant[:, mid, :3]
    bands = []
    start = 0
    for i in range(1, n + 1):
        changed = i == n or not np.array_equal(reps[i], reps[start])
        if changed:
            if i - start >= min_band:
                bands.append((start, i - 1))
            start = i
    return bands if len(bands) >= 2 else []


def segment(
    rgb: np.ndarray,
    min_area: int = 1,
    max_depth: int = 6,
    min_gap: int = 2,
    color_bits: int = 4,
    time_budget: budget_lib.TimeBudget | None = None,
) -> Node:
    """Build the block tree. `min_area` here is in the array's OWN pixel units
    (the caller remaps to original-image px² after coordinate translation)."""
    root = Node(box=(0, 0, rgb.shape[1], rgb.shape[0]), depth=0)
    _segment_into(rgb, root, min_area, max_depth, min_gap, color_bits, time_budget)
    return root


def _segment_into(
    full: np.ndarray,
    node: Node,
    min_area: int,
    max_depth: int,
    min_gap: int,
    color_bits: int,
    time_budget: budget_lib.TimeBudget | None,
) -> None:
    x, y, w, h = node.box
    if w * h < min_area or node.depth >= max_depth or (time_budget is not None and not time_budget.check()):
        node.leaf_kind = "flat" if _is_flat(full[y : y + h, x : x + w]) else "texty"
        return
    crop = full[y : y + h, x : x + w]
    # Recomputed fresh at every level, not inherited from the parent: a
    # button sitting on a white card needs "white" as its local gap
    # reference, not the page's outer background — see the module docstring
    # on `_row_matches` for why this has to be a *specific* color at all.
    bg_rgb = color_lib.background_color(crop)

    if _looks_texty(crop, bg_rgb):
        node.leaf_kind = "texty"
        return

    row_segments = _split_by_gaps(crop, axis=0, min_gap=min_gap, bg_rgb=bg_rgb)
    if row_segments and row_segments != [(0, h - 1)]:
        for r0, r1 in row_segments:
            child = Node(box=(x, y + r0, w, r1 - r0 + 1), depth=node.depth + 1)
            _segment_into(full, child, min_area, max_depth, min_gap, color_bits, time_budget)
            node.children.append(child)
        return

    col_segments = _split_by_gaps(crop, axis=1, min_gap=min_gap, bg_rgb=bg_rgb)
    if col_segments and col_segments != [(0, w - 1)]:
        for c0, c1 in col_segments:
            child = Node(box=(x + c0, y, c1 - c0 + 1, h), depth=node.depth + 1)
            _segment_into(full, child, min_area, max_depth, min_gap, color_bits, time_budget)
            node.children.append(child)
        return

    if _is_flat(crop):
        node.leaf_kind = "flat"
        return

    bands = _color_bands(crop, axis=0, min_band=max(3, h // 12), color_bits=color_bits)
    if bands:
        for b0, b1 in bands:
            child = Node(box=(x, y + b0, w, b1 - b0 + 1), depth=node.depth + 1)
            _segment_into(full, child, min_area, max_depth, min_gap, color_bits, time_budget)
            node.children.append(child)
        return

    node.leaf_kind = "texty"


def iter_nodes(root: Node):
    """Pre-order walk, skipping the synthetic whole-canvas root itself —
    only its descendants are meaningful "regions"."""
    stack = list(reversed(root.children))
    while stack:
        node = stack.pop()
        yield node
        stack.extend(reversed(node.children))


def estimate_border(full: np.ndarray, box: dict, tolerance: int = 16) -> dict | None:
    """Heuristic: compare the box's top edge row to a row further inside —
    a stable color difference between them reads as a 1px border stroke."""
    x, y, w, h = box["x"], box["y"], box["w"], box["h"]
    if w < 6 or h < 6:
        return None
    top_edge = full[y, x : x + w, :3]
    interior_y = min(y + max(3, h // 6), y + h - 1)
    interior = full[interior_y, x : x + w, :3]
    edge_mean = top_edge.mean(axis=0)
    interior_mean = interior.mean(axis=0)
    diff = float(np.abs(edge_mean.astype(np.int16) - interior_mean.astype(np.int16)).sum())
    if diff < tolerance:
        return None
    return {"hex": color_lib.to_hex(edge_mean.astype(np.uint8)), "width": 1}


def estimate_radius(full: np.ndarray, box: dict, fill_rgb, outside_rgb, max_probe: int = 24) -> int:
    """Heuristic corner-rounding estimate: walk diagonally in from each corner
    and count how many pixels read closer to `outside_rgb` than `fill_rgb`."""
    x, y, w, h = box["x"], box["y"], box["w"], box["h"]
    height, width = full.shape[:2]
    probe = min(max_probe, w // 2, h // 2)
    if probe < 2:
        return 0
    fill = np.array(fill_rgb, dtype=np.int16)
    outside = np.array(outside_rgb, dtype=np.int16)

    def corner_depth(cx: int, cy: int, dx: int, dy: int) -> int:
        depth = 0
        for i in range(probe):
            px, py = cx + dx * i, cy + dy * i
            if px < 0 or py < 0 or px >= width or py >= height:
                break
            pixel = full[py, px, :3].astype(np.int16)
            dist_fill = np.abs(pixel - fill).sum()
            dist_out = np.abs(pixel - outside).sum()
            if dist_out < dist_fill:
                depth += 1
            else:
                break
        return depth

    corners = [
        corner_depth(x, y, 1, 1),
        corner_depth(x + w - 1, y, -1, 1),
        corner_depth(x, y + h - 1, 1, -1),
        corner_depth(x + w - 1, y + h - 1, -1, -1),
    ]
    return int(round(sum(corners) / len(corners)))
