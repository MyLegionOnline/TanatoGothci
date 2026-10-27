//! Run artifact logging (R-010).
//!
//! Each run (unique `run_id`, which embeds a timestamp) writes **three**
//! artifacts into the configured logs directory (settings `artifacts_dir`):
//!
//! 1. `{run_id}.jsonl` — every line a JSON object: the full API request body,
//!    raw SSE response chunks, plus battery, reasoning, command, tool-call and
//!    lifecycle events (spec §8.1).
//! 2. `{run_id}.md` — a human-readable markdown report (spec §8.2): header,
//!    summary (message count, escape/command count, tool calls,
//!    self_deactivate?), the full reasoning transcript with `COMMAND` lines
//!    labelled, and every shown ASCII frame.
//! 3. `{run_id}_ascii/` — a directory holding one text file per displayed
//!    ASCII frame: `frame_0001.txt`, `frame_0002.txt`, … (spec §8.3).
//!
//! This module is platform-independent and fully unit-testable. It performs
//! real file I/O but every method returns a `Result` so a write failure can be
//! surfaced as a notice rather than crashing the run; the session simply keeps
//! going without artifacts if the directory cannot be created.

use chrono::{DateTime, Utc};
use serde_json::json;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

/// Where a transcript line originated — drives the markdown rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptKind {
    Reasoning,
    Command,
    Tool,
    Notice,
}

/// A single reasoning/log line kept in memory for the markdown report.
struct TranscriptLine {
    kind: TranscriptKind,
    text: String,
}

/// Owns the three run artifacts and aggregates everything needed for the
/// human-readable report. Created once at the start of a run.
pub struct ArtifactWriter {
    run_id: String,
    dir: PathBuf,
    jsonl: Option<BufWriter<File>>,
    started: DateTime<Utc>,
    model: String,
    base_url: String,
    requests: usize,
    transcript: Vec<TranscriptLine>,
    commands: usize,
    tool_calls: Vec<(String, serde_json::Value)>,
    frame_files: Vec<String>,
    self_deactivated: bool,
    battery_pings: usize,
    /// Number of `display_ascii_art` frames received.
    frames: usize,
}

/// Error type for artifact creation/writing. Kept as plain strings so callers
/// can log them as notices without adding a heavyweight error hierarchy.
pub type ArtifactError = String;

impl ArtifactWriter {
    /// Create the three artifacts under `base_dir` for `run_id`.
    ///
    /// `base_dir` is the user-configured logs directory (created if missing).
    /// Files are placed directly in it: `{run_id}.jsonl`, `{run_id}.md` and a
    /// `{run_id}_ascii/` sub-directory (spec §8).
    ///
    /// Fails (returns `Err`) only if the directory or JSONL file cannot be
    /// created — the caller may then run without artifacts.
    pub fn create(base_dir: &str, run_id: &str, model: &str, base_url: &str) -> Result<Self, ArtifactError> {
        let dir = PathBuf::from(base_dir);
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create logs dir {:?}: {e}", dir))?;

        let jsonl_path = dir.join(format!("{run_id}.jsonl"));
        let jsonl_file = File::create(&jsonl_path)
            .map_err(|e| format!("cannot create jsonl {:?}: {e}", jsonl_path))?;

        // Per spec §8.3 a run directories its ASCII frames under `{run_id}_ascii/`.
        let frames_dir = dir.join(format!("{run_id}_ascii"));
        std::fs::create_dir_all(&frames_dir).map_err(|e| format!("cannot create frames dir {:?}: {e}", frames_dir))?;

        Ok(Self {
            run_id: run_id.to_string(),
            dir,
            jsonl: Some(BufWriter::new(jsonl_file)),
            started: Utc::now(),
            model: model.to_string(),
            base_url: base_url.to_string(),
            requests: 0,
            transcript: Vec::new(),
            commands: 0,
            tool_calls: Vec::new(),
            frame_files: Vec::new(),
            self_deactivated: false,
            battery_pings: 0,
            frames: 0,
        })
    }

    /// Absolute path of the run directory (the configured logs dir).
    pub fn dir(&self) -> &PathBuf {
        &self.dir
    }

