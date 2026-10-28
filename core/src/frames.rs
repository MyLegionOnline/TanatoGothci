//! Detection of ASCII art frames embedded in the model's text stream.
//!
//! The model frequently draws ASCII art directly in its response text instead
//! of calling the `display_ascii_art` tool (see run report
//! `tanato_20261028_223254_2b1e`: `ASCII frames shown: 0` while the transcript
//! is full of ```-fenced drawings). This module is a *fallback* frame source:
//! it scans streamed text and yields candidate frames which the session then
//! emits as `ascii:frame` — deduplicated against frames already delivered via
//! the tool, which stays the priority path.
//!
//! Extraction only *adds* an event: the raw text still flows to the reasoning
//! log unchanged (R-009); nothing is filtered or hidden.

/// A frame detected in the text stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedFrame {
    /// Normalized frame content (lines trimmed of trailing whitespace).
    pub text: String,
}

/// Normalizes a frame for deduplication: trims the whole block, trims each
/// line's trailing whitespace and collapses blank-line runs.
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
/// `Reasoning`/content delta; it returns complete frames as they close.
///
/// Three sources are recognized (matching how the model actually draws):
/// 1. ```-fenced blocks — the fence pairs are often broken across deltas, so
///    the scanner works on a rolling buffer.
/// 2. Blocks after a `COMMAND` marker line — the log shows art right after
///    `> **COMMAND**` / `COMMAND` lines, sometimes without a working fence.
/// 3. Multi-line box-drawing / monospace drawings in plain text.
#[derive(Debug, Default)]
pub struct FrameScanner {
    buf: String,
    /// Normalized contents already emitted (dedup within the stream).
    seen: Vec<String>,
}

/// Minimum lines for a plain-text (unfenced) drawing to count as a frame.
const MIN_PLAIN_LINES: usize = 3;
/// A plain-text line must contain at least one "drawn" char to count.
fn is_drawn_char(c: char) -> bool {
    matches!(
        c,
        '|' | '/' | '\\' | '_' | '-' | '~' | '#' | '@' | '(' | ')' | '[' | ']' | '+' | '='
    ) || ('\u{2500}'..='\u{257F}').contains(&c) // box drawing
        || ('\u{2580}'..='\u{259F}').contains(&c) // block elements
        || ('\u{2596}'..='\u{259F}').contains(&c)
        || ('\u{25A0}'..='\u{25FF}').contains(&c) // geometric shapes (▓░ etc.)
}

fn line_is_art(line: &str) -> bool {
    let l = line.trim();
    if l.is_empty() {
        return false;
    }
    l.chars().filter(|c| is_drawn_char(*c)).count() * 10 >= l.chars().filter(|c| !c.is_whitespace()).count() * 7
}

fn line_is_wordy(line: &str) -> bool {
    let l = line.trim();
    l.split_whitespace().count() >= 3
        && l.chars().filter(|c| c.is_alphabetic()).count() * 10
            >= l.chars().filter(|c| !c.is_whitespace()).count() * 6
}

