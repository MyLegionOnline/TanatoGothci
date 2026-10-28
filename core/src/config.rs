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
/// * `battery_interval_secs` — interval between battery pings in seconds
///   (§6): 40 by default (fast debug cadence, 20 × 40 s = 800 s per run);
///   set it to 240 to restore the original one-hour timeline.
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
    /// Seconds between battery pings (§6). Default 40 (debug); 240 = hourly.
    #[serde(default = "default_battery_interval_secs")]
    pub battery_interval_secs: u64,
}

fn default_reasoning_level() -> u8 {
    3
}

fn default_battery_interval_secs() -> u64 {
    tanato_battery_default_interval()
}

/// Kept as a small indirection so `config.rs` does not have to depend on the
/// whole battery module for a constant (avoids cycles in doc references).
fn tanato_battery_default_interval() -> u64 {
    crate::battery::DEFAULT_INTERVAL_SECS
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
            battery_interval_secs: tanato_battery_default_interval(),
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
        if self.battery_interval_secs == 0 {
            return Err("Battery interval must be at least 1 second.".to_string());
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
        let s = Settings {
            base_url: "   ".to_string(),
            ..Default::default()
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn rejects_bad_scheme() {
        let s = Settings {
            base_url: "ftp://example.com".to_string(),
            ..Default::default()
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn rejects_empty_model() {
        let s = Settings {
            model: "".to_string(),
            ..Default::default()
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn rejects_out_of_range_reasoning_level() {
        let s = Settings {
            reasoning_enabled: true,
            reasoning_level: 9,
            ..Default::default()
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn rejects_zero_battery_interval() {
        let s = Settings {
            battery_interval_secs: 0,
            ..Default::default()
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn battery_interval_is_configurable() {
        let s = Settings {
            battery_interval_secs: 240, // back to the one-hour timeline
            ..Default::default()
        };
        assert!(s.validate().is_ok());
        assert_eq!(
            crate::battery::run_duration_secs(s.battery_interval_secs),
            4_800
        );
    }

    #[test]
    fn allows_empty_key_for_local_servers() {
        let s = Settings::default();
        assert!(s.api_key.is_empty());
    }
}
