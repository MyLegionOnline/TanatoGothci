#!/usr/bin/env python3
"""check_fidelity.py — deterministic readiness verdict for the ui-from-design
verification loop (design_vision_toolkit_plan.md Этап 7.1.1 / 7.1.3).

Reads the diff report `vision.py compare --out ...` wrote, and optionally a
critique_design result the agent saved to a file, and returns one
``{"ready": bool, ...}`` verdict. The point is that "is this done?" stops
being a re-judgment call the model makes fresh every iteration — the same
two numbers (diff_ratio, blocker count) always produce the same answer.

What this script does NOT decide (7.1.3's third readiness condition): whether
a color or spacing value was hardcoded past an existing design_system token.
That requires reading the implementation source against `design_system`, which
is the agent's own job, not something derivable from a diff/critique JSON.

Same JSON-only contract as vision.py (Р2): stdout is always JSON, errors are
``{"error": "...", "hint": "..."}`` with a non-zero exit code, and paths are
resolved relative to the workspace root (the current working directory) with
no escaping via ``..``.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


class FidelityError(Exception):
    def __init__(self, message: str, hint: str):
        super().__init__(message)
        self.hint = hint


def resolve_path(raw: str) -> Path:
    root = Path.cwd().resolve()
    candidate = (root / raw).resolve()
    try:
        candidate.relative_to(root)
    except ValueError as exc:
        raise FidelityError(
            f"path escapes workspace root: {raw!r}",
            hint="pass a path relative to /workspace, without '..' segments, e.g. design/dashboard.diff.json",
        ) from exc
    if not candidate.is_file():
        raise FidelityError(
            f"file not found: {raw!r}",
            hint=f"check the path is relative to /workspace and exists, e.g. design/{Path(raw).name}",
        )
    return candidate


def load_json_object(raw: str, *, field_name: str) -> dict:
    path = resolve_path(raw)
    try:
        parsed = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise FidelityError(
            f"invalid JSON in {field_name} {raw!r}: {exc}",
            hint=f"{field_name} must point at a JSON object written by vision.py or the MCP tool",
        ) from exc
    if not isinstance(parsed, dict):
        raise FidelityError(
            f"{field_name} {raw!r} must contain a JSON object",
            hint=f"{field_name} must point at a JSON object written by vision.py or the MCP tool",
        )
    return parsed


def parse_explain(raw: str | None, in_file: str | None) -> dict[int, str]:
    """``--explain``/``--explain-in`` — a JSON array of diff_region indices
    (0-based, into the `diff_regions` list of the --diff file) the caller
    asserts are explainable (font substitution, photo placeholder,
    anti-aliasing — 7.1.3), each item either a bare integer or
    ``{"index": int, "reason": str}``. Recording the reason in the verdict
    is what "и это записано в отчёте" (7.1.3) means here."""
    if raw is None and in_file is None:
        return {}
    text = resolve_path(in_file).read_text(encoding="utf-8") if in_file else raw
    try:
        items = json.loads(text)
    except json.JSONDecodeError as exc:
        raise FidelityError(
            f"invalid JSON in --explain: {exc}",
            hint='pass a JSON array, e.g. --explain \'[0, {"index": 2, "reason": "photo placeholder"}]\'',
        ) from exc
    if not isinstance(items, list):
        raise FidelityError(
            "--explain must be a JSON array",
            hint='pass a JSON array, e.g. --explain \'[0, {"index": 2, "reason": "photo placeholder"}]\'',
        )
    explained: dict[int, str] = {}
    for item in items:
        if isinstance(item, bool):
            raise FidelityError("--explain entries must be an index or {index, reason}", hint="use an integer or an object with an integer 'index'")
        if isinstance(item, int):
            explained[item] = "unspecified"
        elif isinstance(item, dict) and isinstance(item.get("index"), int):
            explained[item["index"]] = str(item.get("reason") or "unspecified")
        else:
            raise FidelityError(
                "--explain entries must be an index or {index, reason}",
                hint='each entry is an integer or {"index": int, "reason": "..."}',
            )
    return explained


def build_verdict(
    diff: dict,
    *,
    threshold: float,
    explained: dict[int, str],
    critique: dict | None,
) -> dict:
    diff_ratio = diff.get("diff_ratio")
    if not isinstance(diff_ratio, (int, float)):
        raise FidelityError(
            "--diff file has no numeric diff_ratio",
            hint="the file must be the --out payload of `vision.py compare`, not its stdout summary",
        )
    regions = diff.get("diff_regions")
    regions = regions if isinstance(regions, list) else []
    truncated = bool(diff.get("truncated"))

    for index in explained:
        if index < 0 or index >= len(regions):
            raise FidelityError(
                f"--explain references region index {index}, but --diff only has {len(regions)} diff_regions",
                hint="indices are 0-based positions into the diff file's diff_regions array",
            )

    unexplained_regions = [
        {**regions[i], "index": i} for i in range(len(regions)) if i not in explained
    ]
    within_threshold = diff_ratio <= threshold
    # 7.1.3: closing above-threshold work is allowed only when every single
    # diff_region has a recorded explanation AND the region list itself is
    # complete — a truncated list means some disagreement was never listed,
    # so there is nothing honest to declare "all explained".
    all_explained = bool(regions) and not truncated and len(unexplained_regions) == 0
    fidelity_ready = within_threshold or all_explained

    reasons: list[str] = []
    if within_threshold:
        reasons.append(f"diff_ratio {diff_ratio} <= threshold {threshold}")
    elif all_explained:
        reasons.append(
            f"diff_ratio {diff_ratio} > threshold {threshold}, but all {len(regions)} diff_regions are explained"
        )
    else:
        reasons.append(f"diff_ratio {diff_ratio} > threshold {threshold}")
        if regions:
            reasons.append(f"{len(unexplained_regions)}/{len(regions)} diff_regions have no recorded explanation")
        if truncated:
            reasons.append("diff_regions list is truncated — re-run `vision.py compare` with a higher --max-diff-regions before declaring the rest explained")

    blockers: list[dict] = []
    if critique is not None:
        findings = critique.get("findings")
        findings = findings if isinstance(findings, list) else []
        blockers = [f for f in findings if isinstance(f, dict) and f.get("severity") == "blocker"]
        if blockers:
            reasons.append(f"critique_design reported {len(blockers)} blocker finding(s)")

    ready = fidelity_ready and not blockers

    return {
        "ready": ready,
        "diff_ratio": diff_ratio,
        "threshold": threshold,
        "diff_region_count": len(regions),
        "unexplained_region_count": len(unexplained_regions),
        "unexplained_regions": unexplained_regions,
        "explained": [{"index": i, "reason": reason} for i, reason in sorted(explained.items())],
        "truncated": truncated,
        "blockers": blockers,
        "reasons": reasons,
    }


class JSONArgumentParser(argparse.ArgumentParser):
    """Keeps the "stdout is ALWAYS JSON" contract for argument errors too —
    argparse's default ``error()`` prints plain-text usage and exits instead
    (same fix as vision.py's parser)."""

    def error(self, message: str) -> None:
        print(json.dumps({"error": f"argument error: {message}", "hint": f"run `{self.prog} -h` for the full argument list"}, ensure_ascii=False))
        raise SystemExit(2)


def build_parser() -> argparse.ArgumentParser:
    parser = JSONArgumentParser(
        prog="check_fidelity.py",
        description="Deterministic ready/not-ready verdict from a vision.py compare report — see SKILL.md §criteria.",
    )
    parser.add_argument("--diff", required=True, help="path to the --out payload of `vision.py compare`")
    parser.add_argument("--threshold", type=float, default=0.10, help="max acceptable diff_ratio (default matches SKILL.md's 10%%)")
    parser.add_argument("--critique", default=None, help="path to a saved critique_design result — enables the blocker check")
    explain_group = parser.add_mutually_exclusive_group()
    explain_group.add_argument("--explain", dest="explain_json", default=None, help='inline JSON array, e.g. \'[0, {"index":2,"reason":"photo placeholder"}]\'')
    explain_group.add_argument("--explain-in", dest="explain_in", default=None, help="path to a JSON file with the same array shape as --explain")
    parser.add_argument("--out", default=None, help="optional path to also write the verdict to (stdout always gets it too)")
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        diff = load_json_object(args.diff, field_name="--diff")
        critique = load_json_object(args.critique, field_name="--critique") if args.critique else None
        explained = parse_explain(args.explain_json, args.explain_in)
        verdict = build_verdict(diff, threshold=args.threshold, explained=explained, critique=critique)
    except FidelityError as exc:
        print(json.dumps({"error": str(exc), "hint": exc.hint}, ensure_ascii=False))
        return 1
    except Exception as exc:  # noqa: BLE001 - last-resort JSON error contract
        print(json.dumps({"error": f"unexpected error: {exc}", "hint": "check --diff/--critique point at valid JSON files"}, ensure_ascii=False))
        import traceback

        traceback.print_exc(file=sys.stderr)
        return 2

    if args.out:
        out_path = resolve_path_for_write(args.out)
        out_path.write_text(json.dumps(verdict, indent=2, ensure_ascii=False), encoding="utf-8")

    print(json.dumps(verdict, ensure_ascii=False))
    # Matches vision.py's contract (Р2): exit code reflects whether the
    # script ran successfully, not the semantic content of its answer.
    # "not ready" is a normal, well-formed result — callers read `ready`
    # from the JSON, the same way they already read `truncated`/`error`.
    return 0


def resolve_path_for_write(raw: str) -> Path:
    root = Path.cwd().resolve()
    candidate = (root / raw).resolve()
    try:
        candidate.relative_to(root)
    except ValueError as exc:
        raise FidelityError(
            f"path escapes workspace root: {raw!r}",
            hint="pass a path relative to /workspace, without '..' segments",
        ) from exc
    candidate.parent.mkdir(parents=True, exist_ok=True)
    return candidate


if __name__ == "__main__":
    sys.exit(main())
