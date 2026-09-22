use crate::{
    desktop::Hyprland,
    recording::{Input, Recorder, Stamp, Window},
    session::Session,
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::json;
use std::{
    env, fs,
    io::{Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

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

pub fn request(directory: &Path, command: &str) -> Result<String> {
    let mut stream = UnixStream::connect(directory.join("control.sock"))
        .context("recorder is not reachable; start `timeline run`")?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    writeln!(stream, "{command}")?;
    let mut response = String::new();
    stream.take(1_048_577).read_to_string(&mut response)?;
    ensure!(
        response.len() <= 1_048_576,
        "control response exceeded limit"
    );
    let value: serde_json::Value = serde_json::from_str(&response)?;
    if let Some(error) = value.get("error") {
        bail!("control request failed: {error}");
    }
    Ok(response)
}

pub fn run(directory: &Path) -> Result<()> {
    // The writer lock is acquired before reclaiming a stale control socket.
    let mut recorder = Recorder::open(directory, now()?)?;
    recorder.enable_background_cleanup()?;
    let socket_path = directory.join("control.sock");
    match fs::remove_file(&socket_path) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    let listener = UnixListener::bind(&socket_path)?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let _socket_cleanup = SocketCleanup(socket_path);
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(&stop))?;
    }
    let mut screenshots = crate::screenshots::Screenshots::new(directory)?;
    let mut source: Option<(Session, Hyprland)> = None;
    let mut retry_at = Instant::now();
    let mut desktop_error: Option<String> = None;
    while !stop.load(Ordering::Relaxed) {
        if source.is_none() && Instant::now() >= retry_at {
            match connect() {
                Ok((session, hyprland)) => {
                    recorder.apply(
                        Input::Connected {
                            locked: session.locked(),
                            idle: session.idle(),
                        },
                        now()?,
                    )?;
                    source = Some((session, hyprland));
                    desktop_error = None;
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    if desktop_error.as_ref() != Some(&message) {
                        eprintln!("Desktop unavailable: {message}");
                    }
                    desktop_error = Some(message);
                    retry_at = Instant::now() + Duration::from_secs(2);
                }
            }
        }
        if let Some((session, hyprland)) = source.as_mut() {
            // Source errors end coverage. Storage failures propagate and stop the
            // recorder rather than pretending that evidence was committed.
            let events = session
                .events()
                .and_then(|events| Ok((events, hyprland.drain()?)));
            match events {
                Ok((events, changes)) => {
                    for input in events {
                        recorder.apply(input, now()?)?;
                    }
                    recorder.apply(Input::Tick, now()?)?;
                    let status = recorder.status();
                    if !status.manual_paused && !status.locked {
                        if !status.focus_known {
                            match hyprland.refresh().and_then(|focus| {
                                session.synchronize()?;
                                Ok((focus, session.events()?))
                            }) {
                                Ok((focus, events)) => {
                                    // Do not publish a snapshot taken across even
                                    // a lock/unlock cycle that has already ended.
                                    let crossed_boundary =
                                        events.iter().any(|e| matches!(e, Input::Locked(_)));
                                    for input in events {
                                        recorder.apply(input, now()?)?;
                                    }
                                    if !crossed_boundary {
                                        publish_focus_changes(&mut recorder, vec![focus], || {
                                            session.events()
                                        })?;
                                    }
                                }
                                Err(error) => desktop_error = Some(format!("{error:#}")),
                            }
                        } else if !changes.is_empty() {
                            // Establish the current compositor lock state after
                            // draining IPC, then check it between every commit.
                            session.synchronize()?;
                            publish_focus_changes(&mut recorder, changes, || session.events())?;
                        }
                    }
                }
                Err(error) => desktop_error = Some(format!("{error:#}")),
            }
            if desktop_error.is_some() {
                recorder.apply(Input::Disconnected, now()?)?;
                source = None;
                retry_at = Instant::now() + Duration::from_secs(2);
                eprintln!("Desktop disconnected; recording gap started");
            }
        } else {
            recorder.apply(Input::Tick, now()?)?;
        }
        screenshots.step(&mut recorder, source.as_mut().map(|(session, _)| session))?;
        for _ in 0..4 {
            let (mut stream, _) = match listener.accept() {
                Ok(client) => client,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error.into()),
            };
            stream.set_read_timeout(Some(Duration::from_millis(50)))?;
            stream.set_write_timeout(Some(Duration::from_millis(50)))?;
            let command = read_command(&mut stream);
            let response = match command.as_deref() {
                Ok("pause") => {
                    recorder.apply(Input::Pause, now()?)?;
                    json!({"ok":true})
                }
                Ok("resume") => {
                    recorder.apply(Input::Resume, now()?)?;
                    json!({"ok":true})
                }
                Ok("status") => {
                    json!({"recording":recorder.status(), "desktop_error":desktop_error, "screenshot_error":screenshots.error})
                }
                _ => json!({"error":"expected status, pause, or resume followed by a newline"}),
            };
            // A disappearing control client cannot stop the recorder.
            let _ = writeln!(stream, "{response}");
        }
        thread::sleep(Duration::from_millis(50));
    }
    recorder.apply(Input::Disconnected, now()?)?;
    Ok(())
}

/// Publish a received focus batch while giving session boundaries priority.
/// The callback is the external lock/idle notification source; callers establish
/// a compositor synchronization barrier before entering this publication loop.
/// Any error is terminal for the current recording run, leaving uncertain time
/// after its last durable observation for restart recovery.
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

fn read_command(stream: &mut UnixStream) -> Result<String> {
    let mut bytes = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(50);
    for _ in 0..16 {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .context("control request timed out")?;
        stream.set_read_timeout(Some(remaining.max(Duration::from_millis(1))))?;
        let mut byte = [0];
        stream.read_exact(&mut byte)?;
        if byte[0] == b'\n' {
            return Ok(String::from_utf8(bytes)?);
        }
        bytes.push(byte[0]);
    }
    bail!("control command too long")
}

struct SocketCleanup(PathBuf);
impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
