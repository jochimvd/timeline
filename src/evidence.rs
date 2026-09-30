//! Typed persisted evidence. The tagged wire representation also decodes v1–v3
//! records; optional screenshot status fields were introduced in the second slice.
use crate::recording::Window;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocusState {
    pub manual_paused: bool,
    pub connected: bool,
    pub locked: bool,
    pub idle: bool,
    pub focus: Option<Window>,
    pub focus_known: bool,
    #[serde(default)]
    pub screenshot_recording: bool,
    #[serde(default)]
    pub screenshot_pressure: bool,
    #[serde(default)]
    pub screenshot_cleanup: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StateChange {
    Connected,
    Focus,
    Pause,
    Resume,
    Lock,
    Unlock,
    Idle,
    Disconnect,
    Checkpoint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Evidence {
    Boundary {
        kind: BoundaryKind,
        reason: String,
        previous_durable_observation: Option<i64>,
        previous_durable_unix_ms: Option<i64>,
    },
    State {
        kind: StateChange,
        state: FocusState,
    },
    /// Preserve unknown legacy/future records verbatim; they end known coverage.
    Unknown(serde_json::Value),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BoundaryKind {
    #[serde(rename = "boundary")]
    Boundary,
}

impl crate::history::History {
    pub fn commit_evidence(
        &mut self,
        unix_ms: i64,
        monotonic_ms: i64,
        segment: i64,
        evidence: &Evidence,
        paused: Option<bool>,
    ) -> anyhow::Result<i64> {
        self.commit(
            unix_ms,
            monotonic_ms,
            segment,
            serde_json::to_value(evidence)?,
            paused,
        )
    }
}