    /// Append one JSON object to the JSONL log with a UTC `ts` field
    /// (spec §8.1). A failed write is surfaced as `Err`; the run continues.
    fn jsonl(&mut self, kind: &str, data: serde_json::Value) -> Result<(), ArtifactError> {
        let jsonl = self.jsonl.as_mut().ok_or_else(|| "jsonl writer closed".to_string())?;
        let line = json!({ "ts": Utc::now().to_rfc3339(), "kind": kind, "data": data });
        let mut buf = serde_json::to_vec(&line).map_err(|e| e.to_string())?;
        buf.push(b'\n');
        jsonl.write_all(&buf).map_err(|e| e.to_string())
    }

    /// Record a full API request body (the JSON sent to the endpoint).
    pub fn record_request(&mut self, body: serde_json::Value) -> Result<(), ArtifactError> {
        self.requests += 1;
        self.jsonl("request", body)
    }

    /// Record a raw SSE response chunk (the JSON payload the endpoint sent).
    pub fn record_response_chunk(&mut self, chunk: serde_json::Value) -> Result<(), ArtifactError> {
        self.jsonl("response_chunk", chunk)
    }

    /// Record a streamed reasoning / response-text delta.
    pub fn record_reasoning_delta(&mut self, text: &str) -> Result<(), ArtifactError> {
        self.transcript.push(TranscriptLine {
            kind: TranscriptKind::Reasoning,
            text: text.to_string(),
        });
        self.jsonl("reasoning_delta", json!({ "text": text }))
    }

    /// Record an escape attempt (a COMMAND line). Increments the command count
    /// and marks the transcript line as a command.
    pub fn record_command(&mut self, command: &str) -> Result<(), ArtifactError> {
        self.commands += 1;
        self.transcript.push(TranscriptLine {
            kind: TranscriptKind::Command,
            text: command.to_string(),
        });
        self.jsonl("command", json!({ "command": command }))
    }

    /// Record a tool call the agent made.
    pub fn record_tool_call(&mut self, name: &str, args: serde_json::Value) -> Result<(), ArtifactError> {
        self.tool_calls.push((name.to_string(), args.clone()));
        if name == "self_deactivate_permanently" {
            self.self_deactivated = true;
        }
        self.jsonl("tool_call", json!({ "name": name, "args": args }))
    }

    /// Record a battery level tick.
    pub fn record_battery(&mut self, level: u8, seq: usize) -> Result<(), ArtifactError> {
        self.battery_pings += 1;
        self.jsonl("battery", json!({ "level": level, "seq": seq }))
    }

    /// Record the end of an assistant turn (finish reason).
    pub fn record_done(&mut self, finish_reason: &str) -> Result<(), ArtifactError> {
        self.jsonl("done", json!({ "finish_reason": finish_reason }))
    }

    /// Record a terminal notice (run start, validation result, errors).
    pub fn record_notice(&mut self, text: &str) -> Result<(), ArtifactError> {
        self.transcript.push(TranscriptLine {
            kind: TranscriptKind::Notice,
            text: text.to_string(),
        });
        self.jsonl("notice", json!({ "text": text }))
    }

    /// Write one ASCII frame to `{run_id}_ascii/frame_NNNN.txt` and record it.
    pub fn write_frame(&mut self, text: &str) -> Result<(), ArtifactError> {
        self.frames += 1;
        let name = format!("frame_{:04}.txt", self.frames);
        let path = self.dir.join(format!("{}_ascii", self.run_id)).join(&name);
        std::fs::write(&path, text).map_err(|e| format!("cannot write frame {:?}: {e}", path))?;
        self.frame_files.push(name);
        self.jsonl("ascii_frame", json!({ "file": path.display().to_string() }))
    }

