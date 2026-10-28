"""check_fidelity.py contract checks (Этап 7.1.1/7.1.3): a deterministic
ready/not-ready verdict from a `vision.py compare` report, with the
"all diff_regions explained" override and an optional critique blocker
gate — same JSON-always / non-zero-only-on-error contract as vision.py."""
import json
import subprocess
import sys
from pathlib import Path

import pytest

CHECK_FIDELITY_PY = Path(__file__).resolve().parents[1] / "scripts" / "check_fidelity.py"


def run(cwd: Path, *args: str) -> tuple[int, dict]:
    proc = subprocess.run(
        [sys.executable, str(CHECK_FIDELITY_PY), *args],
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


def write_json(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload), encoding="utf-8")


@pytest.fixture()
def workspace(tmp_path):
    (tmp_path / "design").mkdir()
    return tmp_path


def diff_payload(diff_ratio: float, regions: list[dict] | None = None, truncated: bool = False) -> dict:
    return {
        "size_a": {"width": 400, "height": 300},
        "size_b": {"width": 400, "height": 300},
        "diff_ratio": diff_ratio,
        "score": round(1 - diff_ratio, 4),
        "diff_regions": regions or [],
        "truncated": truncated,
    }


def test_within_threshold_is_ready(workspace):
    write_json(workspace / "design" / "d.diff.json", diff_payload(0.04))
    code, payload = run(workspace, "--diff", "design/d.diff.json")
    assert code == 0
    assert payload["ready"] is True
    assert payload["diff_ratio"] == 0.04


def test_above_threshold_with_unexplained_regions_is_not_ready(workspace):
    write_json(
        workspace / "design" / "d.diff.json",
        diff_payload(0.25, regions=[{"box": {"x": 0, "y": 0, "w": 10, "h": 10}, "diff_ratio": 0.25, "kind": "color"}]),
    )
    code, payload = run(workspace, "--diff", "design/d.diff.json")
    assert code == 0  # not-ready is a normal result, not a script error
    assert payload["ready"] is False
    assert payload["unexplained_region_count"] == 1


def test_above_threshold_but_all_regions_explained_is_ready(workspace):
    write_json(
        workspace / "design" / "d.diff.json",
        diff_payload(0.25, regions=[{"box": {"x": 0, "y": 0, "w": 10, "h": 10}, "diff_ratio": 0.25, "kind": "color"}]),
    )
    code, payload = run(workspace, "--diff", "design/d.diff.json", "--explain", '[{"index": 0, "reason": "photo placeholder"}]')
    assert code == 0
    assert payload["ready"] is True
    assert payload["unexplained_region_count"] == 0
    assert payload["explained"] == [{"index": 0, "reason": "photo placeholder"}]


def test_truncated_diff_regions_blocks_the_all_explained_override(workspace):
    write_json(
        workspace / "design" / "d.diff.json",
        diff_payload(0.25, regions=[{"box": {"x": 0, "y": 0, "w": 10, "h": 10}, "diff_ratio": 0.25, "kind": "color"}], truncated=True),
    )
    code, payload = run(workspace, "--diff", "design/d.diff.json", "--explain", "[0]")
    assert code == 0
    assert payload["ready"] is False
    assert any("truncated" in reason for reason in payload["reasons"])


def test_bare_integer_explain_entries_default_to_unspecified_reason(workspace):
    write_json(
        workspace / "design" / "d.diff.json",
        diff_payload(0.25, regions=[{"box": {"x": 0, "y": 0, "w": 10, "h": 10}, "diff_ratio": 0.25, "kind": "color"}]),
    )
    code, payload = run(workspace, "--diff", "design/d.diff.json", "--explain", "[0]")
    assert code == 0
    assert payload["ready"] is True
    assert payload["explained"] == [{"index": 0, "reason": "unspecified"}]


