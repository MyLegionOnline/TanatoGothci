"""Image decoding and downscale helpers shared by every vision.py subcommand.

Every subcommand works in two coordinate spaces: the ORIGINAL image (what
gets reported to the agent — design_vision_toolkit_plan.md Р3/§4 requires all
output coordinates in source-image pixels) and a smaller PROCESSING array
used internally for the actual numpy work (design_vision_toolkit_plan.md
Этап 1.3 memory budget). ``Downscaled.box_to_original`` is the one place that
translates between them; every caller must go through it instead of
recomputing ``value / scale`` inline, or rounding drift compounds silently.
"""
from __future__ import annotations

import io
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from PIL import Image


@dataclass(frozen=True)
class Decoded:
    array: np.ndarray  # uint8, shape (H, W, 3) or (H, W, 4), ORIGINAL resolution
    width: int
    height: int
    format: str
    bytes: int


def load_image(path: Path) -> Decoded:
    data = path.read_bytes()
    with Image.open(io.BytesIO(data)) as img:
        img.load()
        fmt = img.format or "UNKNOWN"
        if img.mode not in ("RGB", "RGBA"):
            img = img.convert("RGBA" if img.mode in ("P", "LA", "PA") or "A" in img.mode else "RGB")
        arr = np.asarray(img, dtype=np.uint8)
    return Decoded(array=arr, width=arr.shape[1], height=arr.shape[0], format=fmt, bytes=len(data))


def peek_dimensions(path: Path) -> tuple[int, int]:
    """Width/height without decoding pixels — used for the pre-flight memory check."""
    with Image.open(path) as img:
        return img.size  # (width, height)


@dataclass(frozen=True)
class Downscaled:
    array: np.ndarray  # uint8, PROCESSING resolution
    scale_x: float
    scale_y: float

    def to_original_x(self, value: float) -> int:
        return round(value / self.scale_x)

    def to_original_y(self, value: float) -> int:
        return round(value / self.scale_y)

    def box_to_original(self, x: float, y: float, w: float, h: float) -> dict:
        # Deriving w/h from (x1 - x0) rather than rounding w independently
        # keeps adjacent regions seamless and avoids compounding rounding
        # error — this is what the Этап 1.5 "±2 px after downscale" test
        # actually checks.
        x0 = self.to_original_x(x)
        y0 = self.to_original_y(y)
        x1 = self.to_original_x(x + w)
        y1 = self.to_original_y(y + h)
        return {"x": x0, "y": y0, "w": max(1, x1 - x0), "h": max(1, y1 - y0)}


def downscale(decoded: Decoded, max_pixels: int = 2_000_000, max_long_side: int | None = None) -> Downscaled:
    h, w = decoded.array.shape[:2]
    scale = 1.0
    total = w * h
    if total > max_pixels > 0:
        scale = min(scale, (max_pixels / total) ** 0.5)
    if max_long_side:
        long_side = max(w, h)
        if long_side * scale > max_long_side:
            scale = min(scale, max_long_side / long_side)
    if scale >= 1.0:
        return Downscaled(array=decoded.array, scale_x=1.0, scale_y=1.0)
    new_w = max(1, round(w * scale))
    new_h = max(1, round(h * scale))
    # NEAREST, not LANCZOS: this array is for measurement, not display.
    # UI screenshots are flat-fill and axis-aligned (Р3) — LANCZOS's
    # ringing/blur at a hard edge shifts the detected boundary by a couple
    # of downscaled px, which the 1/scale remap then amplifies past the
    # ±2px budget the Этап 1.5 coordinate test enforces. NEAREST keeps
    # edges exactly where they were, just at lower sampling density.
    img = Image.fromarray(decoded.array).resize((new_w, new_h), Image.NEAREST)
    arr = np.asarray(img, dtype=np.uint8)
    return Downscaled(array=arr, scale_x=new_w / w, scale_y=new_h / h)
