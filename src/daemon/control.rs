//! Private control socket: one newline-terminated command, one JSON response.
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::{
    fs,
    io::{Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const RESPONSE_LIMIT: usize = 1_048_576;

pub fn request(directory: &Path, command: &str) -> Result<String> {
    let mut stream = UnixStream::connect(socket_path(directory))
        .context("recorder is not reachable; start `timeline run`")?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    writeln!(stream, "{command}")?;
    let mut response = String::new();
    stream
        .take(RESPONSE_LIMIT as u64 + 1)
        .read_to_string(&mut response)?;
    ensure!(
        response.len() <= RESPONSE_LIMIT,
        "control response exceeded limit"
    );
    let value: Value = serde_json::from_str(&response)?;
    if let Some(error) = value.get("error") {
        bail!("control request failed: {error}");
    }
    Ok(response)
}

pub(super) struct Control {
    listener: UnixListener,
    path: PathBuf,
}

impl Control {
    /// The caller must hold the writer lock: a live recorder's socket is never
    /// reclaimed, only a stale one.
    pub fn bind(directory: &Path) -> Result<Self> {
        let path = socket_path(directory);
        match fs::remove_file(&path) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        let listener = UnixListener::bind(&path)
            .with_context(|| format!("cannot bind control socket {}", path.display()))?;
        let control = Self { listener, path };
        fs::set_permissions(&control.path, fs::Permissions::from_mode(0o600))?;
        control.listener.set_nonblocking(true)?;
        Ok(control)
    }

    /// Answer a bounded number of waiting clients without blocking recording.
    /// `respond` receives `None` for a malformed command.
    pub fn serve(&self, mut respond: impl FnMut(Option<&str>) -> Result<Value>) -> Result<()> {
        for _ in 0..4 {
            let mut stream = match self.listener.accept() {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error.into()),
            };
            let command = read_command(&mut stream);
            let response = respond(command.as_deref().ok())?;
            // A disappearing control client cannot stop the recorder.
            let _ = stream
                .set_write_timeout(Some(Duration::from_millis(50)))
                .and_then(|()| writeln!(stream, "{response}"));
        }
        Ok(())
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn socket_path(directory: &Path) -> PathBuf {
    directory.join("control.sock")
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
