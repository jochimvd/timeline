//! Persistent ext-image-copy-capture sessions, one per Wayland output. No
//! encoding, SQLite work, blocking frame waits, or capture policy lives here.
use crate::{
    capture::{CaptureRequest, Monitor, PixelBudget, Pixels},
    recording::Recorder,
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    os::fd::{AsFd, AsRawFd},
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, WEnum,
    protocol::{wl_buffer, wl_callback, wl_output, wl_registry, wl_shm, wl_shm_pool},
};
use wayland_protocols::ext::{
    image_capture_source::v1::client::{
        ext_image_capture_source_v1 as source,
        ext_output_image_capture_source_manager_v1 as sources,
    },
    image_copy_capture::v1::client::{
        ext_image_copy_capture_frame_v1 as frame, ext_image_copy_capture_manager_v1 as manager,
        ext_image_copy_capture_session_v1 as session,
    },
};

#[derive(Default)]
struct State {
    outputs: BTreeMap<u32, Output>,
    shm: Option<wl_shm::WlShm>,
    sources: Option<sources::ExtOutputImageCaptureSourceManagerV1>,
    manager: Option<manager::ExtImageCopyCaptureManagerV1>,
    globals: Vec<u32>,
    removed: bool,
    synced: bool,
    revision: u64,
}
struct Output {
    output: wl_output::WlOutput,
    name: String,
    session: Option<session::ExtImageCopyCaptureSessionV1>,
    width: u32,
    height: u32,
    revision: u64,
    formats: Vec<wl_shm::Format>,
    constraints_pending: bool,
    stopped: bool,
    pending: Option<Pending>,
}
impl Drop for Output {
    fn drop(&mut self) {
        self.pending.take();
        if let Some(session) = self.session.take() {
            session.destroy();
        }
        self.output.release();
    }
}
struct Pending {
    request: CaptureRequest,
    pixels: Option<Pixels>,
    buffer: wl_buffer::WlBuffer,
    frame: frame::ExtImageCopyCaptureFrameV1,
    finished: bool,
    failed: bool,
    presentation_ms: Option<i64>,
    transform: Option<u32>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.frame.destroy();
        self.buffer.destroy();
    }
}

pub struct Captured {
    pub request: CaptureRequest,
    pub pixels: Option<Pixels>,
    pub failure: Option<&'static str>,
}

