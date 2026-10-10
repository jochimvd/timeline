//! Desktop source supervision: session notifications plus Hyprland focus.
//! Source failures become a recording gap and a reconnect; storage failures
//! stay terminal.
use super::{SourceResult, gap_on_source_failure, now};
use crate::{
    desktop::Hyprland,
    recording::{Input, Recorder, Window},
    session::Session,
};
use anyhow::{Context, Result, ensure};
use std::{
    env,
    path::PathBuf,
    time::{Duration, Instant},
};

const RETRY: Duration = Duration::from_secs(2);

pub(super) struct Desktop {
    connection: Option<(Session, Hyprland)>,
    retry_at: Instant,
    pub error: Option<String>,
}

impl Desktop {
    pub fn new() -> Self {
        Self {
            connection: None,
            retry_at: Instant::now(),
            error: None,
        }
    }

    pub fn session(&mut self) -> Option<&mut Session> {
        self.connection.as_mut().map(|(session, _)| session)
    }

    pub fn step(&mut self, recorder: &mut Recorder) -> Result<()> {
        if self.connection.is_none() && Instant::now() >= self.retry_at {
            self.reconnect(recorder)?;
        }
        let Some((session, hyprland)) = self.connection.as_mut() else {
            return recorder.apply(Input::Tick, now()?);
        };
        let observed = observe(recorder, session, hyprland);
        if let Some(message) = gap_on_source_failure(observed)? {
            self.disconnect(recorder, message)?;
        }
        Ok(())
    }

    /// End coverage after a source failure; reconnection follows the retry delay.
    pub fn disconnect(&mut self, recorder: &mut Recorder, message: String) -> Result<()> {
        recorder.apply(Input::Disconnected, now()?)?;
        eprintln!("Desktop disconnected; recording gap started: {message}");
        self.connection = None;
        self.error = Some(message);
        self.retry_at = Instant::now() + RETRY;
        Ok(())
    }

    fn reconnect(&mut self, recorder: &mut Recorder) -> Result<()> {
        match connect() {
            Ok((session, hyprland)) => {
                recorder.apply(
                    Input::Connected {
                        locked: session.locked(),
                        idle: session.idle(),
                    },
                    now()?,
                )?;
                self.connection = Some((session, hyprland));
                self.error = None;
            }
            Err(error) => {
                let message = format!("{error:#}");
                if self.error.as_ref() != Some(&message) {
                    eprintln!("Desktop unavailable: {message}");
                }
                self.error = Some(message);
                self.retry_at = Instant::now() + RETRY;
            }
        }
        Ok(())
    }
}

/// Apply pending session notifications, then publish focus while unpaused and
/// unlocked.
fn observe(recorder: &mut Recorder, session: &mut Session, hyprland: &mut Hyprland) -> Result<()> {
    let events = session.events().source_failure()?;
    let changes = hyprland.drain().source_failure()?;
    for input in events {
        recorder.apply(input, now()?)?;
    }
    recorder.apply(Input::Tick, now()?)?;
    let status = recorder.status();
    if status.manual_paused || status.locked {
        Ok(())
    } else if !status.focus_known {
        refocus(recorder, session, hyprland)
    } else if !changes.is_empty() {
        // Establish the current compositor lock state after draining IPC, then
        // check it between every commit.
        session.synchronize().source_failure()?;
        publish_focus_changes(recorder, changes, || session.events().source_failure())
    } else {
        Ok(())
    }
}

/// Reestablish unknown focus from a fresh snapshot.
fn refocus(recorder: &mut Recorder, session: &mut Session, hyprland: &mut Hyprland) -> Result<()> {
    let focus = hyprland.refresh().source_failure()?;
    session.synchronize().source_failure()?;
    let events = session.events().source_failure()?;
    // Do not publish a snapshot taken across even a lock/unlock cycle that has
    // already ended.
    let crossed_boundary = events.iter().any(|e| matches!(e, Input::Locked(_)));
    for input in events {
        recorder.apply(input, now()?)?;
    }
    if crossed_boundary {
        return Ok(());
    }
    publish_focus_changes(recorder, vec![focus], || session.events().source_failure())
}

/// Publish a received focus batch while giving session boundaries priority.
/// The callback is the external lock/idle notification source; callers establish
/// a compositor synchronization barrier before entering this publication loop.
/// Errors propagate unchanged and end publication: callers mark source failures
/// (`SourceError`) to end coverage with a gap, while storage failures are
/// terminal, leaving uncertain time after the last durable observation for
/// restart recovery.
pub fn publish_focus_changes(
    recorder: &mut Recorder,
    changes: Vec<Option<Window>>,
    mut session_events: impl FnMut() -> Result<Vec<Input>>,
) -> Result<()> {
    for focus in changes {
        let events = session_events()?;
        let crossed_boundary = events.iter().any(|event| matches!(event, Input::Locked(_)));
        for input in events {
            recorder.apply(input, now()?)?;
        }
        if crossed_boundary || recorder.status().locked || recorder.status().manual_paused {
            break;
        }
        recorder.apply(Input::Focus(focus), now()?)?;
        if !recorder.status().focus_known {
            // A suspend/clock/stall boundary may have been detected while this
            // batch was in flight. Discard the remaining pre-boundary results.
            break;
        }
    }
    Ok(())
}

fn connect() -> Result<(Session, Hyprland)> {
    let runtime =
        PathBuf::from(env::var_os("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR is not set")?);
    let signature = env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .context("HYPRLAND_INSTANCE_SIGNATURE is not set")?;
    ensure!(
        !signature.is_empty() && !signature.contains('/'),
        "invalid Hyprland instance signature"
    );
    let mut session = Session::connect()?;
    let mut hyprland = Hyprland::connect(&runtime.join("hypr").join(signature))?;
    hyprland.drain()?;
    session.synchronize()?;
    session.events()?;
    Ok((session, hyprland))
}
