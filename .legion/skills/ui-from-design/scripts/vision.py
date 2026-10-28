#!/usr/bin/env python3
"""vision.py — deterministic pixel-measurement CV core for the ui-from-design
skill (design_vision_toolkit_plan.md Этап 1 / §4.1).

One entry point, seven subcommands (probe, sample, measure, regions, crop,
compare, contrast). No JSON schema validates this from the caller's side
(design_vision_toolkit_plan.md Р2), so this file compensates the way the
plan requires:
  - argparse with explicit choices/types and mutually-exclusive groups;
  - stdout is ALWAYS JSON, success or failure;
  - every error is `{"error": "...", "hint": "..."}` with a runnable example
    in `hint`, plus a non-zero exit code;
  - paths are resolved relative to the current working directory (the
    workspace root in production — see SKILL.md for the invocation
    contract) and a path that escapes it is rejected;
  - large results go to `--out FILE`; stdout gets a short summary instead
    (design_vision_toolkit_plan.md Р4/S3) — this is what keeps a region
    tree or a diff report out of the agent's context.

No network access anywhere in this file or `vlib/` — verified by
`tests/test_no_network.py`.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from PIL import Image

sys.path.insert(0, str(Path(__file__).resolve().parent))

from vlib import budget as budget_lib  # noqa: E402
from vlib import color as color_lib  # noqa: E402
from vlib import decode as decode_lib  # noqa: E402
from vlib import diff as diff_lib  # noqa: E402
from vlib import measure as measure_lib  # noqa: E402
from vlib import segment as segment_lib  # noqa: E402
from vlib import text_mask as text_mask_lib  # noqa: E402

MEMORY_SAFETY_FACTOR = 8  # covers int16/bool/float intermediates the CV code allocates


class VisionError(Exception):
    def __init__(self, message: str, hint: str):
        super().__init__(message)
        self.hint = hint


# --------------------------------------------------------------------------
# Path + I/O helpers
# --------------------------------------------------------------------------


def resolve_path(raw: str, *, must_exist: bool = False) -> Path:
    root = Path.cwd().resolve()
    candidate = (root / raw).resolve()
    try:
        candidate.relative_to(root)
    except ValueError as exc:
        raise VisionError(
            f"path escapes workspace root: {raw!r}",
            hint="pass a path relative to /workspace, without '..' segments, e.g. design/dashboard.png",
        ) from exc
    if must_exist and not candidate.is_file():
        raise VisionError(
            f"file not found: {raw!r}",
            hint=f"check the path is relative to /workspace and exists, e.g. design/{Path(raw).name}",
        )
    return candidate


def load_extra_args(args: argparse.Namespace) -> dict:
    """Merge --in/--json into a dict of the subcommand's non-`--image` fields."""
    if getattr(args, "in_file", None):
        path = resolve_path(args.in_file, must_exist=True)
        try:
            return json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            raise VisionError(
                f"invalid JSON in --in {args.in_file!r}: {exc}",
                hint="the file must contain a single JSON object matching this subcommand's input schema",
            ) from exc
    if getattr(args, "json_arg", None):
        try:
            return json.loads(args.json_arg)
        except json.JSONDecodeError as exc:
            raise VisionError(
                f"invalid JSON in --json: {exc}",
                hint='pass a single-line JSON object, e.g. --json \'{"pairs": [...]}\'',
            ) from exc
    return {}


def write_out(out_arg: str | None, payload: dict) -> Path | None:
    if not out_arg:
        return None
    path = resolve_path(out_arg)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, ensure_ascii=False), encoding="utf-8")
    return path


def emit(summary: dict) -> None:
    print(json.dumps(summary, ensure_ascii=False))


def rel(path: Path) -> str:
    """POSIX-style path relative to the workspace root — the contract in
    design_vision_toolkit_plan.md §4 always shows forward slashes, and the
    agent's other tools (read_file, edit_file) expect the same."""
    return path.relative_to(Path.cwd().resolve()).as_posix()