pub struct DesktopCapture {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
    budget: PixelBudget,
}
impl DesktopCapture {
    pub fn connect(budget: PixelBudget) -> Result<Self> {
        ensure!(
            cfg!(target_endian = "little"),
            "SHM normalization requires little endian"
        );
        let connection = Connection::connect_to_env()?;
        let queue = connection.new_event_queue();
        connection.display().get_registry(&queue.handle(), ());
        let mut capture = Self {
            connection,
            queue,
            state: State::default(),
            budget,
        };
        capture.synchronize()?;
        ensure!(
            capture.state.shm.is_some()
                && capture.state.sources.is_some()
                && capture.state.manager.is_some(),
            "ext SHM screenshot protocols unavailable"
        );
        capture.create_sessions();
        capture.synchronize()?;
        Ok(capture)
    }
    fn create_sessions(&mut self) {
        let (Some(sources), Some(manager)) = (&self.state.sources, &self.state.manager) else {
            return;
        };
        let handle = self.queue.handle();
        for (id, output) in &mut self.state.outputs {
            if output.session.is_none() {
                let source = sources.create_source(&output.output, &handle, ());
                output.session =
                    Some(manager.create_session(&source, manager::Options::empty(), &handle, *id));
                source.destroy();
            }
        }
    }
    pub fn monitors(&self) -> Vec<Monitor> {
        self.state
            .outputs
            .values()
            .filter(|o| !o.stopped && o.revision > 0 && !o.name.is_empty())
            .map(|o| Monitor {
                name: o.name.clone(),
                width: o.width,
                height: o.height,
                revision: o.revision,
            })
            .collect()
    }
    pub fn synchronize(&mut self) -> Result<()> {
        self.state.synced = false;
        self.connection.display().sync(&self.queue.handle(), ());
        let deadline = Instant::now() + Duration::from_millis(500);
        while !self.state.synced {
            ensure!(
                Instant::now() < deadline,
                "capture synchronization timed out"
            );
            self.pump(25)?;
        }
        Ok(())
    }
    fn pump(&mut self, timeout: i32) -> Result<()> {
        self.queue.dispatch_pending(&mut self.state)?;
        self.connection.flush()?;
        if let Some(guard) = self.queue.prepare_read() {
            let mut fd = libc::pollfd {
                fd: self.connection.as_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let count = unsafe { libc::poll(&mut fd, 1, timeout) };
            if count < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(error.into());
                }
            } else if count > 0 {
                match guard.read() {
                    Ok(_) => (),
                    Err(wayland_client::backend::WaylandError::Io(e))
                        if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        self.queue.dispatch_pending(&mut self.state)?;
        ensure!(!self.state.removed, "capture protocol global removed");
        ensure!(self.state.outputs.len() <= 32, "too many Wayland outputs");
        Ok(())
    }
    pub fn poll(&mut self) -> Result<Vec<Captured>> {
        self.pump(0)?;
        self.create_sessions();
        let stamp = crate::daemon::now()?;
        let mut frames = Vec::new();
        for output in self.state.outputs.values_mut() {
            if output.pending.as_ref().is_none_or(|p| {
                !p.finished && stamp.monotonic_ms - p.request.sample.monotonic_ms < 10_000
            }) {
                continue;
            }
            let mut pending = output.pending.take().unwrap();
            let failure = if !pending.finished {
                Some("capture_stalled")
            } else if pending.failed {
                Some("capture_failed")
            } else if pending.request.monitor.revision != output.revision {
                Some("monitor_changed")
            } else {
                None
            };
            let mut failure = failure;
            if failure.is_none() {
                if let (Some(time), Some(transform)) = (pending.presentation_ms, pending.transform)
                {
                    // ext presentation_time is explicitly in system monotonic time.
                    // Reject stale/future protocol timestamps rather than invent coverage.
                    if time < pending.request.sample.monotonic_ms || time > stamp.monotonic_ms + 1 {
                        failure = Some("invalid_capture_time");
                    } else {
                        pending.request.sample.monotonic_ms = time;
                        pending.request.sample.unix_ms =
                            stamp.unix_ms - (stamp.monotonic_ms - time);
                        pending.request.sample.transform = transform;
                    }
                } else {
                    failure = Some("missing_capture_time_or_transform");
                }
            }
            frames.push(Captured {
                request: pending.request.clone(),
                pixels: if failure.is_none() {
                    pending.pixels.take()
                } else {
                    None
                },
                failure,
            });
        }
        Ok(frames)
    }
    pub fn discard_obsolete(&mut self, recorder: &Recorder) {
        for output in self.state.outputs.values_mut() {
            if output
                .pending
                .as_ref()
                .is_some_and(|p| !recorder.capture_valid(&p.request))
            {
                output.pending.take();
            }
        }
    }
    /// Return a visible skip reason without buffering another request.
    pub fn request(&mut self, request: CaptureRequest) -> Result<Option<&'static str>> {
        let Some((id, output)) = self
            .state
            .outputs
            .iter_mut()
            .find(|(_, o)| o.name == request.monitor.name)
        else {
            return Ok(Some("monitor_removed"));
        };
        if output.pending.is_some() {
            return Ok(Some("capture_pending"));
        }
        if output.revision != request.monitor.revision
            || output.constraints_pending
            || output.stopped
        {
            return Ok(Some("monitor_changed"));
        }
        let format = if output.formats.contains(&wl_shm::Format::Xrgb8888) {
            wl_shm::Format::Xrgb8888
        } else if output.formats.contains(&wl_shm::Format::Argb8888) {
            wl_shm::Format::Argb8888
        } else {
            return Ok(Some("unsupported_shm_format"));
        };
        let pixels = match self.budget.allocate(output.width, output.height) {
            Ok(Some(pixels)) => pixels,
            Ok(None) => return Ok(Some("raw_pixel_budget")),
            Err(_) => return Ok(Some("unsupported_or_unavailable_buffer")),
        };
        let handle = self.queue.handle();
        let pool = self
            .state
            .shm
            .as_ref()
            .context("SHM unavailable")?
            .create_pool(pixels.file.as_fd(), pixels.map.len() as i32, &handle, ());
        let buffer = pool.create_buffer(
            0,
            output.width as i32,
            output.height as i32,
            (output.width * 4) as i32,
            format,
            &handle,
            (),
        );
        pool.destroy();
        let frame = output
            .session
            .as_ref()
            .context("capture session unavailable")?
            .create_frame(&handle, *id);
        frame.attach_buffer(&buffer);
        frame.damage_buffer(0, 0, output.width as i32, output.height as i32);
        frame.capture();
        output.pending = Some(Pending {
            request,
            pixels: Some(pixels),
            buffer,
            frame,
            finished: false,
            failed: false,
            presentation_ms: None,
            transform: None,
        });
        self.connection.flush()?;
        Ok(None)
    }
}
impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        s: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        q: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                match interface.as_str() {
                    "wl_output" if version >= 4 => {
                        // Bound allocation even before the next dispatch returns.
                        if s.outputs.len() >= 32 {
                            s.removed = true;
                            return;
                        }
                        s.outputs.insert(
                            name,
                            Output {
                                output: registry.bind(name, 4, q, name),
                                name: String::new(),
                                session: None,
                                width: 0,
                                height: 0,
                                revision: 0,
                                formats: vec![],
                                constraints_pending: false,
                                stopped: false,
                                pending: None,
                            },
                        );
                        return;
                    }
                    "wl_shm" => s.shm = Some(registry.bind(name, 1, q, ())),
                    "ext_output_image_capture_source_manager_v1" => {
                        s.sources = Some(registry.bind(name, 1, q, ()))
                    }
                    "ext_image_copy_capture_manager_v1" => {
                        s.manager = Some(registry.bind(name, 1, q, ()))
                    }
                    _ => return,
                }
                s.globals.push(name);
            }
            wl_registry::Event::GlobalRemove { name } => {
                s.outputs.remove(&name);
                if s.globals.contains(&name) {
                    s.removed = true;
                }
            }
            _ => (),
        }
    }
}
impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(
        s: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event
            && let Some(output) = s.outputs.get_mut(id)
        {
            output.name = name;
        }
    }
}
impl Dispatch<session::ExtImageCopyCaptureSessionV1, u32> for State {
    fn event(
        s: &mut Self,
        _: &session::ExtImageCopyCaptureSessionV1,
        event: session::Event,
        id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(output) = s.outputs.get_mut(id) else {
            return;
        };
        match event {
            session::Event::Stopped => {
                output.stopped = true;
                output.pending.take();
            }
            session::Event::Done => {
                s.revision += 1;
                output.revision = s.revision;
                output.constraints_pending = false;
            }
            event => {
                if !output.constraints_pending {
                    output.formats.clear();
                    output.constraints_pending = true;
                }
                match event {
                    session::Event::BufferSize { width, height } => {
                        output.width = width;
                        output.height = height;
                    }
                    session::Event::ShmFormat {
                        format: WEnum::Value(format),
                    } if output.formats.len() < 64 => {
                        output.formats.push(format);
                    }
                    _ => (),
                }
            }
        }
    }
}
impl Dispatch<frame::ExtImageCopyCaptureFrameV1, u32> for State {
    fn event(
        s: &mut Self,
        frame: &frame::ExtImageCopyCaptureFrameV1,
        event: frame::Event,
        id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(p) = s.outputs.get_mut(id).and_then(|o| o.pending.as_mut()) else {
            return;
        };
        if &p.frame != frame {
            return;
        }
        match event {
            frame::Event::Ready => p.finished = true,
            frame::Event::Failed { .. } => {
                p.failed = true;
                p.finished = true;
            }
            frame::Event::Transform {
                transform: WEnum::Value(transform),
            } => p.transform = Some(transform as u32),
            frame::Event::PresentationTime {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
            } => {
                let seconds = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
                if tv_nsec < 1_000_000_000 {
                    p.presentation_ms = seconds
                        .checked_mul(1000)
                        .and_then(|s| s.checked_add(u64::from(tv_nsec) / 1_000_000))
                        .and_then(|s| s.try_into().ok());
                }
            }
            _ => (),
        }
    }
}
impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        s: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        s.synced = true;
    }
}
wayland_client::delegate_noop!(State: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(State: ignore wl_buffer::WlBuffer);
wayland_client::delegate_noop!(State: ignore sources::ExtOutputImageCaptureSourceManagerV1);
wayland_client::delegate_noop!(State: ignore source::ExtImageCaptureSourceV1);
wayland_client::delegate_noop!(State: ignore manager::ExtImageCopyCaptureManagerV1);
