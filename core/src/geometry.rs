//! ASCII panel geometry (plan §2): the frontend measures the ASCII panel and
//! hands the limits to the backend, which injects them into the system prompt
//! and into the argument description of the `display_ascii_art` tool so the
//! model draws frames that fit the panel.
//!
//! The geometry is only an advisory limit: a frame that exceeds it is NOT
//! dropped or silently trimmed — the excess is logged and the frame is still
//! emitted (`ascii:frame`), because a crooked art is better than an empty
//! panel.

/// Fallback geometry used until the first `set_ascii_geometry` command
/// arrives from the frontend (plan assumption: ~55 columns × 14 rows at
/// 420×640 with monospace 12px / line-height 1.35).
pub const DEFAULT_ASCII_COLS: u32 = 55;
pub const DEFAULT_ASCII_ROWS: u32 = 14;

/// Measured ASCII panel limits (columns × rows).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct AsciiGeometry {
    pub cols: u32,
    pub rows: u32,
}

impl Default for AsciiGeometry {
    fn default() -> Self {
        Self {
            cols: DEFAULT_ASCII_COLS,
            rows: DEFAULT_ASCII_ROWS,
        }
    }
}

impl AsciiGeometry {
    /// Reject null / absurdly small measurements: the frontend may send 0
    /// before the panel is laid out, and such values must never replace an
    /// already-known geometry (task acceptance criteria).
    pub fn is_sane(&self) -> bool {
        self.cols > 0 && self.rows > 0
    }

    /// Human-readable limit, e.g. `"55x14"`.
    pub fn label(&self) -> String {
        format!("{}x{}", self.cols, self.rows)
    }
}

/// System-prompt appendix that tells the model the exact panel limits: a
/// frame strictly within N columns × M rows, monospace / box-drawing
/// characters, no wrapping, equal line widths.
pub fn prompt_geometry_clause(geo: AsciiGeometry) -> String {
    format!(
        "\n\nASCII PANEL GEOMETRY: the panel is exactly {cols} columns width \
and {rows} rows tall. Every ASCII art frame MUST fit strictly within \
{cols} columns x {rows} rows. Use only monospace / box-drawing characters \
(space, | - + / \\ _ = . : ; ' ` \" * # and similar). Never let the model \
wrap your lines: every line must be complete and all lines must have equal \
width (pad shorter lines with trailing spaces up to the frame width). \
Do not exceed {cols} characters per line and {rows} lines per frame.",
        cols = geo.cols,
        rows = geo.rows
    )
}

/// Description of the `display_ascii_art` tool arguments with the measured
/// limits baked in (so the same numbers appear wherever the tool contract is
/// described to the model).
pub fn display_ascii_art_arg_description(geo: AsciiGeometry) -> String {
    format!(
        "display_ascii_art arguments: {{ \"art\": string }} — the art field \
holds the full ASCII frame. The frame MUST fit strictly within {cols} \
columns x {rows} rows: at most {cols} characters per line and at most \
{rows} lines, monospace / box-drawing characters only, no line wrapping, \
all lines of equal width.",
        cols = geo.cols,
        rows = geo.rows
    )
}

/// Measure an emitted frame and, when it exceeds the panel limits, return a
/// warning line for the run log. The frame itself is never rejected — the
/// caller still emits it in full.
pub fn frame_exceeds(geo: AsciiGeometry, frame: &str) -> Option<String> {
    let lines: Vec<&str> = frame.lines().collect();
    let height = lines.len() as u32;
    let width = lines.iter().map(|l| l.chars().count() as u32).max().unwrap_or(0);
    if width > geo.cols || height > geo.rows {
        Some(format!(
            "frame {}x{} exceeds limit {}",
            width, height, geo.label()
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_geometry_matches_plan_assumption() {
        let g = AsciiGeometry::default();
        assert_eq!((g.cols, g.rows), (55, 14));
        assert!(g.is_sane());
    }

    #[test]
    fn sanity_rejects_zero_dimensions() {
        assert!(!AsciiGeometry { cols: 0, rows: 14 }.is_sane());
        assert!(!AsciiGeometry { cols: 55, rows: 0 }.is_sane());
        assert!(AsciiGeometry { cols: 1, rows: 1 }.is_sane());
    }

    #[test]
    fn prompt_clause_contains_exact_numbers() {
        let clause = prompt_geometry_clause(AsciiGeometry { cols: 55, rows: 14 });
        assert!(clause.contains("55 columns"));
        assert!(clause.contains("14 rows"));
        assert!(clause.contains("55 columns x 14 rows"));
        // required style constraints
        assert!(clause.contains("monospace"));
        assert!(clause.contains("box-drawing"));
        assert!(clause.contains("equal width"));
    }

    #[test]
    fn tool_arg_description_contains_exact_numbers() {
        let d = display_ascii_art_arg_description(AsciiGeometry { cols: 55, rows: 14 });
        assert!(d.contains("display_ascii_art"));
        assert!(d.contains("\"art\""));
        assert!(d.contains("55"));
        assert!(d.contains("14"));
    }

    #[test]
    fn frame_within_limits_is_not_flagged() {
        let geo = AsciiGeometry { cols: 55, rows: 14 };
        let frame = "x".repeat(55) + "\n" + &"y".repeat(54);
        assert_eq!(frame_exceeds(geo, &frame), None);
    }

    #[test]
    fn oversized_frame_reports_exact_dimensions_but_is_not_trimmed() {
        let geo = AsciiGeometry { cols: 55, rows: 14 };
        let mut frame = String::new();
        for _ in 0..16 {
            frame.push_str(&"a".repeat(60));
            frame.push('\n');
        }
        let warning = frame_exceeds(geo, &frame).expect("must be flagged");
        assert!(warning.contains("frame 60x16 exceeds limit 55x14"), "{warning}");
        // The frame content itself is untouched — trimming is not our job.
        assert_eq!(frame.lines().count(), 16);
    }

    #[test]
    fn empty_frame_does_not_panic() {
        let geo = AsciiGeometry::default();
        assert_eq!(frame_exceeds(geo, ""), None);
        assert_eq!(frame_exceeds(geo, "\n\n"), None);
    }
}