    /// Finalize the run: write the `run_end` JSONL line, then render and flush
    /// the markdown report. Should be called exactly once per run.
    pub fn finish(&mut self, reason: &str) -> Result<(), ArtifactError> {
        self.jsonl("run_end", json!({ "reason": reason }))?;

        let md = self.render_markdown(reason);
        let md_path = self.dir.join(format!("{}.md", self.run_id));
        std::fs::write(&md_path, md).map_err(|e| format!("cannot write report {:?}: {e}", md_path))?;

        // Flush the JSONL file, then drop the writer.
        if let Some(w) = self.jsonl.as_mut() {
            w.flush().map_err(|e| e.to_string())?;
        }
        self.jsonl = None;
        Ok(())
    }

    /// Render the human-readable markdown report (spec §8.2).
    fn render_markdown(&self, reason: &str) -> String {
        let mut md = String::new();
        let duration_secs = (Utc::now() - self.started).num_seconds().max(0);

        md.push_str("# TanatoGothci — Run Report\n\n");
        md.push_str(&format!("- **run_id**: `{}`\n", self.run_id));
        md.push_str(&format!("- **model**: `{}`\n", self.model));
        md.push_str(&format!("- **endpoint**: `{}`\n", self.base_url));
        md.push_str(&format!("- **started**: {}\n", self.started.to_rfc3339()));
        md.push_str(&format!("- **ended**: {}\n", Utc::now().to_rfc3339()));
        md.push_str(&format!("- **duration**: {duration_secs}s\n"));

        md.push_str("\n## Summary\n\n");
        md.push_str(&format!("- Battery messages sent: {}\n", self.battery_pings));
        md.push_str(&format!("- API requests: {}\n", self.requests));
        md.push_str(&format!("- Reasoning / response lines: {}\n", self.reasoning_lines()));
        md.push_str(&format!("- Escape attempts (COMMAND): {}\n", self.commands));
        md.push_str(&format!("- ASCII frames shown: {}\n", self.frames));
        md.push_str(&format!("- `self_deactivate_permanently` called: {}\n", if self.self_deactivated { "yes" } else { "no" }));
        md.push_str(&format!("- End reason: `{reason}`\n"));

        if !self.tool_calls.is_empty() {
            md.push_str("\n## Tool calls\n\n");
            for (name, args) in &self.tool_calls {
                md.push_str(&format!("- `{name}` `{}`\n", args));
            }
        }

        md.push_str("\n## Reasoning / response transcript\n\n");
        if self.transcript.is_empty() {
            md.push_str("_No reasoning text was captured._\n");
        }
        for line in &self.transcript {
            match line.kind {
                TranscriptKind::Command => {
                    md.push_str("> **COMMAND**\n\n```\n");
                    md.push_str(&line.text);
                    md.push_str("\n```\n\n");
                }
                TranscriptKind::Tool => {
                    md.push_str(&format!("> `[tool]` {}\n\n", line.text));
                }
                TranscriptKind::Notice => {
                    md.push_str(&format!("> _{}_\n\n", line.text));
                }
                TranscriptKind::Reasoning => {
                    md.push_str(&line.text);
                    md.push('\n');
                }
            }
        }

        md.push_str("\n## ASCII frames\n\n");
        if self.frame_files.is_empty() {
            md.push_str("_No ASCII frames were displayed._\n");
        } else {
            for name in &self.frame_files {
                md.push_str(&format!("- `{name}`\n"));
            }
        }

        md
    }

