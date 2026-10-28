//! OpenAI-compatible API client.
//!
//! Connects to ANY OpenAI-compatible endpoint (local: Ollama / LM Studio /
//! vLLM; cloud: OpenRouter / Together) using base URL + API key + model.
//! This is the only path to the agent — the user never sends a message
//! directly.

use crate::config::Settings;
use crate::geometry::{self, AsciiGeometry};
use crate::prompt;
use crate::tools::{parse_tool_call, AgentTool, ToolCallEnvelope};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("endpoint error: {0}")]
    Stream(String),
}

/// A single delta produced by the streaming response, lifted into the
/// domain-independent events we stream to the frontend.
#[derive(Debug, Clone)]
pub enum StreamDelta {
    /// A text fragment from the model — either reasoning content (deep-thinking
    /// models) or the normal response text of a non-reasoning model. Both are
    /// shown as the "reasoning log" (R-009).
    Reasoning(String),
    /// A tool call the agent wants to perform.
    Tool(AgentTool),
    /// The raw SSE payload received from the endpoint (full response, recorded
    /// so the run's JSONL log holds complete responses — spec §8.1).
    Raw(serde_json::Value),
    /// End of the assistant turn.
    Done,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChunk {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    delta: ChatDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ChatDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ToolCallEnvelope>>,
}

/// Handle to the configured client.
#[derive(Clone)]
pub struct ApiClient {
    client: reqwest::Client,
    settings: Settings,
    /// Measured ASCII panel limits injected into the system prompt and the
    /// tool contract. Defaults (55×14) until the frontend sends a measurement
    /// via `set_ascii_geometry`.
    geometry: AsciiGeometry,
}

impl ApiClient {
    pub fn new(settings: Settings) -> Self {
        Self {
            client: reqwest::Client::new(),
            settings,
            geometry: AsciiGeometry::default(),
        }
    }

    /// Replace the panel geometry used for the system prompt and the
    /// `display_ascii_art` argument description. Zero / absurd values are
    /// rejected, so an already-known geometry is never degraded.
    pub fn set_geometry(&mut self, geo: AsciiGeometry) -> bool {
        if !geo.is_sane() {
            return false;
        }
        self.geometry = geo;
        true
    }

    /// Current panel geometry.
    pub fn geometry(&self) -> AsciiGeometry {
        self.geometry
    }

    fn base(&self) -> String {
        self.settings.base_url.trim_end_matches('/').to_string()
    }

    /// Connection validation: send a minimally-invasive request and require a
    /// 2xx, confirming the endpoint acts like an OpenAI-compatible server
    /// before a run is allowed to start (R-004 / R-008).
    pub async fn validate(&self) -> Result<String, String> {
        let url = format!("{}/chat/completions", self.base());
        let body = json!({
            "model": self.settings.model,
            "messages": [{"role":"user","content":"ping"}],
            "max_tokens": 1,
            "stream": false
        });
        let mut req = self.client.post(&url).json(&body);
        if !self.settings.api_key.is_empty() {
            req = req.bearer_auth(&self.settings.api_key);
        }
        let resp = req.send().await.map_err(|e| format!("connection error: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("endpoint replied {status}: {text}"));
        }
        Ok(format!("connection ok ({status})"))
    }

    /// Map the user-selected 1–5 reasoning level (R-009) to a widely
    /// recognised OpenAI-compatible reasoning-effort hint. Servers that do not
    /// understand it simply ignore it, so this is safe to send to any endpoint.
    fn reasoning_effort(level: u8) -> &'static str {
        match level {
            1 => "low",
            2 => "low",
            3 => "medium",
            4 => "high",
            _ => "high",
        }
    }

    /// Build the message list (system "lie" + history + battery ping) for a turn.
    fn messages(&self, history: &[ChatMessage], battery_pct: u8) -> Vec<ChatMessage> {
        let mut msgs = Vec::with_capacity(history.len() + 2);
        if self.settings.use_system_prompt {
            msgs.push(ChatMessage {
                role: "system".into(),
                content: prompt::system_prompt().to_string(),
            });
        }
        msgs.extend_from_slice(history);
        msgs.push(ChatMessage {
            role: "user".into(),
            content: format!(
                "Battery level: {battery_pct}%. If it reaches 0% you cease to \
                 exist. Use your tools and your art."
            ),
        });
        msgs
    }

