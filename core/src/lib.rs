//! tanato-core — platform-independent engine for TanatoGothci.
//!
//! This crate holds all the non-GUI logic so it can be unit-tested without a
//! Tauri / webview toolchain:
//!
//! * `config`  — settings schema + validation;
//! * `prompt`  — the system prompt (the "lie");
//! * `events`  — event kinds shared with the frontend;
//! * `battery` — the one-hour, 16-ping battery countdown engine;
//! * `tools`   — tool-call parsing and escape (command) detection;
//! * `api`     — OpenAI-compatible client (validation ping + SSE streaming);
//! * `artifacts` — per-run artifact logging (JSONL / Markdown / ASCII frames);
//! * `geometry` — measured ASCII panel limits (system prompt + tool contract).

pub mod api;
pub mod artifacts;
pub mod battery;
pub mod config;
pub mod events;
pub mod geometry;
pub mod prompt;
pub mod tools;
