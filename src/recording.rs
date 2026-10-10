//! Recording policy, independent of the compositor, wall clock and UI.
use crate::config::{CHECKPOINT_INTERVAL_MS, CLEANUP_INTERVAL_MS};
use crate::evidence::{BoundaryKind, Evidence, StateChange};
use crate::history::History;
use anyhow::Result;
use serde::{Deserialize, Serialize};
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

pub type Status = crate::evidence::FocusState;

pub struct Recorder {
    pub(crate) history: History,
    pub(crate) capture: crate::capture::CaptureState,
    pub(crate) status: Status,
    pub(crate) segment: i64,
    last_commit_ms: i64,
    last_stamp: Stamp,
    last_cleanup_ms: i64,
    boot_id: String,
}

impl Recorder {
    pub fn open(directory: &Path, stamp: Stamp) -> Result<Self> {
        Self::start(History::open(directory)?, stamp)
    }

    /// Begin a recording run on an opened writer: retention cleanup and the
    /// startup boundary are the run's first history changes.
    pub fn start(mut history: History, stamp: Stamp) -> Result<Self> {
        let previous = history.latest()?;
        let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
        let retention_now = history.advance_retention_clock(&boot_id, stamp.boot_ms, stamp.unix_ms);
        history.expire(retention_now)?;
        let manual_paused = history.manual_paused()?;
        let pressure = history.screenshot_pressure()?;
        let segment = history.start_segment()?;
        history.commit_evidence(
            stamp.unix_ms,
            stamp.monotonic_ms,
            segment,
            &Evidence::Boundary {
                kind: BoundaryKind::Boundary,
                reason: if previous.is_some() {
                    "restart"
                } else {
                    "startup"
                }
                .into(),
                previous_durable_observation: previous.as_ref().map(|o| o.id),
                previous_durable_unix_ms: previous.as_ref().map(|o| o.unix_ms),
            },
            None,
        )?;
        Ok(Self {
            history,
            capture: crate::capture::CaptureState {
                pressure,
                ..Default::default()
            },
            segment,
            last_commit_ms: stamp.monotonic_ms,
            last_stamp: stamp,
            last_cleanup_ms: stamp.boot_ms,
            boot_id,
            status: Status {
                manual_paused,
                connected: false,
                locked: true,
                idle: false,
                focus: None,
                focus_known: false,
                screenshot_recording: false,
                screenshot_pressure: pressure,
                screenshot_cleanup: false,
            },
        })
    }

    pub fn status(&self) -> &Status {
        &self.status
    }
    pub fn history(&self) -> &History {
        &self.history
    }

    /// Runtime opt-in: image reconciliation runs independently of desktop events.
    pub fn enable_background_cleanup(&mut self) -> Result<()> {
        self.history.enable_background_cleanup()
    }

    /// The probe is an external filesystem seam, allowing deterministic pressure tests.
    pub fn check_storage_pressure(
        &mut self,
        stamp: Stamp,
        mut available: impl FnMut() -> Result<u64>,
    ) -> Result<()> {
        use crate::config::{PRESSURE_RESUME_BYTES, PRESSURE_STOP_BYTES};
        self.apply(Input::Tick, stamp)?;
        let mut bytes = available()?;
        if self.capture.pressure_cleanup {
            if self.history.cleanup_pending()? {
                return Ok(());
            }
            self.capture.pressure_cleanup = false;
        } else if bytes < PRESSURE_STOP_BYTES && !self.capture.pressure {
            let retention_now =
                self.history
                    .advance_retention_clock(&self.boot_id, stamp.boot_ms, stamp.unix_ms);
            self.history.expire(retention_now)?;
            if self.history.cleanup_pending()? {
                self.capture.pressure_cleanup = true;
                self.invalidate_captures("disk_cleanup", stamp)?;
                self.update_capture_status();
                return Ok(());
            }
            bytes = available()?;
        }
        let pressure = if self.capture.pressure {
            bytes <= PRESSURE_RESUME_BYTES
        } else {
            bytes < PRESSURE_STOP_BYTES
        };
        if pressure != self.capture.pressure {
            self.invalidate_captures(
                if pressure {
                    "disk_pressure"
                } else {
                    "disk_recovered"
                },
                stamp,
            )?;
            self.history.set_screenshot_pressure(pressure)?;
            self.capture.pressure = pressure;
        }
        self.update_capture_status();
        Ok(())
    }

