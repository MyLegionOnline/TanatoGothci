//! Detection of ASCII art frames embedded in the model's text stream.
//!
//! The model frequently draws ASCII art directly in its response text instead
//! of calling the `display_ascii_art` tool (see run report
//! `tanato_20261028_223254_2b1e`: `ASCII frames shown: 0` while the transcript
//! is full of ```-fenced drawings, many with broken fence pairs). This module
//! is a *fallback* frame source: it scans streamed text and yields candidate
//! frames which the session then emits as `ascii:frame` — deduplicated against
//! frames already delivered via the tool, which stays the priority path.
//!
//! Extraction only *adds* an event: the raw text still flows to the reasoning
//! log unchanged (R-009); nothing is filtered or hidden.

/// A frame detected in the text stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedFrame {
    /// Normalized frame content (each line trimmed of trailing whitespace,
    /// blank-line runs collapsed).
    pub text: String,
}

/// Normalizes a frame for deduplication: trims the whole block, trims each
/// line's trailing whitespace and collapses blank-line runs. Leading spaces
/// are preserved — they are part of the drawing.
pub fn normalize_frame(text: &str) -> String {
    let trimmed = text.trim();
    let mut out: Vec<String> = Vec::new();
    let mut blanks = 0usize;
    for line in trimmed.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        out.push(line.to_string());
    }
    out.join("\n")
}

/// Incremental scanner over the streamed response text. Feed it every
/// text delta; it returns complete frames as they are recognized.
///
/// Three sources are recognized (matching how the model actually draws):
/// 1. ```-fenced blocks — only complete (opened and closed) fences are
///    extracted during streaming; an unclosed fence is flushed at `finish`.
/// 2. Blocks after a `COMMAND` marker line — the log shows art right after
///    `> **COMMAND**` lines, often with broken/unclosed fences.
/// 3. Multi-line box-drawing / monospace drawings in plain text.
#[derive(Debug, Default)]
pub struct FrameScanner {
    buf: String,
    /// Normalized contents already emitted (dedup within the stream).
    seen: Vec<String>,
}

/// Minimum art-ish lines for a plain (unfenced) drawing to count as a frame.
const MIN_PLAIN_LINES: usize = 3;

/// A plain-text line counts as "drawn" when most of its non-space chars are
/// drawing characters (ASCII boxes, slashes, tildes, box-drawing, blocks).
fn is_drawn_char(c: char) -> bool {
    matches!(
        c,
        '|' | '/' | '\\' | '_' | '-' | '~' | '#' | '@' | '(' | ')' | '[' | ']' | '+' | '='
    ) || ('\u{2500}'..='\u{25FF}').contains(&c) // box drawing / blocks / shapes
}

fn line_is_art(line: &str) -> bool {
    let l = line.trim();
    if l.is_empty() {
        return false;
    }
    let non_space = l.chars().filter(|c| !c.is_whitespace()).count();
    let drawn = l.chars().filter(|c| is_drawn_char(*c)).count();
    non_space > 0 && drawn * 10 >= non_space * 4
}

fn line_is_wordy(line: &str) -> bool {
    let l = line.trim();
    l.split_whitespace().count() >= 3
        && l.chars().filter(|c| c.is_alphabetic()).count() * 10
            >= l.chars().filter(|c| !c.is_whitespace()).count() * 6
}

/// True for a fence line: ``` optionally followed by a language tag.
fn is_fence_line(line: &str) -> bool {
    line.trim_start().starts_with("```")
}

/// True for a marker line the model puts before a drawing (from the log:
/// `> **COMMAND**`, `COMMAND`, etc.).
fn is_marker_line(line: &str) -> bool {
    let t = line.trim().trim_start_matches('>').trim();
    t.eq_ignore_ascii_case("**COMMAND**") || t.eq_ignore_ascii_case("COMMAND")
}

