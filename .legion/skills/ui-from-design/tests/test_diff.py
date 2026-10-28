from vlib import decode, diff

from fixtures import diff_pair_shifted_bar, solid_image


def test_identical_images_have_zero_diff_ratio(tmp_path):
    path = tmp_path / "a.png"
    solid_image(path, (100, 100), (200, 200, 200))
    decoded = decode.load_image(path)
    mask, ratio = diff.pixel_diff(decoded.array, decoded.array)
    assert ratio == 0.0
    assert not mask.any()


def test_shifted_bar_gives_exactly_one_diff_region(tmp_path):
    fixture = diff_pair_shifted_bar(tmp_path)
    decoded_a = decode.load_image(fixture["a"])
    decoded_b = decode.load_image(fixture["b"])

    mask, ratio = diff.pixel_diff(decoded_a.array, decoded_b.array)
    assert ratio > 0

    regions, truncated = diff.diff_regions(mask, decoded_a.array, decoded_b.array, min_area=4, max_regions=20)
    assert not truncated
    assert len(regions) == 1

    expected = fixture["expected_box"]
    box = regions[0]["box"]
    for key in ("x", "y", "w", "h"):
        assert abs(box[key] - expected[key]) <= 2


def test_diff_region_kind_is_missing_or_extra_for_a_shift(tmp_path):
    fixture = diff_pair_shifted_bar(tmp_path)
    decoded_a = decode.load_image(fixture["a"])
    decoded_b = decode.load_image(fixture["b"])
    mask, _ = diff.pixel_diff(decoded_a.array, decoded_b.array)
    regions, _ = diff.diff_regions(mask, decoded_a.array, decoded_b.array, min_area=4, max_regions=20)
    assert regions[0]["kind"] in ("missing", "extra", "position")


def test_render_heatmap_returns_an_image_same_size(tmp_path):
    fixture = diff_pair_shifted_bar(tmp_path)
    decoded_a = decode.load_image(fixture["a"])
    decoded_b = decode.load_image(fixture["b"])
    mask, _ = diff.pixel_diff(decoded_a.array, decoded_b.array)
    heatmap = diff.render_heatmap(decoded_a.array, mask)
    assert heatmap.size == (decoded_a.width, decoded_a.height)
