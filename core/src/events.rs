//! Event payload kinds shared with the frontend (R-007/R-009).

use serde::{Deserialize, Serialize};

/// Classifies a reasoning-log line. `Command` marks an escape attempt (shown
/// labelled `COMMAND` and colour-highlighted in the UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineKind {
    Reasoning,
    Command,
}
