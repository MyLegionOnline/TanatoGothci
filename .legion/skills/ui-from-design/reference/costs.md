# Costs — what's free and what's billed

This skill straddles two different economics on purpose
(design_vision_toolkit_plan.md Р2/Р5): the measuring half runs on the room's
own workspace container and costs nothing per call; the understanding half
calls an external vision provider through `mcp-vision` and is billed per
call. Mixing them up in either direction wastes money or wastes accuracy —
know which one you're calling.

## Free — `scripts/vision.py` and `scripts/check_fidelity.py`

Every subcommand (`probe`, `sample`, `measure`, `regions`, `crop`, `compare`,
`contrast`) and `check_fidelity.py` run locally via `bash`, on this room's own
CPU/RAM. **No credits are spent, no matter how many times you call them.**
That is a deliberate design choice, not an oversight: if measuring cost
credits, you'd be tempted to skip a `measure`/`compare` call exactly when it
would catch a real problem. Call them as often as the work needs — batch
`sample`/`measure`/`contrast` inputs to save your own time and the room's
memory budget, not to save money, because there's no money to save there.

## Billed — the three `mcp-vision` MCP tools

| Tool | Price | Why |
|---|---|---|
| `describe_image` | 500 mcr | one external vision-provider call |
| `critique_design` | 700 mcr | provider call + a larger structured output |
| `extract_design_spec` | 900 mcr | provider call + the full Design Spec |

Spend these deliberately:

- **`extract_design_spec` — once per mockup.** Call it again only if the
  mockup itself changed. Always run `vision.py regions` first — skipping it
  doesn't fail the call, it just quietly degrades the coordinates from
  measured to guessed (`"cv_source": "model_estimated"`).
- **`describe_image` — only for a specific `uncertain` node**, or for a
  `diff_regions` entry the numbers alone don't explain. Narrow the image
  with `vision.py crop` first — don't hand the model the full mockup again
  when you only need one detail.
- **`critique_design` — once before closing the task**, not once per
  iteration. Call it again mid-loop only if `diff_ratio` stops improving
  between iterations and you need a second signal on what's actually wrong.

## A full cycle, roughly

`extract_design_spec` (900) + a couple of iterations' worth of `describe_image`
calls for genuinely uncertain details (500 each) + one `critique_design`
before closing (700) — on the order of **2,000–2,500 mcr** per mockup. Every
`vision.py`/`check_fidelity.py` call in between (segmenting, measuring,
comparing, checking readiness) is free, however many iterations it takes.