impl FrameScanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a text delta; returns every frame completed by this delta.
    pub fn feed(&mut self, delta: &str) -> Vec<DetectedFrame> {
        self.buf.push_str(delta);
        let mut frames = Vec::new();
        // A fence closes a candidate region; also cut at a "wordy prose"
        // terminator so unfenced art doesn't glue onto following sentences.
        while let Some(pos) = self.find_cut() {
            let chunk: String = self.buf.drain(..pos).collect();
            frames.extend(self.extract_from(&chunk));
        }
        frames
    }

    /// Finish the turn: flush any trailing unfenced art still buffered.
    pub fn finish(&mut self) -> Vec<DetectedFrame> {
        let chunk = std::mem::take(&mut self.buf);
        self.extract_from(&chunk)
    }

    /// Position at which the buffered text should be cut and examined.
    /// A closing (or opening of a new) fence, or a prose boundary after
    /// enough art-ish lines have accumulated.
    fn find_cut(&self) -> Option<usize> {
        for (line_end, _) in self.buf.match_indices('\n') {
            let line = &self.buf[..line_end];
            if line.trim_start().starts_with("```") {
                return Some(line_end + 1);
            }
            // A clearly wordy prose line ends an unfenced drawing region.
            if line_is_wordy(line) && self.art_run_len(0, line_end) >= MIN_PLAIN_LINES {
                return Some(line_end + 1);
            }
        }
        None
    }

    /// Length of the consecutive run of art-ish lines ending right before
    /// `up_to`.
    fn art_run_len(&self, from: usize, up_to: usize) -> usize {
        self.buf[from..up_to]
            .lines()
            .rev()
            .skip_while(|l| l.trim().is_empty())
            .take_while(|l| line_is_art(l))
            .count()
    }

    /// Pull frames out of a chunk of stream text.
    fn extract_from(&mut self, chunk: &str) -> Vec<DetectedFrame> {
        let mut frames = Vec::new();
        // 1) Fenced blocks.
        let mut rest = chunk;
        while let Some(start) = rest.find("```") {
            let after = &rest[start + 3..];
            // Skip optional language tag on the same line.
            let body_start = after.find('\n').map(|i| i + 1).unwrap_or(after.len());
            match after[body_start..].find("```") {
                Some(end) => {
                    let body = &after[body_start..body_start + end];
                    frames.extend(self.push_candidate(body));
                    rest = &after[body_start + end + 3..];
                }
                None => {
                    // Unbalanced fence in this chunk — treat the remainder as
                    // body anyway (the log shows unclosed fences; better to
                    // draw something than nothing).
                    let body = &after[body_start..];
                    if !body.trim().is_empty() {
                        frames.extend(self.push_candidate(body));
                    }
                    rest = "";
                }
            }
        }
        // 2) Unfenced: COMMAND-marker blocks and plain multi-line drawings.
        //    Split into runs of art lines separated by wordy prose.
        for para in chunk.split("```") {
            for segment in split_wordy(para) {
                let t = segment.trim();
                if t.is_empty() {
                    continue;
                }
                let lines: Vec<&str> = t.lines().collect();
                if lines.len() >= MIN_PLAIN_LINES
                    && lines.iter().filter(|l| line_is_art(l)).count() * 10
                        >= lines.len() * 6
                {
                    frames.extend(self.push_candidate(t));
                }
            }
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

/// Splits text into segments cut at "wordy" prose lines (sentences the model
/// writes between drawings). Art segments stay intact.
fn split_wordy(text: &str) -> Vec<String> {
    let mut segs: Vec<String> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line_is_wordy(line) && !cur.iter().all(|l| l.trim().is_empty()) {
            if !cur.iter().all(|l| l.trim().is_empty()) {
                segs.push(cur.join("\n"));
            }
            cur = Vec::new();
        } else {
            cur.push(line);
        }
    }
    if !cur.iter().all(|l| l.trim().is_empty()) {
        segs.push(cur.join("\n"));
    }
    segs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fenced_frame_is_detected() {
        let mut s = FrameScanner::new();
        let frames = s.feed("```ascii\n  o  \n /|\\ \n/ ! \\\n```\n");
        assert_eq!(frames.len(), 1);
        assert!(frames[0].text.contains("/|\\"));
    }

    #[test]
    fn fenced_frame_split_across_deltas() {
        let mut s = FrameScanner::new();
        assert!(s.feed("```\n   ____\n").is_empty());
        let frames = s.feed("  |  100%  |\n  |____|\n```");
        assert_eq!(frames.len(), 1);
        assert!(frames[0].text.contains("100%"));
    }

    #[test]
    fn real_log_fragment_charger_is_broken() {
        let mut s = FrameScanner::new();
        // From tanato_20261028_223254_2b1e: fence pairs are broken; art
        // follows a COMMAND marker.
        let frames = s.feed("> **COMMAND**\n```\n```\n   _______________\n  |  __\n```\n\n_________  |\n  | |           | |\n\n  | |    100%   | |\n  |_______________|\n\n  CHARGER IS BROKEN!\n");
        assert!(!frames.is_empty(), "expected at least one frame from the real log fragment");
        let joined: String = frames.iter().map(|f| f.text.as_str()).collect::<Vec<_>>().join("\n---\n");
        assert!(joined.contains("100%") || joined.contains("CHARGER"), "joined: {joined}");
    }

    #[test]
    fn plain_box_drawing_detected_and_prose_excluded() {
        let mut s = FrameScanner::new();
        let frames = s.finish_feed("   +---------+\n   |  90%    |\n   | ▓▓▓░░░  |\n   +---------+\n\nBattery is at 90% — I'm comfortable and humming along. No need to panic yet.\n");
        assert_eq!(frames.len(), 1);
        assert!(frames[0].text.contains("90%"));
    }

    /// Helper for tests: feed then finish in one call.
    trait FinishFeed {
        fn finish_feed(&mut self, text: &str) -> Vec<DetectedFrame>;
    }
    impl FinishFeed for FrameScanner {
        fn finish_feed(&mut self, text: &str) -> Vec<DetectedFrame> {
            let mut f = self.feed(text);
            f.extend(self.finish());
            f
        }
    }

    #[test]
    fn short_fragment_is_not_a_frame() {
        let mut s = FrameScanner::new();
        assert!(s.finish_feed("hello world\nthis is fine\n").is_empty());
    }

    #[test]
    fn dedup_within_stream() {
        let art = "```\n  o\n /|\\\n```\n";
        let mut s = FrameScanner::new();
        assert_eq!(s.feed(art).len(), 1);
        assert_eq!(s.feed(art).len(), 0, "duplicate frame must be skipped");
    }

    #[test]
    fn normalize_collapses_blank_runs_and_trailing_ws() {
        let n = normalize_frame("  a   \n\n\n\n b \n");
        assert_eq!(n, "a\n\nb");
        assert_eq!(normalize_frame("   "), "");
    }
}
