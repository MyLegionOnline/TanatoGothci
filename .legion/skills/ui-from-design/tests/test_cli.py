"""End-to-end CLI contract checks (Этап 1.5 acceptance): every subcommand
returns valid JSON on both success and failure, paths stay inside the
workspace root, and `--out` writes the full payload while stdout stays a
short summary (Р4/S3)."""
import json
import subprocess
import sys
from pathlib import Path

import pytest

from fixtures import dashboard_image

VISION_PY = Path(__file__).resolve().parents[1] / "scripts" / "vision.py"


def run(cwd: Path, *args: str) -> tuple[int, dict]:
    proc = subprocess.run(
        [sys.executable, str(VISION_PY), *args],
        cwd=cwd,
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert proc.stdout.strip(), f"expected JSON on stdout, got nothing. stderr={proc.stderr}"
    try:
        payload = json.loads(proc.stdout)
    except json.JSONDecodeError:
        pytest.fail(f"stdout was not valid JSON: {proc.stdout!r}\nstderr={proc.stderr}")
    return proc.returncode, payload


@pytest.fixture()
def workspace(tmp_path):
    (tmp_path / "design").mkdir()
    dashboard_image(tmp_path / "design" / "dashboard.png")
    return tmp_path


def test_probe_success_is_json(workspace):
    code, payload = run(workspace, "probe", "--image", "design/dashboard.png")
    assert code == 0
    assert payload["width"] == 400
    assert payload["height"] == 300
    assert "error" not in payload


def test_regions_writes_out_file_and_summarizes_on_stdout(workspace):
    code, payload = run(workspace, "regions", "--image", "design/dashboard.png", "--out", "design/dashboard.cv.json", "--min-area", "100")
    assert code == 0
    assert payload["out"] == "design/dashboard.cv.json"
    assert payload["region_count"] > 0
    # the summary must stay small — the full tree lives in the file, not stdout
    assert len(json.dumps(payload)) < 500

    out_file = workspace / "design" / "dashboard.cv.json"
    assert out_file.is_file()
    full = json.loads(out_file.read_text())
    assert len(full["regions"]) == payload["region_count"]


def test_missing_file_is_a_json_error_with_hint(workspace):
    code, payload = run(workspace, "probe", "--image", "design/does-not-exist.png")
    assert code != 0
    assert "error" in payload
    assert payload["hint"]


def test_missing_required_flag_is_a_json_error_not_argparse_usage_text(workspace):
    code, payload = run(workspace, "regions", "--image", "design/dashboard.png")
    assert code != 0
    assert "error" in payload
    assert "--out" in payload["error"]


def test_path_escaping_workspace_root_is_rejected(workspace):
    code, payload = run(workspace, "probe", "--image", "../../etc/passwd")
    assert code != 0
    assert "error" in payload
    assert "escapes workspace root" in payload["error"]


def test_invalid_json_input_is_a_json_error(workspace):
    code, payload = run(workspace, "contrast", "--json", "{not valid")
    assert code != 0
    assert "error" in payload
    assert payload["hint"]


def test_contrast_pure_arithmetic_no_image_flag(workspace):
    code, payload = run(workspace, "contrast", "--json", '{"pairs":[{"foreground":"#000000","background":"#FFFFFF"}]}')
    assert code == 0
    assert payload["results"][0]["ratio"] == 21.0


def test_crop_writes_png_artifact(workspace):
    code, payload = run(workspace, "crop", "--image", "design/dashboard.png", "--box", "32,96,320,180")
    assert code == 0
    artifact = workspace / payload["artifacts"][0]["path"]
    assert artifact.is_file()
    assert artifact.suffix == ".png"


def test_no_subcommand_output_ever_embeds_raw_pixels(workspace):
    """Р4/S1: every image-producing subcommand returns a *path*, never
    base64 — grep the stdout summaries for the tell-tale data URI prefix."""
    _, probe_payload = run(workspace, "probe", "--image", "design/dashboard.png", "--grid-step", "50")
    _, crop_payload = run(workspace, "crop", "--image", "design/dashboard.png", "--box", "0,0,50,50")
    for payload in (probe_payload, crop_payload):
        assert "base64" not in json.dumps(payload)
        assert "data:image" not in json.dumps(payload)
