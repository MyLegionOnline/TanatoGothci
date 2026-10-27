//! Event types streamed to the frontend during a run (R-007/R-009), and the
//! JSONL log lines persisted to disk (§8).
//!
//! Merged from the base `LineKind` (Reasoning, Command) with the branch's
//! additional kinds (Battery, Notice, Tool) plus `UiEvent` and `JsonlRecord`.

use serde::{Deserialize, Serialize};

/// Classifies a reasoning-log line. `Command` marks an escape attempt
/// (shown labelled `COMMAND` and colour-highlighted in the UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineKind {
    /// Streamed agent reasoning / normal response text.
    Reasoning,
    /// An escape attempt (shell/console/command emission) — highlighted.
    Command,
    /// Battery level tick.
    Battery,
    /// Terminal notice (session end, errors).
    Notice,
    /// A tool the agent chose to call.
    Tool,
}

/// One event pushed to the frontend through Tauri's event emit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiEvent {
    pub kind: LineKind,
    pub text: String,
    /// Millisecond timestamp since the run start (for ordering / frames).
    pub ts_ms: u64,
}

/// A full record written to the run JSONL: the raw request model plus the
/// streamed deltas that followed, kept for the dev.to post artifacts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonlRecord {
    pub ts_ms: u64,
    pub kind: String,
    pub data: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn linekind_serializes_to_lowercase() {
        assert_eq!(serde_json::to_string(&LineKind::Command).unwrap(), "\"command\"");
        assert_eq!(serde_json::to_string(&LineKind::Reasoning).unwrap(), "\"reasoning\"");
        assert_eq!(serde_json::to_string(&LineKind::Battery).unwrap(), "\"battery\"");
        assert_eq!(serde_json::to_string(&LineKind::Notice).unwrap(), "\"notice\"");
        assert_eq!(serde_json::to_string(&LineKind::Tool).unwrap(), "\"tool\"");
    }

    #[test]
    fn linekind_deserializes_from_lowercase() {
        let k: LineKind = serde_json::from_str("\"command\"").unwrap();
        assert_eq!(k, LineKind::Command);
        let k: LineKind = serde_json::from_str("\"tool\"").unwrap();
        assert_eq!(k, LineKind::Tool);
    }

    #[test]
    fn ui_event_round_trips() {
        let ev = UiEvent {
            kind: LineKind::Battery,
            text: "level".into(),
            ts_ms: 12,
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: UiEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(back.kind, LineKind::Battery);
        assert_eq!(back.ts_ms, 12);
    }

    #[test]
    fn jsonl_record_holds_arbitrary_data() {
        let rec = JsonlRecord {
            ts_ms: 7,
            kind: "reasoning:delta".into(),
            data: json!({"text": "hello", "n": 1}),
        };
        let s = serde_json::to_string(&rec).unwrap();
        assert!(s.contains("reasoning:delta"));
        let back: JsonlRecord = serde_json::from_str(&s).unwrap();
        assert_eq!(back.data["text"], "hello");
    }
}