def test_explain_index_out_of_range_is_a_json_error(workspace):
    write_json(workspace / "design" / "d.diff.json", diff_payload(0.25, regions=[]))
    code, payload = run(workspace, "--diff", "design/d.diff.json", "--explain", "[0]")
    assert code != 0
    assert "error" in payload
    assert payload["hint"]


def test_critique_blocker_forces_not_ready_even_within_threshold(workspace):
    write_json(workspace / "design" / "d.diff.json", diff_payload(0.02))
    write_json(
        workspace / "design" / "d.critique.json",
        {"scores": {"overall": 80}, "findings": [{"aspect": "color", "severity": "blocker", "message": "x", "fix": "y"}], "wcag": [], "summary": "s"},
    )
    code, payload = run(workspace, "--diff", "design/d.diff.json", "--critique", "design/d.critique.json")
    assert code == 0
    assert payload["ready"] is False
    assert len(payload["blockers"]) == 1


def test_critique_without_blockers_stays_ready(workspace):
    write_json(workspace / "design" / "d.diff.json", diff_payload(0.02))
    write_json(
        workspace / "design" / "d.critique.json",
        {"scores": {"overall": 92}, "findings": [{"aspect": "color", "severity": "minor", "message": "x", "fix": "y"}], "wcag": [], "summary": "s"},
    )
    code, payload = run(workspace, "--diff", "design/d.diff.json", "--critique", "design/d.critique.json")
    assert code == 0
    assert payload["ready"] is True


def test_custom_threshold_is_honored(workspace):
    write_json(workspace / "design" / "d.diff.json", diff_payload(0.15))
    code, payload = run(workspace, "--diff", "design/d.diff.json", "--threshold", "0.20")
    assert code == 0
    assert payload["ready"] is True
    assert payload["threshold"] == 0.20


def test_missing_diff_file_is_a_json_error_with_hint(workspace):
    code, payload = run(workspace, "--diff", "design/does-not-exist.json")
    assert code != 0
    assert "error" in payload
    assert payload["hint"]


def test_path_escaping_workspace_root_is_rejected(workspace):
    code, payload = run(workspace, "--diff", "../../etc/passwd")
    assert code != 0
    assert "error" in payload
    assert "escapes workspace root" in payload["error"]


def test_diff_file_without_diff_ratio_is_a_json_error(workspace):
    write_json(workspace / "design" / "bad.json", {"not_a_diff_report": True})
    code, payload = run(workspace, "--diff", "design/bad.json")
    assert code != 0
    assert "error" in payload


def test_missing_required_flag_is_a_json_error_not_argparse_usage_text(workspace):
    code, payload = run(workspace)
    assert code != 0
    assert "error" in payload
    assert "--diff" in payload["error"]


def test_invalid_explain_json_is_a_json_error(workspace):
    write_json(workspace / "design" / "d.diff.json", diff_payload(0.02))
    code, payload = run(workspace, "--diff", "design/d.diff.json", "--explain", "{not valid")
    assert code != 0
    assert "error" in payload
    assert payload["hint"]


def test_explain_in_reads_from_file(workspace):
    write_json(
        workspace / "design" / "d.diff.json",
        diff_payload(0.25, regions=[{"box": {"x": 0, "y": 0, "w": 10, "h": 10}, "diff_ratio": 0.25, "kind": "color"}]),
    )
    write_json(workspace / "design" / "explain.json", [{"index": 0, "reason": "anti-aliasing"}])
    code, payload = run(workspace, "--diff", "design/d.diff.json", "--explain-in", "design/explain.json")
    assert code == 0
    assert payload["ready"] is True


def test_out_writes_the_full_verdict_to_a_file(workspace):
    write_json(workspace / "design" / "d.diff.json", diff_payload(0.02))
    code, payload = run(workspace, "--diff", "design/d.diff.json", "--out", "design/verdict.json")
    assert code == 0
    out_file = workspace / "design" / "verdict.json"
    assert out_file.is_file()
    assert json.loads(out_file.read_text()) == payload
