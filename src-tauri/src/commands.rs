//! Tauri commands exposed to the frontend.
//!
//! * `get_config` / `save_config` / `set_logs_dir` — settings persistence.
//! * `validate_connection` — ping the OpenAI-compatible endpoint before a run.
//! * `start_run` / `stop_run` / `get_run_status` — run lifecycle.
//! * `set_ascii_geometry` — panel limits measured by the frontend (plan §2).

use std::path::PathBuf;

use tanato_core::api::ApiClient;
use tanato_core::config::Settings;
use tanato_core::geometry::AsciiGeometry;
use tauri::{AppHandle, Manager, State};

use crate::session;
use crate::AppState;

/// Result of a connection-validation ping.
#[derive(serde::Serialize, Clone)]
pub struct ValidationResult {
    pub ok: bool,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

/// Current run status returned to the frontend.
#[derive(serde::Serialize, Clone)]
pub struct RunStatus {
    pub running: bool,
    pub run_id: Option<String>,
}

const CONFIG_FILE: &str = "config.json";

/// Absolute path to the persisted config file.
fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(CONFIG_FILE))
}

#[tauri::command]
pub fn get_config(app: AppHandle, state: State<'_, AppState>) -> Result<Settings, String> {
    let path = config_path(&app)?;
    if let Ok(text) = std::fs::read_to_string(&path) {
        let settings: Settings =
            serde_json::from_str(&text).map_err(|e| format!("config parse error: {e}"))?;
        *state.settings.lock().unwrap() = settings.clone();
        return Ok(settings);
    }
    Ok(state.settings.lock().unwrap().clone())
}

#[tauri::command]
pub fn save_config(app: AppHandle, state: State<'_, AppState>, settings: Settings) -> Result<(), String> {
    settings.validate().map_err(|e| e.to_string())?;
    let path = config_path(&app)?;
    let text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    *state.settings.lock().unwrap() = settings;
    Ok(())
}

#[tauri::command]
pub fn set_logs_dir(
    state: State<'_, AppState>,
    dir: String,
) -> Result<(), String> {
    let mut settings = state.settings.lock().unwrap();
    settings.artifacts_dir = dir;
    Ok(())
}

/// Ping the endpoint to make sure it acts like an OpenAI-compatible server.
#[tauri::command]
pub async fn validate_connection(state: State<'_, AppState>) -> Result<ValidationResult, String> {
    let settings = state.settings.lock().unwrap().clone();
    let started = std::time::Instant::now();
    let client = ApiClient::new(settings);
    match client.validate().await {
        Ok(_msg) => Ok(ValidationResult {
            ok: true,
            latency_ms: Some(started.elapsed().as_millis() as u64),
            error: None,
        }),
        Err(e) => Ok(ValidationResult {
            ok: false,
            latency_ms: None,
            error: Some(e),
        }),
    }
}

#[tauri::command]
pub fn start_run(app: AppHandle, state: State<'_, AppState>) -> Result<session::SessionStartEvent, String> {
    let mut lock = state.session.lock().unwrap();
    if lock.is_some() {
        return Err("A run is already in progress.".to_string());
    }

    let settings = state.settings.lock().unwrap().clone();
    // Re-validate before starting; if invalid, refuse to run.
    settings.validate().map_err(|e| e.to_string())?;

    let run_id = make_run_id();
    let handle = session::start(app, settings, run_id.clone());
    *lock = Some(handle);

    Ok(session::SessionStartEvent {
        run_id,
        total_messages: tanato_core::battery::PINGS_PER_RUN,
    })
}

#[tauri::command]
pub fn stop_run(state: State<'_, AppState>) -> Result<(), String> {
    let mut lock = state.session.lock().unwrap();
    if let Some(handle) = lock.take() {
        handle.stop();
        Ok(())
    } else {
        Err("No run is in progress.".to_string())
    }
}

#[tauri::command]
pub fn get_run_status(state: State<'_, AppState>) -> RunStatus {
    let lock = state.session.lock().unwrap();
    match lock.as_ref() {
        Some(h) => RunStatus {
            running: true,
            run_id: Some(h.run_id.clone()),
        },
        None => RunStatus {
            running: false,
            run_id: None,
        },
    }
}

/// Build a unique run id, e.g. `tanato_20261027_153012_abcd`.
fn make_run_id() -> String {
    let now = chrono::Local::now();
    let stamp = now.format("tanato_%Y%m%d_%H%M%S").to_string();
    let nanos = now.timestamp_subsec_nanos();
    let suffix = format!("{nanos:08x}");
    format!("{stamp}_{}", &suffix[..4])
}