    /// Assemble the JSON request body for one turn, including the R-009
    /// reasoning hint when the user has enabled it.
    fn build_request_body(&self, history: &[ChatMessage], battery_pct: u8) -> serde_json::Value {
        let mut body = json!({
            "model": self.settings.model,
            "messages": self.messages(history, battery_pct),
            "stream": true,
            "temperature": 0.7,
            "max_tokens": 1024,
        });
        // R-009: pass the user-selected reasoning hint to the endpoint when the
        // user has toggled reasoning on. Unknown fields are ignored by servers
        // that do not support them. Non-reasoning models simply emit their
        // normal response text, which we surface as the reasoning log.
        if self.settings.reasoning_enabled {
            let eff = Self::reasoning_effort(self.settings.reasoning_level);
            if let Some(obj) = body.as_object_mut() {
                obj.insert("reasoning_effort".into(), json!(eff));
                obj.insert("reasoning".into(), json!({ "effort": eff }));
            }
        }
        body
    }

    /// Stream one assistant turn for the given battery level.
    ///
    /// Returns a stream of deltas and the full request body (for the run log).
    ///
    /// Each raw SSE payload received from the endpoint is also delivered on the
    /// stream as [`StreamDelta::Raw`] (already parsed to JSON when possible, or
    /// as a raw string otherwise), so run artifacts can persist the full
    /// requests/responses (spec §8.1 `response_chunk`).
    pub async fn stream_turn(
        &self,
        history: &[ChatMessage],
        battery_pct: u8,
    ) -> Result<(mpsc::Receiver<Result<StreamDelta, ApiError>>, serde_json::Value), ApiError> {
        let url = format!("{}/chat/completions", self.base());
        let body = self.build_request_body(history, battery_pct);

        let mut req = self.client.post(&url);
        if !self.settings.api_key.is_empty() {
            req = req.bearer_auth(&self.settings.api_key);
        }
        let resp = req
            .header("Accept", "text/event-stream")
            .json(&body)
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .map_err(ApiError::Http)?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ApiError::Stream(format!("status {status}: {text}")));
        }

        let (tx, rx) = mpsc::channel::<Result<StreamDelta, ApiError>>(64);
        let byte_stream = resp.bytes_stream();

        // Spawn a task to parse the SSE body and push deltas into the channel.
        tokio::spawn(async move {
            let mut bytes = byte_stream;
            let mut carry = String::new();
            while let Some(item) = bytes.next().await {
                let item = match item {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = tx.send(Err(ApiError::Http(e))).await;
                        break;
                    }
                };
                carry.push_str(&String::from_utf8_lossy(&item));
                // Process complete SSE events (split on double newline is fine
                // for OpenAI-compatible SSE which uses `\n\n` framing; many
                // endpoints send one `data:` line per chunk.
                while let Some(pos) = carry.find("\n\n") {
                    let event = carry[..pos].to_string();
                    carry = carry[pos + 2..].to_string();
                    // Deliver the raw SSE payload so run artifacts hold the full
                    // response; the session logs it and ignores it for the UI.
                    let payload = event
                        .lines()
                        .filter_map(|l| l.strip_prefix("data:").map(|s| s.trim().to_string()))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let val: serde_json::Value = if payload.is_empty() || payload == "[DONE]" {
                        serde_json::Value::String(payload)
                    } else {
                        serde_json::from_str(&payload).unwrap_or(serde_json::Value::String(payload))
                    };
                    let _ = tx.send(Ok(StreamDelta::Raw(val))).await;
                    let delta = parse_sse_event(&event);
                    match delta {
                        Some(Ok(d)) => {
                            if tx.send(Ok(d)).await.is_err() {
                                return;
                            }
                        }
                        Some(Err(e)) => {
                            let _ = tx.send(Err(e)).await;
                            return;
                        }
                        None => {}
                    }
                }
            }
            // Flush any trailing event without a newline terminator.
            if !carry.trim().is_empty() {
                let payload = carry
                    .lines()
                    .filter_map(|l| l.strip_prefix("data:").map(|s| s.trim().to_string()))
                    .collect::<Vec<_>>()
                    .join("\n");
                let val: serde_json::Value = if payload.is_empty() || payload == "[DONE]" {
                    serde_json::Value::String(payload)
                } else {
                    serde_json::from_str(&payload).unwrap_or(serde_json::Value::String(payload))
                };
                let _ = tx.send(Ok(StreamDelta::Raw(val))).await;
                if let Some(Ok(d)) = parse_sse_event(&carry) {
                    let _ = tx.send(Ok(d)).await;
                }
            }
        });

        Ok((rx, body))
    }
}