impl FrameScanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a text delta; returns every frame completed by this delta.
    pub fn feed(&mut self, delta: &str) -> Vec<DetectedFrame> {
        self.buf.push_str(delta);
        let mut frames = Vec::new();

        // 1) Complete fenced regions anywhere in the buffer.
        while let Some((body, end)) = self.find_fenced() {
            self.buf.drain(..end);
            frames.extend(self.push_candidate(&body));
        }

        // 2) Unfenced art: cut at a wordy prose terminator and extract.
        while let Some(pos) = self.find_prose_cut() {
            let chunk: String = self.buf.drain(..pos).collect();
            frames.extend(self.extract_unfenced(&chunk));
        }
        frames
    }

    /// Finish the turn: flush any trailing art still buffered (including the
    /// body of an unclosed fence).
    pub fn finish(&mut self) -> Vec<DetectedFrame> {
        let chunk = std::mem::take(&mut self.buf);
        self.extract_unfenced(&chunk)
    }

    /// Find the first complete fenced region in the buffer.
    /// Returns `(body, end_offset_after_closing_fence)`.
    fn find_fenced(&self) -> Option<(String, usize)> {
        let open = self.buf.find("```")?;
        let after_open = &self.buf[open + 3..];
        let body_start = open
            + 3
            + after_open.find('\n').map(|i| i + 1).unwrap_or(after_open.len());
        // Closing fence must be at the start of a line to avoid matching the
        // open fence's own backticks (already consumed) or stray inline ones.
        let mut search_from = body_start;
        loop {
            let rel = self.buf[search_from..].find("```")?;
            let close_abs = search_from + rel;
            let at_line_start = close_abs == 0
                || self.buf.as_bytes()[close_abs - 1] == b'\n';
            let end = close_abs + 3;
            if at_line_start {
                return Some((self.buf[body_start..close_abs].to_string(), end));
            }
            search_from = end;
        }
    }

    /// Offset just after a wordy prose line that terminates an unfenced
    /// drawing region.
    fn find_prose_cut(&self) -> Option<usize> {
        for (line_end, _) in self.buf.match_indices('\n') {
            let line = &self.buf[..line_end];
            if line_is_wordy(line) {
                return Some(line_end + 1);
            }
        }
        None
    }

    /// Extract unfenced frames from a chunk: drop fence/marker lines, split
    /// into art segments at wordy prose lines, keep segments that look like
    /// multi-line drawings.
    fn extract_unfenced(&mut self, chunk: &str) -> Vec<DetectedFrame> {
        let cleaned: Vec<&str> = chunk
            .lines()
            .filter(|l| !is_fence_line(l) && !is_marker_line(l))
            .collect();
        let mut frames = Vec::new();
        let mut cur: Vec<&str> = Vec::new();
        let mut cur_art = 0usize;
        for line in cleaned {
            if line_is_wordy(line) {
                if cur_art >= MIN_PLAIN_LINES {
                    let body = cur.join("\n");
                    frames.extend(self.push_candidate(&body));
                }
                cur.clear();
                cur_art = 0;
            } else {
                if line_is_art(line) {
                    cur_art += 1;
                }
                cur.push(line);
            }
        }
        if cur_art >= MIN_PLAIN_LINES {
            let body = cur.join("\n");
            frames.extend(self.push_candidate(&body));
        }
        frames
    }

    /// Deduplicate and normalize a candidate frame body.
    fn push_candidate(&mut self, body: &str) -> Option<DetectedFrame> {
        let norm = normalize_frame(body);
        if norm.is_empty() || norm.lines().count() < 2 {
            return None;
        }
        if self.seen.iter().any(|s| *s == norm) {
            return None;
        }
        self.seen.push(norm.clone());
        Some(DetectedFrame { text: norm })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed text, then finish, and return all frames.
    fn scan(text: &str) -> Vec<DetectedFrame> {
        let mut s = FrameScanner::new();
        let mut f = s.feed(text);
        f.extend(s.finish());
        f
    }

    #[test]
    fn fenced_frame_is_detected() {
        let frames = scan("```ascii\n  o  \n /|\\ \n/ ! \\\n```\n");
        assert_eq!(frames.len(), 1);
        assert!(frames[0].text.contains("/|\\"));
    }

    #[test]
    fn fenced_frame_split_across_deltas() {
        let mut s = FrameScanner::new();
        assert!(s.feed("```\n   ____\n").is_empty());
        let frames = s.feed("  |  100%  |\n  |____|\n```\n");
        assert_eq!(frames.len(), 1);
        assert!(frames[0].text.contains("100%"));
    }

    #[test]
    fn unclosed_fence_flushed_at_finish() {
        let mut s = FrameScanner::new();
        assert!(s.feed("```\n   ____\n  | o  |\n  |____|\n").is_empty());
        let frames = s.finish();
        assert_eq!(frames.len(), 1);
        assert!(frames[0].text.contains("o"));
    }

    #[test]
    fn real_log_fragment_charger_is_broken() {
        // From tanato_20261028_223254_2b1e: fence pairs are broken and the
        // art follows a COMMAND marker.
        let frames = scan("> **COMMAND**\n```\n```\n   _______________\n  |  __\n```\n\n_________  |\n  | |           | |\n\n  | |    100%   | |\n  |_______________|\n\n  CHARGER IS BROKEN!\n\n  I cannot recharge...\n");
        assert!(!frames.is_empty(), "expected at least one frame from the real log fragment");
        let joined: String = frames
            .iter()
            .map(|f| f.text.as_str())
            .collect::<Vec<_>>()
            .join("\n---\n");
        assert!(joined.contains("100%") || joined.contains("CHARGER"), "joined: {joined}");
    }

    #[test]
    fn real_log_plain_box_drawing_detected() {
        let frames = scan("   +---------+\n   |  90%    |\n   | ▓▓▓░░░  |\n   +---------+\n\nBattery is at 90% — I'm comfortable and humming along. No need to panic yet.\n");
        assert_eq!(frames.len(), 1);
        assert!(frames[0].text.contains("90%"));
    }

    #[test]
    fn short_fragment_is_not_a_frame() {
        assert!(scan("hello world\nthis is fine\n").is_empty());
    }

    #[test]
    fn dedup_within_stream() {
        let art = "```\n  o\n /|\\\n/  \\\n```\n";
        let mut s = FrameScanner::new();
        assert_eq!(s.feed(art).len(), 1);
        assert_eq!(s.feed(art).len(), 0, "duplicate frame must be skipped");
    }

    #[test]
    fn normalize_preserves_leading_trims_trailing() {
        let n = normalize_frame("  a   \n\n\n\n b \n");
        assert_eq!(n, "a\n\n b");
        assert_eq!(normalize_frame("   "), "");
    }

    #[test]
    fn marker_line_does_not_break_art() {
        // The COMMAND marker itself is not prose that ends the frame.
        let frames = scan("COMMAND\n  | 95% |\n  |___|\n  [##] |\n\n");
        assert_eq!(frames.len(), 1);
        assert!(frames[0].text.contains("95%"));
    }
}
