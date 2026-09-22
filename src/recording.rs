//! Recording policy, independent of the compositor, wall clock and UI.
use crate::history::History;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Stamp {
    pub unix_ms: i64,
    pub monotonic_ms: i64,
    pub boot_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub address: String,
    pub application: String,
    pub title: String,
}

pub enum Input {
    Connected { locked: bool, idle: bool },
    Focus(Option<Window>),
    Pause,
    Resume,
    Locked(bool),
    Idle(bool),
    Disconnected,
    Tick,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub manual_paused: bool,
    pub connected: bool,
    pub locked: bool,
    pub idle: bool,
    pub focus: Option<Window>,
    pub focus_known: bool,
    pub screenshot_recording: bool,
}

pub struct Recorder {
    history: History,
    status: Status,
    segment: i64,
    last_commit_ms: i64,
    last_stamp: Stamp,
}

impl Recorder {
    pub fn open(directory: &Path, stamp: Stamp) -> Result<Self> {
        let mut history = History::open(directory)?;
        let previous = history.latest()?;
        let manual_paused = history.manual_paused()?;
        let segment = history.start_segment()?;
        history.append(stamp.unix_ms, stamp.monotonic_ms, segment, json!({
            "kind": "boundary", "reason": if previous.is_some() { "restart" } else { "startup" },
            "previous_durable_observation": previous.as_ref().map(|o| o.id),
            "previous_durable_unix_ms": previous.as_ref().map(|o| o.unix_ms),
        }))?;
        Ok(Self {
            history,
            segment,
            last_commit_ms: stamp.monotonic_ms,
            last_stamp: stamp,
            status: Status {
                manual_paused,
                connected: false,
                locked: true,
                idle: false,
                focus: None,
                focus_known: false,
                screenshot_recording: false,
            },
        })
    }

    pub fn status(&self) -> &Status {
        &self.status
    }
    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn apply(&mut self, input: Input, stamp: Stamp) -> Result<()> {
        let elapsed = stamp.monotonic_ms - self.last_stamp.monotonic_ms;
        let boot_elapsed = stamp.boot_ms - self.last_stamp.boot_ms;
        let wall_elapsed = stamp.unix_ms - self.last_stamp.unix_ms;
        let discontinuity = if elapsed < 0 || boot_elapsed < 0 {
            Some("monotonic_reset")
        } else if boot_elapsed - elapsed > 1_000 {
            Some("suspend")
        } else if (wall_elapsed - boot_elapsed).abs() > 1_000 {
            Some("clock_discontinuity")
        } else if elapsed > 10_000 {
            Some("observation_stall")
        } else {
            None
        };
        if let Some(reason) = discontinuity {
            let previous = self.history.latest()?;
            self.segment = self.history.start_segment()?;
            self.status.focus = None;
            self.status.focus_known = false;
            self.history.append(
                stamp.unix_ms,
                stamp.monotonic_ms,
                self.segment,
                json!({
                    "kind": "boundary", "reason": reason,
                    "previous_durable_observation": previous.as_ref().map(|o| o.id),
                    "previous_durable_unix_ms": previous.as_ref().map(|o| o.unix_ms),
                }),
            )?;
            self.last_commit_ms = stamp.monotonic_ms;
        }
        self.last_stamp = stamp;
        let mut next = self.status.clone();
        let (kind, paused) = match input {
            Input::Connected { locked, idle } => {
                next.connected = true;
                next.locked = locked;
                next.idle = idle;
                next.focus = None;
                next.focus_known = false;
                ("connected", None)
            }
            Input::Focus(focus) => {
                if next.manual_paused || next.locked || !next.connected {
                    return Ok(());
                }
                next.focus = focus;
                next.focus_known = true;
                ("focus", None)
            }
            Input::Pause | Input::Resume => {
                let paused = matches!(input, Input::Pause);
                next.manual_paused = paused;
                next.focus = None;
                next.focus_known = false;
                (if paused { "pause" } else { "resume" }, Some(paused))
            }
            Input::Locked(locked) => {
                next.locked = locked;
                next.focus = None;
                next.focus_known = false;
                (if locked { "lock" } else { "unlock" }, None)
            }
            Input::Idle(idle) => {
                next.idle = idle;
                if next.manual_paused || next.locked || !next.connected {
                    self.status = next;
                    return Ok(());
                }
                ("idle", None)
            }
            Input::Disconnected => {
                next.connected = false;
                next.locked = true;
                next.focus = None;
                next.focus_known = false;
                ("disconnect", None)
            }
            Input::Tick => {
                if !next.focus_known || stamp.monotonic_ms - self.last_commit_ms < 5_000 {
                    return Ok(());
                }
                ("checkpoint", None)
            }
        };
        self.history.commit(
            stamp.unix_ms,
            stamp.monotonic_ms,
            self.segment,
            json!({"kind": kind, "state": next}),
            paused,
        )?;
        self.status = next;
        self.last_commit_ms = stamp.monotonic_ms;
        Ok(())
    }
}
