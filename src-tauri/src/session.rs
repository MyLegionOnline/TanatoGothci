//! Run session orchestration.
//!
//! A session drives the battery countdown (21 pings, 100% → 0% at the
//! configured interval, default 40 s), sending each battery ping
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
use tanato_core::frames::{normalize_frame, FrameScanner};
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
    geometry: tanato_core::geometry::AsciiGeometry,
    run_id: String,
) -> SessionHandle {
    let cancel = CancellationToken::new();
    let cancel_task = cancel.clone();
    tauri::async_runtime::spawn(run_session(app, settings, geometry, run_id.clone(), cancel_task));
    SessionHandle { run_id, cancel }
}

/// The body of a session task: battery countdown → per-ping API stream.
async fn run_session(
    app: AppHandle,
    settings: Settings,
    geometry: tanato_core::geometry::AsciiGeometry,
    run_id: String,
    cancel: CancellationToken,
) {
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

    let mut client = ApiClient::new(settings.clone());
    // Adopt the panel geometry measured by the frontend (falls back to the
    // 55×14 default until `set_ascii_geometry` has been called).
    let _ = client.set_geometry(geometry);

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

    let completed = battery::run_countdown_with(settings.battery_interval_secs.max(1), |level, seq| {
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
            let (rx, body) = match client.stream_turn(&[], level).await {
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
            // Fallback frame source: the model often draws ASCII art directly
            // in its text (run tanato_20261028_223254_2b1e: ASCII frames
            // shown: 0) instead of calling display_ascii_art. Scan the text
            // stream; frames found here are emitted in addition to the tool
            // path, deduplicated against tool frames by normalized content.
            // The raw text is NOT filtered — reasoning:delta stays unchanged.
            let mut frame_scanner = FrameScanner::new();
            let mut emitted_frames: Vec<String> = Vec::new();
            while let Some(item) = rx.recv().await {
                match item {
                    Ok(StreamDelta::Raw(raw)) => {
                        // Full response chunk — persisted only, not shown in UI.
                        if let Some(w) = writer.lock().unwrap().as_mut() {
                            let _ = w.record_response_chunk(raw);
                        }
                    }
                    Ok(StreamDelta::Reasoning(text)) => {
                        for frame in frame_scanner.feed(&text) {
                            emit_frame(
                                &app, &writer, &mut emitted_frames, client.geometry(), frame.text,
                            );
                        }
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
                                let geo = client.geometry();
                                if let Some(warning) = tanato_core::geometry::frame_exceeds(geo, &art) {
                                    if let Some(w) = writer.lock().unwrap().as_mut() {
                                        let _ = w.record_notice(&warning);
                                    }
                                    emit(&app, "reasoning:delta", ReasoningDeltaEvent {
                                        text: format!("[warning] {warning}"),
                                        kind: LineKind::Notice,
                                    });
                                }
                                let normalized = normalize_frame(&art);
                                let already_shown = emitted_frames.contains(&normalized);
                                emitted_frames.push(normalized);
                                if let Some(w) = writer.lock().unwrap().as_mut() {
                                    let _ = w.record_tool_call("display_ascii_art", serde_json::json!({ "art": art.clone() }));
                                    if !already_shown {
                                        let _ = w.write_frame(&art);
                                    }
                                }
                                emit(&app, "tool:call", ToolCallEvent {
                                    name: "display_ascii_art".into(),
                                    args: serde_json::json!({ "art": art.clone() }),
                                });
                                if !already_shown {
                                    emit(&app, "ascii:frame", AsciiFrameEvent { text: art });
                                }
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
                        for frame in frame_scanner.finish() {
                            emit_frame(
                                &app, &writer, &mut emitted_frames, client.geometry(), frame.text,
                            );
                        }
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
            !deactivated_flag.load(Ordering::SeqCst)
        }
    })
    .await;

    // Derive the end reason from how the countdown ended.
    let deactivated = deactivated_flag.load(Ordering::SeqCst);
    let api_error = api_error_flag.load(Ordering::SeqCst);
    let why: &str = battery::end_reason(completed, deactivated, api_error);
    {
        let mut writer_final = writer.lock().unwrap();
        if let Some(w) = writer_final.as_mut() {
            if let Err(err) = w.finish(why) {
                emit(&app, "reasoning:delta", ReasoningDeltaEvent {
                    text: format!("[notice] could not finalize run artifacts: {err}"),
                    kind: LineKind::Notice,
                });
            }
        }
    }
    emit(&app, "session:end", SessionEndEvent { run_id, reason: why.to_string() });
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

/// Emit a text-derived ASCII frame once (dedup by normalized content against
/// frames already delivered via the display_ascii_art tool). Also writes it
/// to the run artifacts as a frame.
fn emit_frame(
    app: &AppHandle,
    writer: &Arc<Mutex<Option<ArtifactWriter>>>,
    emitted: &mut Vec<String>,
    geo: tanato_core::geometry::AsciiGeometry,
    text: String,
) {
    let normalized = normalize_frame(&text);
    if emitted.contains(&normalized) {
        return;
    }
    emitted.push(normalized);
    // A frame exceeding the panel limits is NOT dropped or trimmed: log the
    // excess and emit it anyway (a crooked art is better than an empty panel).
    if let Some(warning) = tanato_core::geometry::frame_exceeds(geo, &text) {
        if let Some(w) = writer.lock().unwrap().as_mut() {
            let _ = w.record_notice(&warning);
        }
        emit(app, "reasoning:delta", ReasoningDeltaEvent {
            text: format!("[warning] {warning}"),
            kind: LineKind::Notice,
        });
    }
    if let Some(w) = writer.lock().unwrap().as_mut() {
        let _ = w.write_frame(&text);
    }
    emit(app, "ascii:frame", AsciiFrameEvent { text });
}