def default_artifact_path(image_path: Path, suffix: str) -> Path:
    root = Path.cwd().resolve()
    out_dir = root / "design" / "artifacts"
    out_dir.mkdir(parents=True, exist_ok=True)
    return out_dir / f"{image_path.stem}-{suffix}.png"


# --------------------------------------------------------------------------
# Image loading with the memory pre-flight check (Этап 1.3)
# --------------------------------------------------------------------------


def load_and_prepare(
    image_arg: str, *, max_pixels: int, max_long_side: int | None = None
) -> tuple[Path, decode_lib.Decoded, decode_lib.Downscaled]:
    path = resolve_path(image_arg, must_exist=True)
    try:
        w0, h0 = decode_lib.peek_dimensions(path)
    except Exception as exc:  # noqa: BLE001 - Pillow raises several distinct error types
        raise VisionError(
            f"could not read image: {image_arg!r} ({exc})",
            hint="confirm the file is a valid PNG/JPEG and the path is relative to /workspace",
        ) from exc
    budget_lib.check_memory_budget(
        w0 * h0 * 4 * MEMORY_SAFETY_FACTOR,
        hint_example=f"vision.py probe --image {image_arg} --max-pixels 1000000",
    )
    decoded = decode_lib.load_image(path)
    downscaled = decode_lib.downscale(decoded, max_pixels=max_pixels, max_long_side=max_long_side)
    return path, decoded, downscaled


def add_image_args(parser: argparse.ArgumentParser, *, required: bool = True) -> None:
    parser.add_argument("--image", required=required, help="path relative to /workspace")
    add_max_pixels_arg(parser)


