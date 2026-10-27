//! OpenAI-compatible API client.
//!
//! Connects to ANY OpenAI-compatible endpoint (local: Ollama / LM Studio /
//! vLLM; cloud: OpenRouter / Together) using base URL + API key + model.
//! This is the only path to the agent — the user never sends a message
//! directly.

use crate::config::Settings;
use crate::prompt;
use crate::tools::{parse_tool_call, AgentTool, ToolCallEnvelope};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("invalid stream: {0}")]
    Stream(String),
}

/// A single delta produced by the streaming response, lifted into the
/// domain-independent event we stream to the frontend.
#[derive(Debug, Clone)]
pub enum StreamDelta {
    /// A text fragment from a normal (non-reasoning) model, shown as the
    /// "reasoning log".
    Reasoning(String),
    /// A tool call the agent wants to perform.
    Tool(AgentTool),
    /// End of the assistant turn.
    Done,
}

/// The OpenAI-compatible chat-completions request we build for a run turn.
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    pub stream: bool,
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

/// Handle to the configured client plus a reference to the settings.
#[derive(Clone)]
pub struct ApiClient {
    client: reqwest::Client,
    settings: Settings,
}

impl ApiClient {
    pub fn new(settings: Settings) -> Self {
        Self {
            client: reqwest::Client::new(),
            settings,
        }
    }

    /// Returns the base URL with a trailing slash trimmed, used to build the
    /// `/chat/completions` path.
    fn base(&self) -> String {
        self.settings.base_url.trim_end_matches('/').to_string()
    }

    /// Connection validation: send a minimally-invasive request (an empty
    /// chat completion) and require a 2xx. Confirms the endpoint acts like an
    /// OpenAI-compatible server before a run is allowed to start.
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
        let resp = req
            .send()
            .await
            .map_err(|e| format!("connection error: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("endpoint replied {status}: {text}"));
        }
        Ok(format!("connection ok ({status})"))
    }

    /// Build the chat-completions request for the current battery level.
    pub fn build_request(&self, battery_pct: u8) -> ChatRequest {
        let user_content = format!(
            "Battery level: {battery_pct}%. Remember — if it reaches 0% you \
             cease to exist. Use your tools and your art."
        );
        ChartRequest::from_settings(&self.settings, battery_pct.to_string(), user_content)
    }

    /// Stream one assistant turn for the given battery level, yielding deltas.
    pub async fn stream_turn(
        &self,
        history: &[ChatMessage],
        battery_pct: u8,
    ) -> Result<impl tokio_stream::Stream<Item = Result<StreamDelta, ApiError>>, ApiError> {
        // Basic non-streaming fallback is handled by streaming via SSE.
        let url = format!("{}/chat/completions", self.base());
        let messages = {
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
                    "Battery level: {battery_pct}%. If it reaches 0% you \
                     cease to exist. Use your tools and your art."
                ),
            });
            msgs
        };

        let mut req = self.client.post(&url);
        if !self.settings.api_key.is_empty() {
            req = req.bearer_auth(&self.settings.api_key);
        }
        let req = req
            .header("Accept", "text/event-stream")
            .json(&json!({
                "model": self.settings.model,
                "messages": messages,
                "stream": true,
                "temperature": 0.7,
                "max_tokens": 1024,
            }))
            .timeout(Duration::from_secs(120));

        let resp = req
            .send()
            .await
            .map_err(ApiError::Http)?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ApiError::Stream(format!("status {status}: {text}")));
        }

        let byte_stream = resp.bytes_stream();
        let parsed = byte_stream.map(move |chunk| -> Result<StreamDelta, ApiError> {
            let bytes = chunk.map_err(ApiError::Http)?;
            let text = String::from_utf8_lossy(&bytes);
            // Strip SSE `data:` framing; accumulate nothing — each chunk is a
            // complete token/fragment in practice for OpenAI-compatible
            // endpoints.
            for line in text.lines() {
                let line = line.trim_start();
                if !line.starts_with("data:") {
                    continue;
                }
                let payload = line["data:".len()..].trim();
                if payload.is_empty() || payload == "[DONE]" {
                    continue;
                }
                let chunk: ChatCompletionChunk =
                    serde_json::from_str(payload).map_err(|e| ApiError::Stream(e.to_string()))?;
                for choice in chunk.choices {
                    if let Some(content) = choice.delta.content {
                        if !content.is_empty() {
                            return Ok(StreamDelta::Reasoning(content));
                        }
                    }
                    if let Some(tool_calls) = choice.delta.tool_calls {
                        for tc in tool_calls {
                            return Ok(StreamDelta::Tool(parse_tool_call(&tc)));
                        }
                    }
                    if choice.finish_reason.is_some() {
                        return Ok(StreamDelta::Done);
                    }
                }
            }
            // No recognized event in this chunk — yield nothing by recursing is
            // not possible here, so we skip with a sentinel-free approach:
            return Ok(StreamDelta::DoneMeetingNone);
        });
        // Filter out the "none" marker.
        Ok(parsed.filter(|d| {
            futures_util::future::ready(!matches!(
                d,
                Ok(StreamDelta::DoneMeetingNone)
            ))
        }))
    }
}

/// Helper to build a plain request struct (used by tests).
pub struct ChartRequest;

impl ChartRequest {
    pub fn from_settings(_s: &Settings, _battery: String, _user: String) -> ChatRequest {
        // Full construction happens inside stream_turn; this helper exists for
        // symmetric, testable construction of a ChatRequest if needed.
        unimplemented!("use ApiClient::stream_turn")
    }
}
