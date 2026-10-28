from vlib import decode, text_mask

from fixtures import text_lines_image


def test_glyph_height_and_line_pitch_match_ground_truth(tmp_path):
    path = tmp_path / "text.png"
    expected = text_lines_image(path, glyph_height=16, line_pitch=24, lines=2)
    decoded = decode.load_image(path)

    metrics = text_mask.measure_text_region(decoded.array)
    assert metrics is not None
    assert abs(metrics["glyph_height_px"] - expected["glyph_height"]) <= 1
    assert abs(metrics["line_height_px"] - expected["line_pitch"]) <= 1
    assert metrics["lines"] == expected["lines"]


def test_text_metrics_never_carry_string_content(tmp_path):
    path = tmp_path / "text.png"
    text_lines_image(path, glyph_height=16, line_pitch=24, lines=2)
    decoded = decode.load_image(path)
    metrics = text_mask.measure_text_region(decoded.array)
    assert metrics is not None
    assert "content" not in metrics
    assert "text" not in metrics
    for value in metrics.values():
        if isinstance(value, str):
            # No field should hold anything resembling recognized characters —
            # only hex colors, alignment/weight labels, and an id are strings.
            assert value.startswith("#") or value in ("left", "right", "center", "regular", "medium", "bold")


def test_flat_crop_has_no_text(tmp_path):
    import numpy as np

    array = np.full((30, 30, 3), 255, dtype=np.uint8)
    assert text_mask.measure_text_region(array) is None


def test_single_line_uses_height_based_line_height_estimate(tmp_path):
    path = tmp_path / "text.png"
    text_lines_image(path, glyph_height=16, line_pitch=24, lines=1)
    decoded = decode.load_image(path)
    metrics = text_mask.measure_text_region(decoded.array)
    assert metrics is not None
    assert metrics["lines"] == 1
    assert metrics["line_height_px"] > metrics["glyph_height_px"]
