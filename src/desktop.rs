//! Hyprland IPC adapter. Socket reads and individual messages are bounded.
use crate::recording::Window;
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_BATCH: usize = 256 * 1024;
const MAX_LINE: usize = 64 * 1024;

pub struct Hyprland {
    events: UnixStream,
    directory: PathBuf,
    buffer: Vec<u8>,
    current: Option<Window>,
    pending: Option<(String, String)>,
}

impl Hyprland {
    pub fn connect(directory: &Path) -> Result<Self> {
        // Subscribe before taking the initial snapshot, so changes during startup
        // remain visible. The snapshot itself is only published after queued events.
        let events = UnixStream::connect(directory.join(".socket2.sock"))?;
        events.set_nonblocking(true)?;
        let current = snapshot(directory)?;
        Ok(Self {
            events,
            directory: directory.into(),
            buffer: Vec::new(),
            current,
            pending: None,
        })
    }

    pub fn current(&self) -> Option<Window> {
        self.current.clone()
    }

    /// Drain received focus changes in their wire order. Overflow is a coverage
    /// failure, never silent event coalescing.
    pub fn drain(&mut self) -> Result<Vec<Option<Window>>> {
        let mut chunk = [0; 8192];
        loop {
            match self.events.read(&mut chunk) {
                Ok(0) => bail!("Hyprland event socket disconnected"),
                Ok(n) => {
                    ensure!(
                        self.buffer.len() + n <= MAX_BATCH,
                        "Hyprland event backlog exceeded limit"
                    );
                    self.buffer.extend_from_slice(&chunk[..n]);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
        }
        let mut changes = Vec::new();
        let mut consumed = 0;
        while let Some(length) = self.buffer[consumed..].iter().position(|b| *b == b'\n') {
            ensure!(length <= MAX_LINE, "Hyprland event exceeded message limit");
            let line = std::str::from_utf8(&self.buffer[consumed..consumed + length])?;
            consumed += length + 1;
            let Some((event, data)) = line.split_once(">>") else {
                bail!("malformed Hyprland event")
            };
            match event {
                "activewindow" => {
                    let (application, title) = data
                        .split_once(',')
                        .context("malformed active window event")?;
                    self.pending = Some((application.into(), title.into()));
                }
                "activewindowv2" => {
                    if data.is_empty() {
                        self.current = None;
                        self.pending = None;
                    } else {
                        let (application, title) = self
                            .pending
                            .take()
                            .context("active window identity arrived without its details")?;
                        self.current = Some(Window {
                            address: address(data),
                            application,
                            title,
                        });
                    }
                    changes.push(self.current.clone());
                }
                "windowtitlev2" => {
                    let (window_address, title) = data
                        .split_once(',')
                        .context("malformed window title event")?;
                    if let Some(current) = &mut self.current
                        && current.address == address(window_address)
                    {
                        current.title = title.into();
                        changes.push(self.current.clone());
                    }
                }
                _ => (),
            }
            ensure!(
                changes.len() <= 500,
                "too many focus observations in one batch"
            );
        }
        self.buffer.drain(..consumed);
        ensure!(
            self.buffer.len() <= MAX_LINE,
            "Hyprland partial event exceeded limit"
        );
        Ok(changes)
    }

    /// Reestablish focus after a recording boundary. Old events are discarded,
    /// and the caller must recheck lock/pause state before publishing the result.
    pub fn refresh(&mut self) -> Result<Option<Window>> {
        self.drain()?;
        self.current = snapshot(&self.directory)?;
        // Events concurrent with the snapshot are drained by the next poll.
        Ok(self.current())
    }
}

fn address(value: &str) -> String {
    value.trim_start_matches("0x").to_ascii_lowercase()
}

fn snapshot(directory: &Path) -> Result<Option<Window>> {
    let mut socket = UnixStream::connect(directory.join(".socket.sock"))?;
    socket.set_read_timeout(Some(Duration::from_millis(250)))?;
    socket.set_write_timeout(Some(Duration::from_millis(250)))?;
    socket.write_all(b"j/activewindow")?;
    let mut bytes = Vec::new();
    socket.take((MAX_LINE + 1) as u64).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_LINE,
        "active window response exceeded limit"
    );
    let value: Value = serde_json::from_slice(&bytes)?;
    let object = value
        .as_object()
        .context("invalid active window response")?;
    if object.is_empty() {
        return Ok(None);
    }
    let addr = value["address"]
        .as_str()
        .context("window address missing")?;
    if addr.is_empty() || addr == "0x0" {
        return Ok(None);
    }
    Ok(Some(Window {
        address: address(addr),
        application: value["class"]
            .as_str()
            .context("window class missing")?
            .into(),
        title: value["title"]
            .as_str()
            .context("window title missing")?
            .into(),
    }))
}