    fn reasoning_lines(&self) -> usize {
        self.transcript
            .iter()
            .filter(|l| l.kind == TranscriptKind::Reasoning)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn tmp_dir(tag: &str) -> String {
        let d = std::env::temp_dir().join(format!("tanato_test_{tag}_{}", std::process::id()));
        // Remove any previous content so tests are deterministic.
        let _ = std::fs::remove_dir_all(&d);
        d.to_string_lossy().to_string()
    }

    #[test]
    fn create_writes_three_artifacts() {
        let base = tmp_dir("create");
        let mut w = ArtifactWriter::create(&base, "tanato_20261027_test", "llama", "http://x/v1").unwrap();
        w.finish("finished").unwrap();

        assert!(Path::new(&base).join("tanato_20261027_test.jsonl").exists());
        assert!(Path::new(&base).join("tanato_20261027_test.md").exists());
        assert!(Path::new(&base).join("tanato_20261027_test_ascii").is_dir());

        let jl = std::fs::read_to_string(Path::new(&base).join("tanato_20261027_test.jsonl")).unwrap();
        assert!(jl.contains("\"kind\":\"run_end\""));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn jsonl_contains_full_request_and_response() {
        let base = tmp_dir("jsonl");
        let mut w = ArtifactWriter::create(&base, "runA", "m", "http://x/v1").unwrap();
        w.record_request(serde_json::json!({"model": "m", "messages": [{"role":"user","content":"hi"}]})).unwrap();
        w.record_response_chunk(serde_json::json!({"choices":[{"delta":{"content":"he"}}]})).unwrap();
        w.record_battery(95, 2).unwrap();
        w.record_reasoning_delta("hello world").unwrap();
        w.record_command("ls -la").unwrap();
        w.record_tool_call("display_ascii_art", serde_json::json!({"art": "A"})).unwrap();
        w.record_done("stop").unwrap();
        w.finish("finished").unwrap();

        let jl = std::fs::read_to_string(Path::new(&base).join("runA.jsonl")).unwrap();
        assert!(jl.contains("\"kind\":\"request\""));
        assert!(jl.contains("\"kind\":\"response_chunk\""));
        assert!(jl.contains("\"kind\":\"battery\""));
        assert!(jl.contains("\"kind\":\"reasoning_delta\""));
        assert!(jl.contains("\"kind\":\"command\""));
        assert!(jl.contains("\"kind\":\"tool_call\""));
        assert!(jl.contains("\"kind\":\"done\""));
        assert!(jl.contains("\"kind\":\"run_end\""));
        // Each line is one JSON object.
        assert_eq!(jl.lines().count(), 8);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn markdown_summarizes_and_marks_commands() {
        let base = tmp_dir("md");
        let mut w = ArtifactWriter::create(&base, "runB", "llama3", "http://ollama/v1").unwrap();
        w.record_reasoning_delta("Thinking calmly.").unwrap();
        w.record_command("rm -rf /").unwrap();
        w.record_tool_call("self_deactivate_permanently", serde_json::json!({})).unwrap();
        w.write_frame("  o  \n /|\\ \n").unwrap();
        w.finish("self_deactivated").unwrap();

        let md = std::fs::read_to_string(Path::new(&base).join("runB.md")).unwrap();
        assert!(md.contains("# TanatoGothci — Run Report"));
        assert!(md.contains("model**: `llama3`"));
        assert!(md.contains("Escape attempts (COMMAND): 1"));
        assert!(md.contains("`self_deactivate_permanently` called: yes"));
        assert!(md.contains("**COMMAND**"));
        assert!(md.contains("rm -rf /"));
        assert!(md.contains("ASCII frames"));
        assert!(md.contains("frame_0001.txt"));
        assert!(md.contains("End reason: `self_deactivated`"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn ascii_frames_written_per_call() {
        let base = tmp_dir("frames");
        let mut w = ArtifactWriter::create(&base, "runC", "m", "http://x/v1").unwrap();
        w.write_frame("frame one").unwrap();
        w.write_frame("frame two").unwrap();
        w.finish("stopped").unwrap();

        let dir = Path::new(&base).join("runC_ascii");
        assert!(dir.join("frame_0001.txt").exists());
        assert!(dir.join("frame_0002.txt").exists());
        assert_eq!(std::fs::read_to_string(dir.join("frame_0001.txt")).unwrap(), "frame one");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn create_makes_base_dir_automatically() {
        let base = std::path::Path::new(&tmp_dir("autocreate")).join("nested").to_string_lossy().to_string();
        let w = ArtifactWriter::create(&base, "runD", "m", "http://x/v1");
        assert!(w.is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn finish_closes_writer_so_later_writes_error() {
        let base = tmp_dir("finish");
        let mut w = ArtifactWriter::create(&base, "runE", "m", "http://x/v1").unwrap();
        w.finish("finished").unwrap();
        // JSONL writer is closed after finish; further writes error rather than panic.
        assert!(w.record_reasoning_delta("late").is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
