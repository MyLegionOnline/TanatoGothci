"""Batch geometry: distances, gaps, alignment groups, spacing rhythm.

Pure arithmetic on caller-supplied boxes/points — no pixel access, so this is
the one module that never needs `decode`/`budget`.
"""
from __future__ import annotations

import math


def distance(a: dict, b: dict) -> dict:
    dx = b["x"] - a["x"]
    dy = b["y"] - a["y"]
    return {"px": round(math.hypot(dx, dy), 2), "dx": dx, "dy": dy}


def _edges(box: dict) -> dict:
    return {
        "left": box["x"],
        "right": box["x"] + box["w"],
        "top": box["y"],
        "bottom": box["y"] + box["h"],
    }


def gap(a: dict, b: dict) -> dict:
    ea, eb = _edges(a), _edges(b)
    horizontal = max(eb["left"] - ea["right"], ea["left"] - eb["right"], 0)
    vertical = max(eb["top"] - ea["bottom"], ea["top"] - eb["bottom"], 0)
    return {"horizontal": horizontal, "vertical": vertical}


_EDGE_GETTERS = {
    "left": lambda b: b["x"],
    "right": lambda b: b["x"] + b["w"],
    "top": lambda b: b["y"],
    "bottom": lambda b: b["y"] + b["h"],
    "center_x": lambda b: b["x"] + b["w"] / 2,
    "center_y": lambda b: b["y"] + b["h"] / 2,
}


def alignment_groups(boxes: list[dict], tolerance: int = 2) -> list[dict]:
    """For each edge kind, greedily cluster boxes whose edge values fall
    within `tolerance` of their neighbor in sorted order. A simplification of
    true clustering (a long chain of near-tolerance steps can drift beyond
    `tolerance` end-to-end) — acceptable for the UI-alignment use case, where
    real groups sit far tighter than borderline chains."""
    groups = []
    for edge_name, getter in _EDGE_GETTERS.items():
        indexed = sorted(range(len(boxes)), key=lambda i: getter(boxes[i]))
        cluster: list[int] = []

        def flush(members: list[int]) -> None:
            if len(members) < 2:
                return
            values = [getter(boxes[i]) for i in members]
            labels = [boxes[i].get("label", f"box{i}") for i in members]
            groups.append(
                {
                    "edge": edge_name,
                    "members": labels,
                    "max_deviation": round(max(values) - min(values), 2),
                }
            )

        for i in indexed:
            if cluster and getter(boxes[i]) - getter(boxes[cluster[0]]) > tolerance:
                flush(cluster)
                cluster = []
            cluster.append(i)
        flush(cluster)
    return groups


def spacing_rhythm(gaps_px: list[float], spacing_scale: int = 8) -> dict:
    observed = sorted({round(g) for g in gaps_px})
    nearest_multiples = {}
    violators = []
    for g in observed:
        nearest = round(g / spacing_scale) * spacing_scale if spacing_scale else g
        nearest_multiples[g] = nearest
        if abs(g - nearest) > max(2, spacing_scale * 0.25):
            violators.append(g)
    return {
        "scale": spacing_scale,
        "observed": observed,
        "nearest_multiples": nearest_multiples,
        "violators": violators,
    }
