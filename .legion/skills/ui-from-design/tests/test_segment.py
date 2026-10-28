from vlib import decode, segment

from fixtures import dashboard_image


def _boxes_by_area(root: segment.Node) -> list[tuple[int, int, int, int]]:
    return [n.box for n in segment.iter_nodes(root)]


def _assert_box_close(actual: dict, expected: dict, tol: int = 2) -> None:
    for key in ("x", "y", "w", "h"):
        assert abs(actual[key] - expected[key]) <= tol, f"{key}: {actual} vs expected {expected}"


def test_regions_tree_matches_known_boxes(tmp_path):
    image_path = tmp_path / "dashboard.png"
    expected = dashboard_image(image_path)

    decoded = decode.load_image(image_path)
    tree = segment.segment(decoded.array, min_area=100, max_depth=7, min_gap=2)
    nodes = list(segment.iter_nodes(tree))
    boxes = [dict(x=n.box[0], y=n.box[1], w=n.box[2], h=n.box[3]) for n in nodes]

    def find_closest(target: dict) -> dict:
        return min(boxes, key=lambda b: abs(b["x"] - target["x"]) + abs(b["y"] - target["y"]) + abs(b["w"] - target["w"]) + abs(b["h"] - target["h"]))

    _assert_box_close(find_closest(expected["header"]), expected["header"])
    _assert_box_close(find_closest(expected["card"]), expected["card"])
    _assert_box_close(find_closest(expected["card_header"]), expected["card_header"])
    _assert_box_close(find_closest(expected["button"]), expected["button"])


def test_flat_leaf_has_no_children():
    import numpy as np

    array = np.full((40, 40, 3), (10, 20, 30), dtype=np.uint8)
    tree = segment.segment(array, min_area=1, max_depth=5, min_gap=2)
    # A wholly flat image has nothing to cut on: the synthetic root itself
    # becomes the leaf (no fragments). `iter_nodes` deliberately skips the
    # root (see its docstring), so a fully flat image correctly yields zero
    # reportable "regions" — there is nothing to segment.
    assert tree.leaf_kind == "flat"
    assert tree.children == []
    assert list(segment.iter_nodes(tree)) == []


def test_border_and_radius_estimate_are_heuristic_but_present(tmp_path):
    image_path = tmp_path / "dashboard.png"
    expected = dashboard_image(image_path)
    decoded = decode.load_image(image_path)

    button = expected["button"]
    radius = segment.estimate_radius(decoded.array, button, fill_rgb=(59, 130, 246), outside_rgb=(255, 255, 255))
    # The button is drawn as a hard-edged rectangle (no rounding) — radius
    # should read as ~0, not some arbitrary positive number.
    assert radius == 0
