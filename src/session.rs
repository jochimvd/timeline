//! Compositor lock and input-idle notifications. Recording stays closed until
//! the initial protocol synchronization has established the lock state.
use crate::recording::Input;
use anyhow::{Context, Result, bail, ensure};
use std::{
    os::fd::{AsFd, AsRawFd},
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle,
    protocol::{wl_callback, wl_registry, wl_seat},
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1 as idle_notification, ext_idle_notifier_v1 as idle_manager,
};

mod lock_protocol {
    use wayland_client;
    pub mod __interfaces {
        wayland_scanner::generate_interfaces!("protocols/hyprland-lock-notify-v1.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("protocols/hyprland-lock-notify-v1.xml");
}
use lock_protocol::{
    hyprland_lock_notification_v1 as lock_notification, hyprland_lock_notifier_v1 as lock_manager,
};

#[derive(Default)]
struct State {
    seat: Option<wl_seat::WlSeat>,
    lock: Option<lock_manager::HyprlandLockNotifierV1>,
    idle_manager: Option<idle_manager::ExtIdleNotifierV1>,
    locked: bool,
    idle: bool,
    synced: bool,
    removed: bool,
    required_globals: Vec<u32>,
    events: Vec<Input>,
}

pub struct Session {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
}

impl Session {
    pub fn connect() -> Result<Self> {
        let connection = Connection::connect_to_env()?;
        let queue = connection.new_event_queue();
        connection.display().get_registry(&queue.handle(), ());
        let mut session = Self {
            connection,
            queue,
            state: State::default(),
        };
        session.synchronize()?;
        let handle = session.queue.handle();
        session
            .state
            .lock
            .as_ref()
            .context("Hyprland lock notifications unavailable; refusing to record")?
            .get_lock_notification(&handle, ());
        let seat = session
            .state
            .seat
            .as_ref()
            .context("Wayland seat unavailable")?;
        session
            .state
            .idle_manager
            .as_ref()
            .context("input idle notifications unavailable")?
            .get_input_idle_notification(300_000, seat, &handle, ());
        session.synchronize()?;
        session.state.events.clear();
        Ok(session)
    }

    pub fn locked(&self) -> bool {
        self.state.locked
    }
    pub fn idle(&self) -> bool {
        self.state.idle
    }

    /// A bounded synchronization barrier is used around fresh focus snapshots.
    pub fn synchronize(&mut self) -> Result<()> {
        self.state.synced = false;
        self.connection.display().sync(&self.queue.handle(), ());
        let deadline = Instant::now() + Duration::from_millis(500);
        while !self.state.synced {
            ensure!(
                Instant::now() < deadline,
                "Wayland synchronization timed out"
            );
            self.pump(25)?;
        }
        Ok(())
    }

    pub fn events(&mut self) -> Result<Vec<Input>> {
        self.pump(0)?;
        Ok(std::mem::take(&mut self.state.events))
    }

    fn pump(&mut self, timeout_ms: i32) -> Result<()> {
        self.queue.dispatch_pending(&mut self.state)?;
        self.connection.flush()?;
        if let Some(guard) = self.queue.prepare_read() {
            let mut fd = libc::pollfd {
                fd: self.connection.as_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let count = unsafe { libc::poll(&mut fd, 1, timeout_ms) };
            if count < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(error.into());
                }
            } else if count > 0 {
                match guard.read() {
                    Ok(_) => (),
                    Err(wayland_client::backend::WaylandError::Io(error))
                        if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        self.queue.dispatch_pending(&mut self.state)?;
        if self.state.removed {
            bail!("required Wayland global removed");
        }
        ensure!(
            self.state.events.len() <= 500,
            "Wayland notification backlog exceeded limit"
        );
        Ok(())
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        handle: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                match interface.as_str() {
                    "wl_seat" if state.seat.is_none() => {
                        state.seat = Some(registry.bind(name, version.min(1), handle, ()))
                    }
                    "hyprland_lock_notifier_v1" => {
                        state.lock = Some(registry.bind(name, 1, handle, ()))
                    }
                    "ext_idle_notifier_v1" if version >= 2 => {
                        state.idle_manager = Some(registry.bind(name, 2, handle, ()))
                    }
                    _ => return,
                }
                state.required_globals.push(name);
            }
            wl_registry::Event::GlobalRemove { name } if state.required_globals.contains(&name) => {
                state.removed = true
            }
            _ => (),
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.synced = true;
    }
}

impl Dispatch<lock_notification::HyprlandLockNotificationV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &lock_notification::HyprlandLockNotificationV1,
        event: lock_notification::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.locked = matches!(event, lock_notification::Event::Locked);
        state.events.push(Input::Locked(state.locked));
    }
}

impl Dispatch<idle_notification::ExtIdleNotificationV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &idle_notification::ExtIdleNotificationV1,
        event: idle_notification::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.idle = matches!(event, idle_notification::Event::Idled);
        state.events.push(Input::Idle(state.idle));
    }
}

wayland_client::delegate_noop!(State: ignore wl_seat::WlSeat);
wayland_client::delegate_noop!(State: ignore lock_manager::HyprlandLockNotifierV1);
wayland_client::delegate_noop!(State: ignore idle_manager::ExtIdleNotifierV1);
