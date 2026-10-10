//! Recorder process wiring. Source and endpoint failures degrade coverage or
//! access; persistence failures stop the run rather than acknowledging unsaved
//! evidence.
mod control;
mod endpoint;
mod source;

use crate::{
    history::History,
    recording::{Input, Recorder, Stamp},
    screenshots::Screenshots,
    session::Session,
};
use anyhow::Result;
use serde_json::{Value, json};
use std::{
    fmt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub use control::request;
pub use source::publish_focus_changes;

pub fn now() -> Result<Stamp> {
    fn clock(id: libc::clockid_t) -> Result<i64> {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        if unsafe { libc::clock_gettime(id, &mut time) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(time.tv_sec * 1000 + time.tv_nsec / 1_000_000)
    }
    Ok(Stamp {
        unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_millis()
            .try_into()?,
        monotonic_ms: clock(libc::CLOCK_MONOTONIC)?,
        boot_ms: clock(libc::CLOCK_BOOTTIME)?,
    })
}

pub fn run(directory: &Path) -> Result<()> {
    // Take the writer lock, then the control socket and endpoint configuration,
    // before the startup boundary: a failed start leaves history unchanged.
    let history = History::open(directory)?;
    let control = control::Control::bind(directory)?;
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(&stop))?;
    }
    let mut endpoint =
        endpoint::Endpoint::new(directory, &crate::config::http_address()?, stop.clone())?;
    endpoint.step();
    let mut recorder = Recorder::start(history, now()?)?;
    recorder.enable_background_cleanup()?;
    let mut desktop = source::Desktop::new();
    let mut screenshots = Screenshots::new(directory)?;
    while !stop.load(Ordering::Relaxed) {
        desktop.step(&mut recorder)?;
        let captured = screenshots.step(&mut recorder, desktop.session());
        if let Some(message) = gap_on_source_failure(captured)? {
            desktop.disconnect(&mut recorder, message)?;
        }
        endpoint.step();
        control.serve(|command| match command {
            Some("launch") => Ok(match endpoint.launch_url() {
                Ok(url) => json!({"url":url}),
                Err(error) => json!({"error":format!("browser launch unavailable: {error:#}")}),
            }),
            Some("pause") => acknowledge(&mut recorder, Input::Pause),
            Some("resume") => acknowledge(&mut recorder, Input::Resume),
            Some("status") => Ok(json!({
                "recording": recorder.status(),
                "desktop_error": desktop.error,
                "screenshot_error": screenshots.error,
                "web_error": endpoint.error,
            })),
            _ => Ok(json!({"error":"expected status, pause, or resume followed by a newline"})),
        })?;
        thread::sleep(Duration::from_millis(50));
    }
    recorder.apply(Input::Disconnected, now()?)?;
    endpoint.shutdown();
    Ok(())
}

/// Success is reported only after the command's state is committed.
fn acknowledge(recorder: &mut Recorder, input: Input) -> Result<Value> {
    recorder.apply(input, now()?)?;
    Ok(json!({"ok":true}))
}

/// Apply pending session notifications; a session failure ends coverage.
pub(crate) fn apply_session_events(recorder: &mut Recorder, session: &mut Session) -> Result<()> {
    for input in session.events().source_failure()? {
        recorder.apply(input, now()?)?;
    }
    Ok(())
}

/// A desktop source failure: it ends coverage with a recording gap followed by
/// a reconnect. Unmarked errors (storage, clock) remain terminal.
#[derive(Debug)]
pub(crate) struct SourceError(anyhow::Error);

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#}", self.0)
    }
}

impl std::error::Error for SourceError {}

pub(crate) trait SourceResult<T> {
    /// Mark an error from the desktop source as a `SourceError`.
    fn source_failure(self) -> Result<T>;
}

impl<T> SourceResult<T> for Result<T> {
    fn source_failure(self) -> Result<T> {
        self.map_err(|error| SourceError(error).into())
    }
}

/// Separate a source failure, returned as its message, from terminal errors.
pub(crate) fn gap_on_source_failure(result: Result<()>) -> Result<Option<String>> {
    match result {
        Err(error) if error.is::<SourceError>() => Ok(Some(format!("{error:#}"))),
        result => result.map(|()| None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context, anyhow};

    #[test]
    fn only_marked_source_failures_become_gaps() {
        let source: Result<()> = Err(anyhow!("Wayland synchronization timed out"));
        assert_eq!(
            gap_on_source_failure(source.source_failure().context("publishing focus")).unwrap(),
            Some("publishing focus: Wayland synchronization timed out".into())
        );
        let storage: Result<()> = Err(anyhow!("database is locked"));
        assert!(gap_on_source_failure(storage).is_err());
        assert_eq!(gap_on_source_failure(Ok(())).unwrap(), None);
    }
}