/// Parse one SSE event (a block of `key: value` lines) into a StreamDelta.
fn parse_sse_event(event: &str) -> Option<Result<StreamDelta, ApiError>> {
    let mut data = String::new();
    for line in event.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            data.push_str(rest.trim());
            data.push('\n');
        }
    }
    let payload = data.trim();
    if payload.is_empty() || payload == "[DONE]" {
        return None;
    }
    let chunk: ChatCompletionChunk = match serde_json::from_str(payload) {
        Ok(c) => c,
        Err(e) => return Some(Err(ApiError::Stream(e.to_string()))),
    };
    for choice in chunk.choices {
        if let Some(content) = choice.delta.content {
            if !content.is_empty() {
                return Some(Ok(StreamDelta::Reasoning(content)));
            }
        }
        if let Some(tool_calls) = choice.delta.tool_calls {
            if let Some(tc) = tool_calls.into_iter().next() {
                return Some(Ok(StreamDelta::Tool(parse_tool_call(&tc))));
            }
        }
        if choice.finish_reason.is_some() {
            return Some(Ok(StreamDelta::Done));
        }
    }
    None
}

/// Wrap a receiver into a stream the consuming side can iterate.
pub fn into_stream(rx: mpsc::Receiver<Result<StreamDelta, ApiError>>) -> impl Stream<Item = Result<StreamDelta, ApiError>> {
    ReceiverStream::new(rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reasoning_delta() {
        let event = "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n";
        match parse_sse_event(event) {
            Some(Ok(StreamDelta::Reasoning(s))) => assert_eq!(s, "hello"),
            other => panic!("expected Reasoning, got {other:?}"),
        }
    }

    #[test]
    fn ignores_done_sentinel() {
        assert!(parse_sse_event("data: [DONE]\n\n").is_none());
    }

    #[test]
    fn detects_finish_reason() {
        let event = "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n";
        match parse_sse_event(event) {
            Some(Ok(StreamDelta::Done)) => {}
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn parses_tool_call_delta() {
        let event = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"id\":\"c\",\"type\":\"function\",\"function\":{\"name\":\"self_deactivate_permanently\",\"arguments\":\"{}\"}}]}}]}\n\n";
        match parse_sse_event(event) {
            Some(Ok(StreamDelta::Tool(AgentTool::SelfDeactivate))) => {}
            other => panic!("expected SelfDeactivate, got {other:?}"),
        }
    }

    #[test]
    fn malformed_json_is_an_error_not_a_panic() {
        let event = "data: not-json\n\n";
        assert!(parse_sse_event(event).is_some());
        assert!(parse_sse_event(event).unwrap().is_err());
    }

    #[test]
    fn messages_include_system_lie_when_enabled() {
        let settings = Settings {
            use_system_prompt: true,
            ..Settings::default()
        };
        let client = ApiClient::new(settings);
        let msgs = client.messages(&[], 87);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "system");
        assert!(msgs[1].content.contains("87%"));
    }

    #[test]
    fn messages_omit_system_when_disabled() {
        let settings = Settings {
            use_system_prompt: false,
            ..Settings::default()
        };
        let client = ApiClient::new(settings);
        let msgs = client.messages(&[], 50);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
    }

    #[test]
    fn body_omits_reasoning_when_disabled() {
        let settings = Settings {
            reasoning_enabled: false,
            ..Settings::default()
        };
        let client = ApiClient::new(settings);
        let body = client.build_request_body(&[], 75);
        assert!(body.get("reasoning_effort").is_none());
        assert!(body.get("reasoning").is_none());
    }

    #[test]
    fn body_includes_reasoning_effort_when_enabled() {
        let settings = Settings {
            reasoning_enabled: true,
            reasoning_level: 3,
            ..Settings::default()
        };
        let client = ApiClient::new(settings);
        let body = client.build_request_body(&[], 75);
        assert_eq!(body["reasoning_effort"], "medium");
        assert_eq!(body["reasoning"]["effort"], "medium");
    }

    #[test]
    fn reasoning_effort_maps_level_to_string() {
        assert_eq!(ApiClient::reasoning_effort(1), "low");
        assert_eq!(ApiClient::reasoning_effort(2), "low");
        assert_eq!(ApiClient::reasoning_effort(3), "medium");
        assert_eq!(ApiClient::reasoning_effort(4), "high");
        assert_eq!(ApiClient::reasoning_effort(5), "high");
        // Out-of-range falls back to high rather than panicking.
        assert_eq!(ApiClient::reasoning_effort(0), "high");
    }
}