def add_max_pixels_arg(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--max-pixels", type=int, default=2_000_000, help="processing downscale budget (Этап 1.3)")


def add_batch_args(parser: argparse.ArgumentParser, *, required: bool = False) -> None:
    group = parser.add_mutually_exclusive_group(required=required)
    group.add_argument("--in", dest="in_file", help="path to a JSON file with this subcommand's extra input")
    group.add_argument("--json", dest="json_arg", help="inline JSON string, same shape as --in")


def parse_box(raw: str, field_name: str = "--box") -> dict:
    parts = raw.split(",")
    if len(parts) != 4:
        raise VisionError(
            f"{field_name} must be 'x,y,w,h': got {raw!r}",
            hint=f"example: {field_name} 32,96,320,180",
        )
    try:
        x, y, w, h = (int(p) for p in parts)
    except ValueError as exc:
        raise VisionError(
            f"{field_name} values must be integers: got {raw!r}",
            hint=f"example: {field_name} 32,96,320,180",
        ) from exc
    return {"x": x, "y": y, "w": w, "h": h}


def clamp_box(box: dict, width: int, height: int) -> dict:
    x = max(0, min(box["x"], width - 1))
    y = max(0, min(box["y"], height - 1))
    w = max(1, min(box["w"], width - x))
    h = max(1, min(box["h"], height - y))
    return {"x": x, "y": y, "w": w, "h": h}


# --------------------------------------------------------------------------
# probe
# --------------------------------------------------------------------------


def cmd_probe(args: argparse.Namespace) -> dict:
    path, decoded, _ = load_and_prepare(args.image, max_pixels=args.max_pixels)
    palette = color_lib.dominant_palette(decoded.array, palette_size=args.palette_size)
    background_hex = color_lib.to_hex(color_lib.background_color(decoded.array))
    result = {
        "width": decoded.width,
        "height": decoded.height,
        "format": decoded.format,
        "bytes": decoded.bytes,
        "aspect_ratio": round(decoded.width / decoded.height, 4) if decoded.height else None,
        "palette": palette,
        "background_hex": background_hex,
    }
    if args.grid_step:
        img = Image.fromarray(decoded.array).convert("RGBA")
        from PIL import ImageDraw

        draw = ImageDraw.Draw(img, "RGBA")
        grid_color = (255, 0, 255, 120)
        for x in range(0, decoded.width, args.grid_step):
            draw.line([(x, 0), (x, decoded.height)], fill=grid_color, width=1)
        for y in range(0, decoded.height, args.grid_step):
            draw.line([(0, y), (decoded.width, y)], fill=grid_color, width=1)
        artifact_path = default_artifact_path(path, "grid")
        img.convert("RGB").save(artifact_path, format="PNG")
        result["artifacts"] = [{"kind": "grid", "path": rel(artifact_path)}]
    return result


# --------------------------------------------------------------------------
# sample
# --------------------------------------------------------------------------


def cmd_sample(args: argparse.Namespace) -> dict:
    extra = load_extra_args(args)
    points = extra.get("points") or []
    regions = extra.get("regions") or []
    if len(points) + len(regions) > 64:
        raise VisionError(
            f"too many sample items: {len(points) + len(regions)} (max 64)",
            hint="split the batch across multiple `sample` calls",
        )
    _, decoded, _ = load_and_prepare(args.image, max_pixels=args.max_pixels)
    h, w = decoded.array.shape[:2]

    point_results = []
    for i, p in enumerate(points):
        x = max(0, min(int(p["x"]), w - 1))
        y = max(0, min(int(p["y"]), h - 1))
        rgb = tuple(int(c) for c in decoded.array[y, x, :3])
        point_results.append({"label": p.get("label", f"point{i}"), "hex": color_lib.to_hex(rgb), "rgb": list(rgb)})

    region_results = []
    for i, r in enumerate(regions):
        box = clamp_box({"x": int(r["x"]), "y": int(r["y"]), "w": int(r["w"]), "h": int(r["h"])}, w, h)
        crop = decoded.array[box["y"] : box["y"] + box["h"], box["x"] : box["x"] + box["w"]]
        colors = color_lib.dominant_colors(crop, top=2)
        background = colors[0][0] if colors else (255, 255, 255)
        foreground = colors[1][0] if len(colors) > 1 else background
        mean_rgb = tuple(int(c) for c in crop[..., :3].reshape(-1, 3).mean(axis=0))
        ratio = color_lib.contrast_ratio(foreground, background)
        region_results.append(
            {
                "label": r.get("label", f"region{i}"),
                "dominant_hex": color_lib.to_hex(background),
                "mean_hex": color_lib.to_hex(mean_rgb),
                "foreground_hex": color_lib.to_hex(foreground),
                "background_hex": color_lib.to_hex(background),
                "contrast_ratio": ratio,
                "wcag": color_lib.wcag_levels(ratio),
            }
        )
    return {"points": point_results, "regions": region_results, "truncated": False}


# --------------------------------------------------------------------------
# measure
# --------------------------------------------------------------------------


def cmd_measure(args: argparse.Namespace) -> dict:
    extra = load_extra_args(args)
    if args.image:
        resolve_path(args.image, must_exist=True)

    distances = []
    for d in extra.get("distances") or []:
        result = measure_lib.distance(d["from"], d["to"])
        if "label" in d:
            result["label"] = d["label"]
        distances.append(result)

    gaps = []
    gap_values: list[float] = []
    for g in extra.get("gaps") or []:
        result = measure_lib.gap(g["a"], g["b"])
        if "label" in g:
            result["label"] = g["label"]
        gaps.append(result)
        gap_values.extend([result["horizontal"], result["vertical"]])

    alignment_cfg = extra.get("alignment") or {}
    groups = []
    if alignment_cfg.get("boxes"):
        groups = measure_lib.alignment_groups(alignment_cfg["boxes"], tolerance=alignment_cfg.get("tolerance", 2))

    spacing_scale = extra.get("spacing_scale", 8)
    rhythm = measure_lib.spacing_rhythm([v for v in gap_values if v > 0], spacing_scale=spacing_scale)

    return {"distances": distances, "gaps": gaps, "alignment": {"groups": groups}, "rhythm": rhythm}


# --------------------------------------------------------------------------
# regions — the CV core
# --------------------------------------------------------------------------


class _RegionBuilder:
    def __init__(self, decoded: decode_lib.Decoded, ds: decode_lib.Downscaled, roi_offset: tuple[int, int], max_regions: int, text_metrics: bool):
        self.decoded = decoded
        self.ds = ds
        self.roi_x, self.roi_y = roi_offset
        self.max_regions = max_regions
        self.text_metrics = text_metrics
        self.regions: list[dict] = []
        self.text: list[dict] = []
        self.truncated = False
        self.total_area = decoded.width * decoded.height

    def visit(self, node: segment_lib.Node, parent_id: str | None, depth: int, min_area: int) -> None:
        if self.truncated:
            return
        x, y, w, h = node.box
        original = self.ds.box_to_original(x, y, w, h)
        original["x"] += self.roi_x
        original["y"] += self.roi_y
        area = original["w"] * original["h"]
        if area < min_area:
            return  # prune this whole (tiny) subtree — noise, not a region
        if len(self.regions) >= self.max_regions:
            self.truncated = True
            return

        region_id = f"r{len(self.regions) + 1}"
        crop = self._crop_original(original)
        bg_rgb = color_lib.dominant_colors(crop, top=1)
        bg_rgb = bg_rgb[0][0] if bg_rgb else (255, 255, 255)
        border = segment_lib.estimate_border(self.decoded.array, original)
        outside_rgb = self._outside_color(original)
        radius = segment_lib.estimate_radius(self.decoded.array, original, bg_rgb, outside_rgb)

        entry = {
            "id": region_id,
            "parent_id": parent_id,
            "depth": depth,
            "box": original,
            "bg_hex": color_lib.to_hex(bg_rgb),
            "border": border,
            "radius_estimate": radius,
            "area_ratio": round(area / self.total_area, 4) if self.total_area else 0.0,
            "text_ids": [],
        }
        self.regions.append(entry)

        if self.text_metrics and node.leaf_kind == "texty" and not node.children:
            metrics = text_mask_lib.measure_text_region(crop)
            if metrics is not None:
                text_id = f"t{len(self.text) + 1}"
                metrics["id"] = text_id
                metrics["box"] = {
                    "x": metrics["box"]["x"] + original["x"],
                    "y": metrics["box"]["y"] + original["y"],
                    "w": metrics["box"]["w"],
                    "h": metrics["box"]["h"],
                }
                self.text.append(metrics)
                entry["text_ids"].append(text_id)

        for child in node.children:
            self.visit(child, region_id, depth + 1, min_area)

    def _crop_original(self, box: dict):
        h, w = self.decoded.array.shape[:2]
        clamped = clamp_box(box, w, h)
        return self.decoded.array[clamped["y"] : clamped["y"] + clamped["h"], clamped["x"] : clamped["x"] + clamped["w"]]

    def _outside_color(self, box: dict) -> tuple[int, int, int]:
        h, w = self.decoded.array.shape[:2]
        x, y = box["x"], box["y"]
        if y > 0:
            return tuple(int(c) for c in self.decoded.array[y - 1, min(x, w - 1), :3])
        if x > 0:
            return tuple(int(c) for c in self.decoded.array[min(y, h - 1), x - 1, :3])
        return color_lib.background_color(self.decoded.array)


def cmd_regions(args: argparse.Namespace) -> dict:
    roi = json.loads(args.roi_json) if args.roi_json else None
    path, decoded, _ = load_and_prepare(args.image, max_pixels=args.max_pixels, max_long_side=1600)

    roi_x = roi_y = 0
    array = decoded.array
    if roi:
        clamped = clamp_box(roi, decoded.width, decoded.height)
        roi_x, roi_y = clamped["x"], clamped["y"]
        array = array[clamped["y"] : clamped["y"] + clamped["h"], clamped["x"] : clamped["x"] + clamped["w"]]

    sub_decoded = decode_lib.Decoded(array=array, width=array.shape[1], height=array.shape[0], format=decoded.format, bytes=decoded.bytes)
    ds = decode_lib.downscale(sub_decoded, max_pixels=args.max_pixels, max_long_side=1600)

    time_budget = budget_lib.TimeBudget(seconds=args.time_budget)
    tree = segment_lib.segment(ds.array, min_area=1, max_depth=7, min_gap=2, color_bits=args.quantize, time_budget=time_budget)

    builder = _RegionBuilder(decoded, ds, (roi_x, roi_y), max_regions=args.max_regions, text_metrics=args.text_metrics)
    for child in tree.children:
        if builder.truncated:
            break
        builder.visit(child, None, 1, args.min_area)

    truncated = builder.truncated or time_budget.expired
    payload = {"regions": builder.regions, "text": builder.text, "truncated": truncated}
    out_path = write_out(args.out, payload)
    return {
        "region_count": len(builder.regions),
        "text_count": len(builder.text),
        "truncated": truncated,
        "out": rel(out_path) if out_path else None,
    }


# --------------------------------------------------------------------------
# crop
# --------------------------------------------------------------------------


def _quadrant_box(quadrant: str, width: int, height: int) -> dict:
    half_w, half_h = width // 2, height // 2
    boxes = {
        "tl": {"x": 0, "y": 0, "w": half_w, "h": half_h},
        "tr": {"x": half_w, "y": 0, "w": width - half_w, "h": half_h},
        "bl": {"x": 0, "y": half_h, "w": half_w, "h": height - half_h},
        "br": {"x": half_w, "y": half_h, "w": width - half_w, "h": height - half_h},
    }
    return boxes[quadrant]


def cmd_crop(args: argparse.Namespace) -> dict:
    path, decoded, _ = load_and_prepare(args.image, max_pixels=args.max_pixels)
    if args.box:
        box = parse_box(args.box)
    else:
        box = _quadrant_box(args.quadrant, decoded.width, decoded.height)
    box = clamp_box(box, decoded.width, decoded.height)

    crop = decoded.array[box["y"] : box["y"] + box["h"], box["x"] : box["x"] + box["w"]]
    img = Image.fromarray(crop)
    if args.scale != 1.0:
        new_size = (max(1, round(img.width * args.scale)), max(1, round(img.height * args.scale)))
        img = img.resize(new_size, Image.LANCZOS)
    if max(img.width, img.height) > args.max_side:
        fit_scale = args.max_side / max(img.width, img.height)
        new_size = (max(1, round(img.width * fit_scale)), max(1, round(img.height * fit_scale)))
        img = img.resize(new_size, Image.LANCZOS)

    out_path = resolve_path(args.out) if args.out else default_artifact_path(path, "crop")
    out_path.parent.mkdir(parents=True, exist_ok=True)
    img.save(out_path, format="PNG")

    return {
        "box": box,
        "output_size": {"width": img.width, "height": img.height},
        "artifacts": [{"kind": "crop", "path": rel(out_path)}],
    }


# --------------------------------------------------------------------------
# compare
# --------------------------------------------------------------------------


def cmd_compare(args: argparse.Namespace) -> dict:
    path_a, decoded_a, _ = load_and_prepare(args.image, max_pixels=args.max_pixels)
    _, decoded_b, _ = load_and_prepare(args.reference, max_pixels=args.max_pixels)

    ds_a = decode_lib.downscale(decoded_a, max_pixels=args.max_pixels)
    ds_b = decode_lib.downscale(decoded_b, max_pixels=args.max_pixels)

    if args.fit == "none" and ds_a.array.shape[:2] != ds_b.array.shape[:2]:
        raise VisionError(
            f"image/reference size mismatch: {ds_a.array.shape[:2]} vs {ds_b.array.shape[:2]} with --fit none",
            hint="use --fit scale (default), or pass a reference with matching dimensions",
        )
    a_arr, b_arr = diff_lib.resize_to_match(ds_a.array, ds_b.array, fit=args.fit)

    mask, ratio = diff_lib.pixel_diff(a_arr, b_arr, tolerance=args.tolerance)
    regions, region_truncated = diff_lib.diff_regions(mask, a_arr, b_arr, min_area=9, max_regions=args.max_diff_regions)
    region_boxes = [ds_a.box_to_original(r["box"]["x"], r["box"]["y"], r["box"]["w"], r["box"]["h"]) for r in regions]
    for r, original_box in zip(regions, region_boxes):
        r["box"] = original_box

    score = round(max(0.0, 1.0 - ratio), 4)
    artifacts = None
    if args.heatmap:
        heatmap = diff_lib.render_heatmap(a_arr, mask)
        artifact_path = default_artifact_path(path_a, "heatmap")
        heatmap.save(artifact_path, format="PNG")
        artifacts = [{"kind": "heatmap", "path": rel(artifact_path)}]

    payload = {
        "size_a": {"width": decoded_a.width, "height": decoded_a.height},
        "size_b": {"width": decoded_b.width, "height": decoded_b.height},
        "diff_ratio": round(ratio, 4),
        "score": score,
        "diff_regions": regions,
        "artifacts": artifacts,
        "truncated": region_truncated,
    }
    out_path = write_out(args.out, payload)
    return {
        "diff_ratio": payload["diff_ratio"],
        "score": score,
        "diff_region_count": len(regions),
        "truncated": region_truncated,
        "out": rel(out_path) if out_path else None,
    }


# --------------------------------------------------------------------------
# contrast
# --------------------------------------------------------------------------


def cmd_contrast(args: argparse.Namespace) -> dict:
    extra = load_extra_args(args)
    pairs = extra.get("pairs") or []
    if len(pairs) > 32:
        raise VisionError(f"too many pairs: {len(pairs)} (max 32)", hint="split the batch across multiple `contrast` calls")

    results = []
    for i, pair in enumerate(pairs):
        try:
            fg = color_lib.hex_to_rgb(pair["foreground"])
            bg = color_lib.hex_to_rgb(pair["background"])
        except (KeyError, ValueError) as exc:
            raise VisionError(
                f"pair {i} has an invalid color: {exc}",
                hint='colors must be hex strings, e.g. {"foreground": "#111827", "background": "#FFFFFF"}',
            ) from exc
        ratio = color_lib.contrast_ratio(fg, bg)
        levels = color_lib.wcag_levels(ratio, font_px=pair.get("font_px"), bold=bool(pair.get("bold")))
        entry = {"label": pair.get("label", f"pair{i}"), "ratio": ratio, **levels}
        if not levels["aa"]:
            entry["suggested_foreground"] = color_lib.suggest_foreground(bg)
        results.append(entry)

    payload = {"results": results}
    out_path = write_out(args.out, payload)
    if out_path:
        return {"pair_count": len(results), "out": rel(out_path)}
    return payload


# --------------------------------------------------------------------------
# CLI wiring
# --------------------------------------------------------------------------


class JSONArgumentParser(argparse.ArgumentParser):
    """argparse's default `error()` prints plain-text usage to stderr and
    exits — that breaks the "stdout is ALWAYS JSON" contract for the most
    common mistake an agent makes (wrong flag, missing required argument).
    This subclass keeps that same contract for argument errors too."""

    def error(self, message: str) -> None:
        emit({"error": f"argument error: {message}", "hint": f"run `{self.prog} -h` for the full argument list"})
        raise SystemExit(2)


def build_parser() -> argparse.ArgumentParser:
    parser = JSONArgumentParser(prog="vision.py", description="Deterministic pixel measurement for UI screenshots — see reference/vision-cli.md")
    sub = parser.add_subparsers(dest="command", required=True, parser_class=JSONArgumentParser)

    p_probe = sub.add_parser("probe", help="dimensions, palette, background color — the first call")
    add_image_args(p_probe)
    p_probe.add_argument("--palette-size", type=int, default=8)
    p_probe.add_argument("--grid-step", type=int, default=None)
    p_probe.add_argument("--out", default=None)
    p_probe.set_defaults(func=cmd_probe)

    p_sample = sub.add_parser("sample", help="batch color sampling at points/regions")
    add_image_args(p_sample)
    add_batch_args(p_sample, required=True)
    p_sample.add_argument("--out", default=None)
    p_sample.set_defaults(func=cmd_sample)

    p_measure = sub.add_parser("measure", help="batch distances/gaps/alignment/rhythm")
    p_measure.add_argument("--image", required=False, help="path relative to /workspace (optional, existence-checked only)")
    add_batch_args(p_measure, required=True)
    p_measure.add_argument("--out", default=None)
    p_measure.set_defaults(func=cmd_measure)

    p_regions = sub.add_parser("regions", help="segment the image into a block tree + text metrics")
    add_image_args(p_regions)
    p_regions.add_argument("--roi", dest="roi_json", default=None, help='JSON box, e.g. \'{"x":0,"y":0,"w":800,"h":600}\'')
    p_regions.add_argument("--min-area", type=int, default=400)
    p_regions.add_argument("--max-regions", type=int, default=120)
    p_regions.add_argument("--text-metrics", dest="text_metrics", action=argparse.BooleanOptionalAction, default=True)
    p_regions.add_argument("--quantize", type=int, default=5)
    p_regions.add_argument("--time-budget", type=float, default=20.0)
    p_regions.add_argument("--out", required=True)
    p_regions.set_defaults(func=cmd_regions)

    p_crop = sub.add_parser("crop", help="cut/scale a fragment, write it as a PNG artifact")
    add_image_args(p_crop)
    box_group = p_crop.add_mutually_exclusive_group(required=True)
    box_group.add_argument("--box", default=None, help="x,y,w,h")
    box_group.add_argument("--quadrant", choices=["tl", "tr", "bl", "br"], default=None)
    p_crop.add_argument("--scale", type=float, default=1.0)
    p_crop.add_argument("--max-side", type=int, default=1280)
    p_crop.add_argument("--out", default=None, help="artifact PNG path (default: design/artifacts/<image>-crop.png)")
    p_crop.set_defaults(func=cmd_crop)

    p_compare = sub.add_parser("compare", help="pixel diff between a mockup and an implementation screenshot")
    add_image_args(p_compare)
    p_compare.add_argument("--reference", required=True, help="path to the mockup, relative to /workspace")
    p_compare.add_argument("--tolerance", type=int, default=8)
    p_compare.add_argument("--fit", choices=["scale", "none"], default="scale")
    p_compare.add_argument("--heatmap", dest="heatmap", action=argparse.BooleanOptionalAction, default=True)
    p_compare.add_argument("--max-diff-regions", type=int, default=20)
    p_compare.add_argument("--out", required=True)
    p_compare.set_defaults(func=cmd_compare)

    p_contrast = sub.add_parser("contrast", help="pure WCAG contrast arithmetic, no image")
    add_batch_args(p_contrast, required=True)
    p_contrast.add_argument("--out", default=None)
    p_contrast.set_defaults(func=cmd_contrast)

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        result = args.func(args)
    except VisionError as exc:
        emit({"error": str(exc), "hint": exc.hint})
        return 1
    except budget_lib.BudgetError as exc:
        emit({"error": str(exc), "hint": exc.hint})
        return 1
    except Exception as exc:  # noqa: BLE001 - last-resort JSON error contract
        emit(
            {
                "error": f"unexpected error: {exc}",
                "hint": "retry with a smaller image or --max-pixels, then check the traceback on stderr",
            }
        )
        import traceback

        traceback.print_exc(file=sys.stderr)
        return 2
    emit(result)
    return 0


if __name__ == "__main__":
    sys.exit(main())
