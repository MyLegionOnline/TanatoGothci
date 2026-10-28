# `vision.py` — CLI reference

One entry point, seven subcommands. Every invocation prints JSON to stdout —
on success and on failure alike — and every path argument is relative to the
workspace root (`/workspace`); a path that escapes it is rejected. Errors are
always `{"error": "...", "hint": "..."}` with a non-zero exit code, and the
`hint` is a runnable example — if a call fails, fix it from the hint before
trying anything else.

Large results (`regions`, `compare`) require `--out FILE`: the full payload
is written there, and stdout gets a short summary instead. Do not `read_file`
a `--out` file unless you actually need the detail — the summary already has
the counts you usually need to decide what to do next.

Invoke every subcommand as:
```bash
python3 .legion/skills/ui-from-design/scripts/vision.py <subcommand> [flags]
```

## `probe` — first call, orientation

```bash
vision.py probe --image design/dashboard.png [--palette-size 8] [--grid-step N] [--out FILE]
```
Returns `{width, height, format, bytes, aspect_ratio, palette: [{hex, rgb,
coverage}], background_hex}`. `--grid-step` overlays a coordinate grid and
writes it as a PNG artifact (`artifacts: [{kind: "grid", path}]`) — only
useful for a human looking at `design/artifacts/` in `RepoPanel`, or when you
need to eyeball a region that didn't segment cleanly. Coordinates you act on
should still come from `regions`/`measure`, not from reading pixels off a
grid image yourself.

## `sample` — batch color sampling

```bash
vision.py sample --image design/dashboard.png --json '{"points": [{"x":10,"y":10,"label":"bg"}], "regions": [{"x":32,"y":96,"w":320,"h":180,"label":"card"}]}'
```
`--json` or `--in FILE` (mutually exclusive) carries the batch — up to 64
points+regions combined. Point results: `{label, hex, rgb}`. Region results:
`{label, dominant_hex, mean_hex, foreground_hex, background_hex,
contrast_ratio, wcag: {aa, aa_large, aaa}}`.

## `measure` — batch distances / gaps / alignment / rhythm

```bash
vision.py measure --json '{
  "distances": [{"from": {"x":0,"y":0}, "to": {"x":100,"y":0}, "label": "header-width"}],
  "gaps": [{"a": {"x":0,"y":0,"w":50,"h":50}, "b": {"x":80,"y":0,"w":50,"h":50}, "label": "card-gap"}],
  "alignment": {"boxes": [{"x":32,"y":0,"w":100,"h":20,"label":"a"}, {"x":32,"y":40,"w":100,"h":20,"label":"b"}], "tolerance": 2},
  "spacing_scale": 8
}'
```
No image is decoded — this is pure arithmetic on the boxes you pass. Returns
`distances` (px, dx, dy), `gaps` (horizontal/vertical), `alignment.groups`
(`left|right|top|bottom|center_x|center_y` clusters + members + max
deviation), and a `rhythm` summary of observed gaps against `spacing_scale`.

## `regions` — the CV core (run this before every `extract_design_spec`/`critique_design`)

```bash
vision.py regions --image images/dashboard.png --out design/dashboard.cv.json \
  [--roi '{"x":0,"y":0,"w":800,"h":600}'] [--min-area 400] [--max-regions 120] \
  [--no-text-metrics] [--quantize 5] [--time-budget 20]
```
`--out` is **required**. stdout gets `{region_count, text_count, truncated,
out}`; the file holds the real payload:
```jsonc
{
  "regions": [{"id": "r3", "parent_id": "r1", "depth": 2,
               "box": {"x":32,"y":96,"w":320,"h":180},
               "bg_hex": "#FFFFFF", "border": {"hex": "#E5E7EB", "width": 1},
               "radius_estimate": 8, "area_ratio": 0.04, "text_ids": ["t7"]}],
  "text": [{"id": "t7", "box": {...}, "glyph_height_px": 16, "cap_height_px": 11,
            "line_height_px": 24, "lines": 1, "align": "left",
            "color_hex": "#111827", "background_hex": "#FFFFFF",
            "contrast_ratio": 16.1, "weight_guess": "regular"}],
  "truncated": false
}
```
`text[]` never carries the string content of a line — only its geometry and
color. Content comes from `extract_design_spec`/`describe_image`, which read
the actual pixels through a vision model. Pass this file's path as `cv_path`
to either MCP tool; Hub substitutes its parsed content for you.

## `crop` — cut/scale a fragment to a PNG artifact

```bash
vision.py crop --image images/dashboard.png --box 32,96,320,180 [--scale 2] [--max-side 1280] [--out design/artifacts/card.png]
# or: --quadrant tl|tr|bl|br instead of --box
```
Returns `{box, output_size, artifacts: [{kind: "crop", path}]}`. Use this to
narrow a region before `describe_image` when something in `uncertain` needs a
closer look — never paste the whole mockup back through the model for a
detail question.

## `compare` — mockup vs. implementation screenshot

```bash
vision.py compare --image <screenshot.png> --reference images/dashboard.png \
  --out design/dashboard.diff.json [--tolerance 8] [--fit scale|none] \
  [--no-heatmap] [--max-diff-regions 20]
```
`--out` is **required**. stdout gets `{diff_ratio, score, diff_region_count,
truncated, out}`; the file holds `{size_a, size_b, diff_ratio, score,
diff_regions: [{box, diff_ratio, kind: "color"|"position"|"missing"|"extra"}],
artifacts: [{kind: "heatmap", path}], truncated}`. Feed this file's path to
`check_fidelity.py --diff` for the readiness verdict, or to `critique_design`
as `diff_path` for its `fidelity` score.

## `contrast` — pure WCAG arithmetic, no image

```bash
vision.py contrast --json '{"pairs": [{"foreground": "#111827", "background": "#FFFFFF", "label": "body"}]}'
```
Up to 32 pairs. Returns `{ratio, aa, aa_large, aaa, suggested_foreground?}`
per pair (`suggested_foreground` only when `aa` fails).

## `check_fidelity.py` — the readiness verdict

Not a `vision.py` subcommand — a separate script, same JSON-always contract:

```bash
python3 .legion/skills/ui-from-design/scripts/check_fidelity.py \
  --diff design/dashboard.diff.json [--threshold 0.10] \
  [--critique design/dashboard.critique.json] \
  [--explain '[{"index": 0, "reason": "photo placeholder"}]']
```
Returns `{ready, diff_ratio, threshold, diff_region_count,
unexplained_region_count, unexplained_regions, explained, truncated,
blockers, reasons}`. See SKILL.md's **Readiness criteria** for what `ready`
actually means and when an above-threshold result is still closeable.
