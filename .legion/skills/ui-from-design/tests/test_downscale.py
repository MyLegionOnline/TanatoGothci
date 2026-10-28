"""Этап 1.5's key test: coordinates measured on a downscaled processing
array must map back to within ±2px of the true original-resolution boxes.
Without this, `regions`/`compare` would hand the agent plausible-looking but
wrong numbers (design_vision_toolkit_plan.md Р3, Этап 1.3, Этап 3А.2)."""
from vlib import decode, segment

from fixtures import dashboard_image


def test_downscaled_regions_match_ground_truth_within_2px(tmp_path):
    path = tmp_path / "big.png"
    expected = dashboard_image(path, scale=5)  # 2000x1500 = 3,000,000 px
    decoded = decode.load_image(path)
    assert decoded.width * decoded.height > 2_000_000  # sanity: this actually forces downscale

    ds = decode.downscale(decoded, max_pixels=2_000_000)
    assert ds.scale_x < 1.0 and ds.scale_y < 1.0

    tree = segment.segment(ds.array, min_area=1, max_depth=7, min_gap=2)
    boxes = [ds.box_to_original(*node.box) for node in segment.iter_nodes(tree)]

    def find_closest(target: dict) -> dict:
        return min(
            boxes,
            key=lambda b: abs(b["x"] - target["x"]) + abs(b["y"] - target["y"]) + abs(b["w"] - target["w"]) + abs(b["h"] - target["h"]),
        )

    for key in ("header", "card", "card_header", "button"):
        actual = find_closest(expected[key])
        for coord in ("x", "y", "w", "h"):
            assert abs(actual[coord] - expected[key][coord]) <= 2, f"{key}.{coord}: {actual} vs {expected[key]}"


def test_downscale_is_a_noop_under_the_pixel_budget(tmp_path):
    path = tmp_path / "small.png"
    dashboard_image(path, scale=1)  # 400x300 = 120,000 px, well under 2,000,000
    decoded = decode.load_image(path)
    ds = decode.downscale(decoded, max_pixels=2_000_000)
    assert ds.scale_x == 1.0 and ds.scale_y == 1.0
    assert ds.array.shape == decoded.array.shape


def test_box_to_original_is_seamless_for_adjacent_boxes():
    """Two boxes that touch in downscaled space (box A's right edge ==
    box B's left edge) must still touch after remapping — independent
    per-box rounding must not open a gap or overlap between them."""
    ds = decode.Downscaled(array=None, scale_x=0.37, scale_y=0.37)  # array unused by box_to_original
    a = ds.box_to_original(0, 0, 10, 10)
    b = ds.box_to_original(10, 0, 10, 10)
    assert a["x"] + a["w"] == b["x"]
