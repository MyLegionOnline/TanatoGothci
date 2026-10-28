import numpy as np

from vlib import color


def test_hex_roundtrip():
    assert color.to_hex((17, 24, 39)) == "#111827"
    assert color.hex_to_rgb("#111827") == (17, 24, 39)
    assert color.hex_to_rgb("#fff") == (255, 255, 255)


def test_contrast_ratio_black_on_white_is_21():
    # WCAG reference value: black on white is the maximum possible ratio, 21:1.
    assert color.contrast_ratio((0, 0, 0), (255, 255, 255)) == 21.0


def test_contrast_ratio_identical_colors_is_1():
    assert color.contrast_ratio((128, 128, 128), (128, 128, 128)) == 1.0


def test_wcag_levels_reference_pair():
    # #111827 on #FFFFFF is a common "body text" pair — ratio ~17.7, passes everything.
    ratio = color.contrast_ratio((17, 24, 39), (255, 255, 255))
    levels = color.wcag_levels(ratio, font_px=16)
    assert levels == {"aa": True, "aa_large": True, "aaa": True}


def test_wcag_levels_borderline_fails_normal_but_passes_large():
    # gray(130) on white: ratio 3.84 — fails normal-text AA (4.5) but passes
    # once the *same* pair is classified as large text (>=24px, threshold 3.0).
    fg, bg = (130, 130, 130), (255, 255, 255)
    ratio = color.contrast_ratio(fg, bg)
    normal = color.wcag_levels(ratio, font_px=16)
    large = color.wcag_levels(ratio, font_px=32)
    assert not normal["aa"]
    assert large["aa"]
    assert normal["aa_large"] and large["aa_large"]  # aa_large is ratio>=3.0 regardless of size


def test_dominant_palette_coverage_sums_close_to_one():
    array = np.zeros((10, 10, 3), dtype=np.uint8)
    array[:6] = (255, 0, 0)
    array[6:] = (0, 255, 0)
    # bits_per_channel=8 -> no quantization loss, exact colors round-trip
    palette = color.dominant_palette(array, palette_size=4, bits_per_channel=8)
    assert palette[0]["hex"] == "#FF0000"
    assert palette[0]["coverage"] == 0.6
    assert palette[1]["hex"] == "#00FF00"
    assert palette[1]["coverage"] == 0.4


def test_background_color_uses_border_majority():
    array = np.full((20, 20, 3), 255, dtype=np.uint8)
    array[8:12, 8:12] = (0, 0, 0)  # a black square in the middle, away from the border
    assert color.background_color(array) == (255, 255, 255)


def test_suggest_foreground_picks_higher_contrast():
    assert color.suggest_foreground((10, 10, 10)) == "#FFFFFF"
    assert color.suggest_foreground((250, 250, 250)) == "#000000"
