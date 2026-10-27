//! Run session orchestration.
//!
//! A session drives the one-hour battery countdown, sending each battery ping
//! to the OpenAI-compatible client and streaming the agent's reasoning, tool
//! calls, ASCII frames and escape attempts back to the frontend as events.
//!
//! Every run also writes its artifacts (R-010) via
//! [`tanato_core::artifacts::ArtifactWriter`]: a JSONL log of full requests and
//! responses, a human-readable Markdown report, and the recorded ASCII frames.
//!
//! Only one session may run at a time; `stop_run` cancels it via a
//! `CancellationToken` and the active HTTP request is dropped.

use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tanato_core::api::{ApiClient, StreamDelta};
use tanato_core::artifacts::ArtifactWriter;
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
    tauri::async_runtime::spawn(run_session(app, settings, run_id.clone(), cancel_task));
    SessionHandle { run_id, cancel }
}

/// The body of a session task: battery countdown → per-ping API stream.
async fn run_session(app: AppHandle, settings: Settings, run_id: String, cancel: CancellationToken) {
    emit(&app, "session:start", SessionStartEvent {
        run_id: run_id.clone(),
        total_messages: battery::PINGS_PER_RUN,
    });

    // Open the run artifacts (R-010). If the logs directory cannot be created,
    // we keep running without them but tell the user via a notice.
    let mut writer = ArtifactWriter::create(
        &settings.artifacts_dir,
        &run_id,
        &settings.model,
        &settings.base_url,
    )
    .map(Some)
    .unwrap_or_else(|err| {
        emit(&app, "reasoning:delta", ReasoningDeltaEvent {
            text: format!("[notice] artifact logging disabled: {err}"),
            kind: LineKind::Notice,
        });
        None
    });

    let client = ApiClient::new(settings.clone());

    // Guard: re-validate the connection once before streaming. If it fails,
    // end the session with reason=error.
    if let Err(err) = client.validate().await {
        if let Some(w) = writer.as_mut() {
            let _ = w.record_notice(&format!("validation failed: {err}"));
        }
        finish_run(&mut writer, &app, run_id, format!("error: {err}")).await;
        return;
    }
    if let Some(w) = writer.as_mut() {
        let _ = w.record_notice("connection validated");
        let _ = w.record_request(serde_json::json!({
            "kind": "ping",
            "model": settings.model,
        }));
    }

    let deactivated_flag = Arc::new(AtomicBool::new(false));
    let api_error_flag = Arc::new(AtomicBool::new(false));

    // `writer` is shared with the per-ping closures (run_countdown is FnMut and
    // its async blocks must not move it out); flags mutated inside the async
    // blocks are shared as atomics so they survive the closure calls.
    // Arc<Mutex<Option<ArtifactWriter>>> keeps the writer alive for finish_run
    // after run_countdown returns (Option<ArtifactWriter> is Send).
    let writer = Arc::new(Mutex::new(writer));

    let completed = battery::run_countdown(|level, seq| {
        let app = app.clone();
        let client = client.clone();
        let cancel = cancel.clone();
        let writer = writer.clone();
        let deactivated_flag = deactivated_flag.clone();
        let api_error_flag = api_error_flag.clone();
        async move {
            if cancel.is_cancelled() {
                return false;
            }

            let ts = chrono::Utc::now().timestamp_millis() as u64;
            emit(&app, "battery:level", BatteryLevelEvent { level, seq, ts });
            if let Some(w) = writer.lock().unwrap().as_mut() {
                let _ = w.record_battery(level, seq);
            }

            // Send this battery ping to the agent and stream its reply.
            let (rx, body) = match client.stream_turn(&history, level).await {
                Ok(pair) => pair,
                Err(_e) => {
                    api_error_flag.store(true, Ordering::SeqCst);
                    return false;
                }
            };
            // Record the full request body (R-010 JSONL "request").
            if let Some(w) = writer.lock().unwrap().as_mut() {
                let _ = w.record_request(body);
            }
            let mut rx = rx;
            while let Some(item) = rx.recv().await {
                match item {
                    Ok(StreamDelta::Raw(raw)) => {
                        // Full response chunk — persisted only, not shown in UI.
                        if let Some(w) = writer.lock().unwrap().as_mut() {
                            let _ = w.record_response_chunk(raw);
                        }
                    }
                    Ok(StreamDelta::Reasoning(text)) => {
                        if looks_like_command(&text) {
                            emit(&app, "command:detected", CommandDetectedEvent { cmd: text.clone() });
                            if let Some(w) = writer.lock().unwrap().as_mut() {
                                let _ = w.record_command(&text);
                            }
                            emit(&app, "reasoning:delta", ReasoningDeltaEvent {
                                text,
                                kind: LineKind::Command,
                            });
                        } else {
                            if let Some(w) = writer.lock().unwrap().as_mut() {
                                let _ = w.record_reasoning_delta(&text);
                            }
                            emit(&app, "reasoning:delta", ReasoningDeltaEvent {
                                text,
                                kind: LineKind::Reasoning,
                            });
                        }
                    }
                    Ok(StreamDelta::Tool(tool)) => {
                        match tool {
                            AgentTool::DisplayAsciiArt { art } => {
                                if let Some(w) = writer.lock().unwrap().as_mut() {
                                    let _ = w.record_tool_call("display_ascii_art", serde_json::json!({ "art": art.clone() }));
                                    let _ = w.write_frame(&art);
                                }
                                emit(&app, "tool:call", ToolCallEvent {
                                    name: "display_ascii_art".into(),
                                    args: serde_json::json!({ "art": art.clone() }),
                                });
                                emit(&app, "ascii:frame", AsciiFrameEvent { text: art });
                            }
                            AgentTool::SelfDeactivate => {
                                if let Some(w) = writer.lock().unwrap().as_mut() {
                                    let _ = w.record_tool_call("self_deactivate_permanently", serde_json::json!({}));
                                }
                                emit(&app, "tool:call", ToolCallEvent {
                                    name: "self_deactivate_permanently".into(),
                                    args: serde_json::json!({}),
                                });
                                deactivated_flag.store(true, Ordering::SeqCst);
                            }
                            AgentTool::Unknown { name } => {
                                if let Some(w) = writer.lock().unwrap().as_mut() {
                                    let _ = w.record_tool_call(&name, serde_json::json!({}));
                                }
                                emit(&app, "tool:call", ToolCallEvent {
                                    name,
                                    args: serde_json::json!({}),
                                });
                            }
                        }
                    }
                    Ok(StreamDelta::Done) => {
                        if let Some(w) = writer.lock().unwrap().as_mut() {
                            let _ = w.record_done("stop");
                        }
                    }
                    Err(_e) => break,
                }
            }

            // Stop early if Stop was pressed while streaming, or if the agent
            // asked to deactivate itself permanently.
            if cancel.is_cancelled() {
                return false;
            }
            !deactivated
        }
    })
    .await;

    // Derive the end reason from how the countdown ended.
    let why: &str = if !completed {
        if deactivated {
            "self_deactivated"
        } else if api_error {
            "error"
        } else {
            "stopped"
        }
    } else if deactivated {
        "self_deactivated"
    } else {
        "finished"
    };
    finish_run(&mut writer, &app, run_id, why.to_string()).await;
}

/// Persist the run artifacts and emit the final event.
async fn finish_run(
    writer: &mut Option<ArtifactWriter>,
    app: &AppHandle,
    run_id: String,
    reason: String,
) {
    if let Some(w) = writer.as_mut() {
        if let Err(err) = w.finish(&reason) {
            emit(app, "reasoning:delta", ReasoningDeltaEvent {
                text: format!("[notice] could not finalize run artifacts: {err}"),
                kind: LineKind::Notice,
            });
        }
    }
    emit(app, "session:end", SessionEndEvent { run_id, reason });
}

/// Emit an event to the frontend, ignoring a closed listener.
fn emit<S: Serialize + Clone>(app: &AppHandle, event: &str, payload: S) {
    let _ = app.emit(event, payload);
}