    pub fn apply(&mut self, input: Input, stamp: Stamp) -> Result<()> {
        self.history.cleanup_pending()?;
        let retention_now =
            self.history
                .advance_retention_clock(&self.boot_id, stamp.boot_ms, stamp.unix_ms);
        if stamp.boot_ms - self.last_cleanup_ms >= CLEANUP_INTERVAL_MS {
            self.history.expire(retention_now)?;
            self.last_cleanup_ms = stamp.boot_ms;
        }
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
            self.invalidate_captures(reason, stamp)?;
            let previous = self.history.latest()?;
            self.segment = self.history.start_segment()?;
            self.status.focus = None;
            self.status.focus_known = false;
            self.history.commit_evidence(
                stamp.unix_ms,
                stamp.monotonic_ms,
                self.segment,
                &Evidence::Boundary {
                    kind: BoundaryKind::Boundary,
                    reason: reason.into(),
                    previous_durable_observation: previous.as_ref().map(|o| o.id),
                    previous_durable_unix_ms: previous.as_ref().map(|o| o.unix_ms),
                },
                None,
            )?;
            self.last_commit_ms = stamp.monotonic_ms;
        }
        self.last_stamp = stamp;
        // A focus result already in flight cannot establish fresh coverage after
        // a timing boundary. The runtime must obtain a new snapshot instead.
        if discontinuity.is_some() && matches!(input, Input::Focus(_)) {
            return Ok(());
        }
        let boundary = match &input {
            Input::Connected { .. } => Some("connected"),
            Input::Pause => Some("pause"),
            Input::Resume => Some("resume"),
            Input::Locked(true) => Some("lock"),
            Input::Locked(false) => Some("unlock"),
            Input::Disconnected => Some("disconnect"),
            _ => None,
        };
        if let Some(reason) = boundary {
            self.invalidate_captures(reason, stamp)?;
        }
        let mut next = self.status.clone();
        let (kind, paused) = match input {
            Input::Connected { locked, idle } => {
                next.connected = true;
                next.locked = locked;
                next.idle = idle;
                next.focus = None;
                next.focus_known = false;
                (StateChange::Connected, None)
            }
            Input::Focus(focus) => {
                if next.manual_paused || next.locked || !next.connected {
                    return Ok(());
                }
                next.focus = focus;
                next.focus_known = true;
                (StateChange::Focus, None)
            }
            Input::Pause | Input::Resume => {
                let paused = matches!(input, Input::Pause);
                next.manual_paused = paused;
                next.focus = None;
                next.focus_known = false;
                (
                    if paused {
                        StateChange::Pause
                    } else {
                        StateChange::Resume
                    },
                    Some(paused),
                )
            }
            Input::Locked(locked) => {
                next.locked = locked;
                next.focus = None;
                next.focus_known = false;
                (
                    if locked {
                        StateChange::Lock
                    } else {
                        StateChange::Unlock
                    },
                    None,
                )
            }
            Input::Idle(idle) => {
                next.idle = idle;
                if next.manual_paused || next.locked || !next.connected {
                    self.status = next;
                    return Ok(());
                }
                (StateChange::Idle, None)
            }
            Input::Disconnected => {
                next.connected = false;
                next.locked = true;
                next.focus = None;
                next.focus_known = false;
                (StateChange::Disconnect, None)
            }
            Input::Tick => {
                if !next.focus_known
                    || stamp.monotonic_ms - self.last_commit_ms < CHECKPOINT_INTERVAL_MS
                {
                    return Ok(());
                }
                (StateChange::Checkpoint, None)
            }
        };
        self.history.commit_evidence(
            stamp.unix_ms,
            stamp.monotonic_ms,
            self.segment,
            &Evidence::State {
                kind,
                state: next.clone(),
            },
            paused,
        )?;
        self.status = next;
        self.update_capture_status();
        self.last_commit_ms = stamp.monotonic_ms;
        Ok(())
    }
}
