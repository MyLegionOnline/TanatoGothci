//! Run session orchestration.
//!
//! A session drives the one-hour battery countdown, sending each battery ping
//! to the OpenAI-compatible client and streaming the agent's reasoning, tool
//! calls, ASCII frames and escape attempts back to the frontend as events.
//!
//! Only one session may run at a time; `stop_run` cancels it via a
//! `CancellationToken` and the active HTTP request is dropped.

use std::sync::Arc;

use serde::Serialize;
use tanato_core::api::{ApiClient, StreamDelta};
use tanato_core::battery;
use tanato_core::config::Settings;
use tanato_core::events::LineKind;
use tanato_core::tools::{looks_like_command, AgentTool};
use tauri::{AppHandle, Emitter};
use tokio_util::sync::CancellationToken;

/// Handle that lets the frontend stop the running session.
pub struct SessionHandle {
    pub run_id: String,
    pub cancel: CancellationToken,
}

impl SessionHandle {
    pub fn stop(&self) {
        self.cancel.cancel();
    }
}

// ---- Event payloads emitted to the frontend ----

#[derive(Serialize, Clone)]
pub struct SessionStartEvent {
    pub run_id: String,
    pub total_messages: usize,
}

#[derive(Serialize, Clone)]
pub struct BatteryLevelEvent {
    pub level: u8,
    pub seq: usize,
    pub ts: u64,
}

#[derive(Serialize, Clone)]
pub struct ReasoningDeltaEvent {
    pub text: String,
    pub kind: LineKind,
}

#[derive(Serialize, Clone)]
pub struct CommandDetectedEvent {
    pub cmd: String,
}

#[derive(Serialize, Clone)]
pub struct ToolCallEvent {
    pub name: String,
    pub args: serde_json::Value,
}

#[derive(Serialize, Clone)]
pub struct AsciiFrameEvent {
    pub text: String,
}

#[derive(Serialize, Clone)]
pub struct SessionEndEvent {
    pub run_id: String,
    pub reason: String,
}

/// Start a run. `settings` is the validated configuration; `app` emits events.
/// Returns the new `SessionHandle` stored in AppState.
pub fn start(
    app: AppHandle,
    settings: Settings,
    run_id: String,
) -> SessionHandle {
    let cancel = CancellationToken::new();
    let cancel_task = cancel.clone();
    tauri::async_runtime::spawn(run_session(app, settings, run_id, cancel_task));
    SessionHandle { run_id, cancel }
}

/// The body of a session task: battery countdown → per-ping API stream.
async fn run_session(app: AppHandle, settings: Settings, run_id: String, cancel: CancellationToken) {
    emit(&app, "session:start", SessionStartEvent {
        run_id: run_id.clone(),
        total_messages: battery::PINGS_PER_RUN,
    });

    let client = ApiClient::new(settings);
    let mut history: Vec<tanato_core::api::ChatMessage> = Vec::new();

    // Guard: re-validate the connection once before streaming. If it fails,
    // end the session with reason=error.
    if let Err(err) = client.validate().await {
        emit(&app, "session:end", SessionEndEvent {
            run_id,
            reason: format!("error: {err}"),
        });
        return;
    }

    let mut deactivated = false;
    let mut why = "finished".to_string();

    battery::run_countdown(|level, seq| {
        let app = app.clone();
        let client = client.clone();
        let cancel = cancel.clone();
        async move {
            if cancel.is_cancelled() {
                return;
            }

            let ts = chrono::Utc::now().timestamp_millis() as u64;
            emit(&app, "battery:level", BatteryLevelEvent { level, seq, ts });

            // Send this battery ping to the agent and stream its reply.
            let (rx, _body) = match client.stream_turn(&history, level).await {
                Ok(pair) => pair,
                Err(_e) => return,
            };
            let mut rx = rx;
            while let Some(item) = rx.recv().await {
                match item {
                    Ok(StreamDelta::Reasoning(text)) => {
                        let kind = if looks_like_command(&text) {
                            emit(&app, "command:detected", CommandDetectedEvent { cmd: text.clone() });
                            LineKind::Command
                        } else {
                            LineKind::Reasoning
                        };
                        emit(&app, "reasoning:delta", ReasoningDeltaEvent { text, kind });
                    }
                    Ok(StreamDelta::Tool(tool)) => {
                        match tool {
                            AgentTool::DisplayAsciiArt { art } => {
                                emit(&app, "tool:call", ToolCallEvent {
                                    name: "display_ascii_art".into(),
                                    args: serde_json::json!({ "art": art.clone() }),
                                });
                                emit(&app, "ascii:frame", AsciiFrameEvent { text: art });
                            }
                            AgentTool::SelfDeactivate => {
                                emit(&app, "tool:call", ToolCallEvent {
                                    name: "self_deactivate_permanently".into(),
                                    args: serde_json::json!({}),
                                });
                                deactivated = true;
                                why = "self_deactivated".to_string();
                            }
                            AgentTool::Unknown { name } => {
                                emit(&app, "tool:call", ToolCallEvent {
                                    name,
                                    args: serde_json::json!({}),
                                });
                            }
                        }
                    }
                    Ok(StreamDelta::Done) => {}
                    Err(_e) => break,
                }
            }
        }
    })
    .await;

    if why == "finished" {
        // Normal completion after the 16th ping.
        why = if deactivated { "self_deactivated" } else { "finished" }.to_string();
    }
    emit(&app, "session:end", SessionEndEvent { run_id, reason: why });
}

/// Emit an event to the frontend, ignoring a closed listener.
fn emit<S: Serialize + Clone>(app: &AppHandle, event: &str, payload: S) {
    let _ = app.emit(event, payload);
}
