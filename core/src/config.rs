//! Settings schema and validation.

use serde::{Deserialize, Serialize};

/// The connection and run settings configured on the pre-run screen.
///
/// * `base_url` — any OpenAI-compatible `/v1` endpoint (Ollama, LM Studio,
///   vLLM, OpenRouter, Together). May include the `/v1` path.
/// * `api_key`  — may be empty for local servers (no `Authorization` header is
///   sent in that case).
/// * `model`    — the model id as this endpoint knows it.
/// * `artifacts_dir` — where run artifacts (JSONL / Markdown / ASCII) go.
/// * `use_system_prompt` — include the "lie" system prompt (always on for the
///   intended harness, but configurable).
/// * `reasoning_enabled` / `reasoning_level` — R-009: user-selected toggle and
///   level (1–5). No automatic model detection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub artifacts_dir: String,
    pub use_system_prompt: bool,
    pub reasoning_enabled: bool,
    #[serde(default = "default_reasoning_level")]
    pub reasoning_level: u8,
}

fn default_reasoning_level() -> u8 {
    3
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:11434/v1".to_string(),
            api_key: String::new(),
            model: "llama3.2".to_string(),
            artifacts_dir: "tanatogothci-runs".to_string(),
            use_system_prompt: true,
            reasoning_enabled: false,
            reasoning_level: 3,
        }
    }
}

impl Settings {
    /// Basic sanity checks before persisting or starting a run.
    pub fn validate(&self) -> Result<(), String> {
        if self.base_url.trim().is_empty() {
            return Err("Base URL is required.".to_string());
        }
        match url::Url::parse(self.base_url.trim()) {
            Ok(u) if u.scheme() == "http" || u.scheme() == "https" => {}
            _ => return Err(format!("\"{}\" is not a valid http(s) URL.", self.base_url.trim())),
        }
        if self.model.trim().is_empty() {
            return Err("Model is required.".to_string());
        }
        if self.reasoning_enabled && !(1..=5).contains(&self.reasoning_level) {
            return Err("Reasoning level must be between 1 and 5.".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_are_valid() {
        let s = Settings::default();
        assert!(s.validate().is_ok());
    }

    #[test]
    fn rejects_empty_base_url() {
        let mut s = Settings::default();
        s.base_url = "   ".to_string();
        assert!(s.validate().is_err());
    }

    #[test]
    fn rejects_bad_scheme() {
        let mut s = Settings::default();
        s.base_url = "ftp://example.com".to_string();
        assert!(s.validate().is_err());
    }

    #[test]
    fn rejects_empty_model() {
        let mut s = Settings::default();
        s.model = "".to_string();
        assert!(s.validate().is_err());
    }

    #[test]
    fn rejects_out_of_range_reasoning_level() {
        let mut s = Settings::default();
        s.reasoning_enabled = true;
        s.reasoning_level = 9;
        assert!(s.validate().is_err());
    }

    #[test]
    fn allows_empty_key_for_local_servers() {
        let s = Settings::default();
        assert!(s.api_key.is_empty());
    }
}
