//! TanatoGothci — Tauri v2 application shell.
//!
//! The Rust backend exposes commands to the webview frontend:
//! configuration (`get_config`/`save_config`/`set_logs_dir`), connection
//! validation (`validate_connection`), and run control (`start_run`/`stop_run`
//! /`get_run_status`). During a run the backend streams events
//! (`battery:level`, `reasoning:delta`, `command:detected`, `tool:call`,
//! `ascii:frame`, `session:start`/`session:end`) to the frontend.
//!
//! All platform-independent logic lives in the `tanato-core` crate so it can
//! be unit-tested without a GUI toolchain; this crate only wires it to Tauri.

pub mod commands;
pub mod session;

use std::sync::Mutex;

use tanato_core::config::Settings;

/// Global state shared with the Tauri commands.
pub struct AppState {
    /// The settings edited in the pre-run screen (persisted to disk).
    pub settings: Mutex<Settings>,
    /// The currently running session, if any (only one at a time).
    pub session: Mutex<Option<session::SessionHandle>>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            settings: Mutex::new(Settings::default()),
            session: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::save_config,
            commands::validate_connection,
            commands::start_run,
            commands::stop_run,
            commands::get_run_status,
            commands::set_logs_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
