//! Bounded event pumping shared by the recorder's Wayland connections. Each
//! connection keeps its own protocol state and limits.
use anyhow::{Result, ensure};
use std::{
    io::ErrorKind,
    os::fd::{AsFd, AsRawFd},
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, Dispatch, EventQueue,
    backend::{ReadEventsGuard, WaylandError},
    protocol::{wl_callback, wl_registry},
};

const SYNC_TIMEOUT: Duration = Duration::from_millis(500);
const SYNC_POLL_MS: i32 = 25;

pub(crate) trait Protocol: Sized + 'static {
    /// Set by the sync callback; cleared before each round trip.
    fn synced(&mut self) -> &mut bool;
    /// Connection-specific invariants checked after every dispatch.
    fn check(&self) -> Result<()>;
}

/// Connect to the session compositor and request its globals.
pub(crate) fn connect<S>() -> Result<(Connection, EventQueue<S>)>
where
    S: Dispatch<wl_registry::WlRegistry, ()> + 'static,
{
    let connection = Connection::connect_to_env()?;
    let queue = connection.new_event_queue();
    connection.display().get_registry(&queue.handle(), ());
    Ok((connection, queue))
}

/// Dispatch queued events, waiting at most `timeout_ms` for socket input.
pub(crate) fn pump<S: Protocol>(
    connection: &Connection,
    queue: &mut EventQueue<S>,
    state: &mut S,
    timeout_ms: i32,
) -> Result<()> {
    queue.dispatch_pending(state)?;
    connection.flush()?;
    if let Some(guard) = queue.prepare_read()
        && readable(connection, timeout_ms)?
    {
        read(guard)?;
    }
    queue.dispatch_pending(state)?;
    state.check()
}

/// A round trip bounded by `SYNC_TIMEOUT`; `label` names the connection.
pub(crate) fn synchronize<S>(
    connection: &Connection,
    queue: &mut EventQueue<S>,
    state: &mut S,
    label: &str,
) -> Result<()>
where
    S: Protocol + Dispatch<wl_callback::WlCallback, ()>,
{
    *state.synced() = false;
    connection.display().sync(&queue.handle(), ());
    let deadline = Instant::now() + SYNC_TIMEOUT;
    while !*state.synced() {
        ensure!(
            Instant::now() < deadline,
            "{label} synchronization timed out"
        );
        pump(connection, queue, state, SYNC_POLL_MS)?;
    }
    Ok(())
}

/// An interrupted poll is treated as a timeout.
fn readable(connection: &Connection, timeout_ms: i32) -> Result<bool> {
    let mut fd = libc::pollfd {
        fd: connection.as_fd().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    match unsafe { libc::poll(&mut fd, 1, timeout_ms) } {
        count if count > 0 => Ok(true),
        0 => Ok(false),
        _ => {
            let error = std::io::Error::last_os_error();
            if error.kind() != ErrorKind::Interrupted {
                return Err(error.into());
            }
            Ok(false)
        }
    }
}

fn read(guard: ReadEventsGuard) -> Result<()> {
    match guard.read() {
        Ok(_) => Ok(()),
        Err(WaylandError::Io(error)) if error.kind() == ErrorKind::WouldBlock => Ok(()),
        Err(error) => Err(error.into()),
    }
}
