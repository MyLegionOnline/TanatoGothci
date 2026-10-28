from vlib import measure


def test_distance_basic():
    result = measure.distance({"x": 0, "y": 0}, {"x": 3, "y": 4})
    assert result == {"px": 5.0, "dx": 3, "dy": 4}


def test_gap_horizontal_and_vertical():
    a = {"x": 0, "y": 0, "w": 10, "h": 10}
    b = {"x": 20, "y": 5, "w": 10, "h": 10}
    result = measure.gap(a, b)
    assert result["horizontal"] == 10
    assert result["vertical"] == 0


def test_gap_zero_when_overlapping():
    a = {"x": 0, "y": 0, "w": 10, "h": 10}
    b = {"x": 5, "y": 5, "w": 10, "h": 10}
    result = measure.gap(a, b)
    assert result["horizontal"] == 0
    assert result["vertical"] == 0


def test_alignment_groups_finds_shared_left_edge():
    boxes = [
        {"x": 10, "y": 0, "w": 50, "h": 20, "label": "a"},
        {"x": 11, "y": 40, "w": 30, "h": 20, "label": "b"},
        {"x": 200, "y": 80, "w": 30, "h": 20, "label": "c"},
    ]
    groups = measure.alignment_groups(boxes, tolerance=2)
    left_groups = [g for g in groups if g["edge"] == "left"]
    assert len(left_groups) == 1
    assert set(left_groups[0]["members"]) == {"a", "b"}


def test_alignment_groups_ignores_singletons():
    boxes = [{"x": 0, "y": 0, "w": 10, "h": 10, "label": "solo"}]
    assert measure.alignment_groups(boxes) == []


def test_spacing_rhythm_flags_violators():
    rhythm = measure.spacing_rhythm([8, 16, 24, 13], spacing_scale=8)
    assert rhythm["scale"] == 8
    assert 13 in rhythm["violators"]
    assert 8 not in rhythm["violators"]
    assert rhythm["nearest_multiples"][13] == 16
