use super::*;
use crate::{
    capture::CaptureTrigger,
    recording::{Input, Stamp},
};
use std::os::unix::net::UnixStream;
use wayland_client::Proxy;

// Real client protocol objects on an isolated socket pair. Dispatch synthetic
// compositor events through the production handlers without touching a desktop.
struct Harness {
    capture: DesktopCapture,
    registry: wl_registry::WlRegistry,
    _peer: UnixStream,
}
impl Harness {
    fn new() -> Self {
        let (socket, peer) = UnixStream::pair().unwrap();
        let connection = Connection::from_socket(socket).unwrap();
        let queue = connection.new_event_queue();
        let registry = connection.display().get_registry(&queue.handle(), ());
        let mut h = Self {
            capture: DesktopCapture {
                connection,
                queue,
                state: State::default(),
                budget: PixelBudget::default(),
            },
            registry,
            _peer: peer,
        };
        for (name, interface, version) in [
            (1, "wl_shm", 1),
            (2, "ext_output_image_capture_source_manager_v1", 1),
            (3, "ext_image_copy_capture_manager_v1", 1),
            (4, "wl_output", 4),
            (5, "wl_output", 4),
        ] {
            h.registry_event(wl_registry::Event::Global {
                name,
                interface: interface.into(),
                version,
            });
        }
        for (id, output) in &mut h.capture.state.outputs {
            output.name = format!("DP-{id}");
        }
        h.capture.create_sessions(Instant::now());
        h.constraints(4, 2, 1);
        h.constraints(5, 2, 1);
        h
    }
    fn registry_event(&mut self, event: wl_registry::Event) {
        let c = &mut self.capture;
        <State as Dispatch<wl_registry::WlRegistry, ()>>::event(
            &mut c.state,
            &self.registry,
            event,
            &(),
            &c.connection,
            &c.queue.handle(),
        );
    }
    fn session(&self, id: u32) -> session::ExtImageCopyCaptureSessionV1 {
        self.capture.state.outputs[&id].session.clone().unwrap()
    }
    fn event(
        &mut self,
        id: u32,
        session: &session::ExtImageCopyCaptureSessionV1,
        event: session::Event,
    ) {
        let c = &mut self.capture;
        <State as Dispatch<session::ExtImageCopyCaptureSessionV1, u32>>::event(
            &mut c.state,
            session,
            event,
            &id,
            &c.connection,
            &c.queue.handle(),
        );
    }
    fn constraints(&mut self, id: u32, width: u32, height: u32) {
        let session = self.session(id);
        self.event(id, &session, session::Event::BufferSize { width, height });
        self.event(
            id,
            &session,
            session::Event::ShmFormat {
                format: WEnum::Value(wl_shm::Format::Xrgb8888),
            },
        );
        self.event(id, &session, session::Event::Done);
    }
    fn frame_event(
        &mut self,
        id: u32,
        frame: &frame::ExtImageCopyCaptureFrameV1,
        event: frame::Event,
    ) {
        let c = &mut self.capture;
        <State as Dispatch<frame::ExtImageCopyCaptureFrameV1, u32>>::event(
            &mut c.state,
            frame,
            event,
            &id,
            &c.connection,
            &c.queue.handle(),
        );
    }
    fn ready(&mut self, id: u32, request: &CaptureRequest) {
        let frame = self.capture.state.outputs[&id]
            .pending
            .as_ref()
            .unwrap()
            .frame
            .clone();
        let ms = request.sample.monotonic_ms as u64;
        self.frame_event(
            id,
            &frame,
            frame::Event::Transform {
                transform: WEnum::Value(wl_output::Transform::Normal),
            },
        );
        self.frame_event(
            id,
            &frame,
            frame::Event::PresentationTime {
                tv_sec_hi: ((ms / 1000) >> 32) as u32,
                tv_sec_lo: (ms / 1000) as u32,
                tv_nsec: (ms % 1000) as u32 * 1_000_000,
            },
        );
        self.frame_event(id, &frame, frame::Event::Ready);
    }
}
fn stamp(seconds: i64) -> Stamp {
    Stamp {
        unix_ms: 1_000_000 + seconds * 1000,
        monotonic_ms: seconds * 1000,
        boot_ms: seconds * 1000,
    }
}
fn recorder(h: &Harness, dir: &std::path::Path) -> Recorder {
    let mut r = Recorder::open(dir, stamp(0)).unwrap();
    r.apply(
        Input::Connected {
            locked: false,
            idle: false,
        },
        stamp(0),
    )
    .unwrap();
    r.apply(Input::Focus(None), stamp(0)).unwrap();
    r.set_monitors(h.capture.monitors(), stamp(0)).unwrap();
    r
}

#[test]
fn stopped_session_recovers_only_its_output_and_rejects_retired_work() {
    let mut h = Harness::new();
    let dir = tempfile::tempdir().unwrap();
    let mut r = recorder(&h, dir.path());
    let requests = r.request_captures(CaptureTrigger::Timer, stamp(0)).unwrap();
    let old_request = requests.iter().find(|r| r.monitor.name == "DP-4").unwrap();
    let other_request = requests.iter().find(|r| r.monitor.name == "DP-5").unwrap();
    h.capture.request(old_request.clone()).unwrap();
    let old_frame = h.capture.state.outputs[&4]
        .pending
        .as_ref()
        .unwrap()
        .frame
        .clone();
    let old = h.session(4);
    let healthy = h.session(5);
    h.event(4, &old, session::Event::Stopped);
    assert!(!old.is_alive());
    assert!(!old_frame.is_alive());
    assert!(h.capture.state.outputs[&4].pending.is_none());
    assert!(h.capture.state.outputs[&4].formats.is_empty());
    assert_eq!(h.capture.monitors().len(), 1);
    r.set_monitors(h.capture.monitors(), stamp(1)).unwrap();
    assert!(!r.capture_valid(old_request));
    assert!(r.capture_valid(other_request));
    assert!(r.status().focus_known);
    let gaps = r.history().screenshots(0, 100).unwrap();
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].sample.monitor, "DP-4");
    let at = h.capture.state.outputs[&4].retry.at.unwrap();
    h.capture.create_sessions(at - Duration::from_nanos(1));
    assert!(h.capture.state.outputs[&4].session.is_none());
    h.capture.create_sessions(at);
    let replacement = h.session(4);
    assert_ne!(replacement, old);
    assert_eq!(h.session(5), healthy);
    assert_eq!(
        h.capture.monitors().len(),
        1,
        "replacement must await fresh constraints"
    );
    for event in [
        session::Event::BufferSize {
            width: 999,
            height: 999,
        },
        session::Event::Done,
        session::Event::Stopped,
    ] {
        h.event(4, &old, event);
    }
    assert_eq!(h.session(4), replacement);
    assert_eq!(h.capture.state.outputs[&4].revision, 0);
    h.constraints(4, 4, 2);
    r.set_monitors(h.capture.monitors(), stamp(2)).unwrap();
    assert!(!r.capture_valid(old_request));
    assert!(r.capture_valid(other_request));
    assert_eq!(
        h.capture.state.outputs[&4].retry.delay,
        Duration::from_secs(4),
        "constraints alone must not reset backoff"
    );
    let fresh = r
        .request_captures(CaptureTrigger::Timer, stamp(5))
        .unwrap()
        .into_iter()
        .find(|r| r.monitor.name == "DP-4")
        .unwrap();
    assert_eq!((fresh.monitor.width, fresh.monitor.height), (4, 2));
    h.capture.request(fresh.clone()).unwrap();
    h.frame_event(4, &old_frame, frame::Event::Ready);
    assert!(
        !h.capture.state.outputs[&4]
            .pending
            .as_ref()
            .unwrap()
            .finished
    );
    h.ready(4, &fresh);
    let frames = h.capture.poll().unwrap();
    assert_eq!(frames.len(), 1);
    assert!(frames[0].failure.is_none());
    assert!(frames[0].pixels.is_some());
    assert!(r.capture_valid(&frames[0].request));
    assert_eq!(
        h.capture.state.outputs[&4].retry.delay,
        SESSION_RETRY_INITIAL
    );
    assert!(h.capture.state.outputs[&4].retry.at.is_none());
    assert_eq!(h.session(5), healthy);
}

#[test]
fn repeated_stops_back_off_without_resetting_on_announcements_and_removal_cancels_retry() {
    let mut h = Harness::new();
    let healthy = h.session(5);
    for seconds in [2, 4, 8, 16, 30, 30, 30] {
        let current = h.session(4);
        let before = Instant::now();
        h.event(4, &current, session::Event::Stopped);
        let after = Instant::now();
        let at = h.capture.state.outputs[&4].retry.at.unwrap();
        assert!(
            at >= before + Duration::from_secs(seconds)
                && at <= after + Duration::from_secs(seconds)
        );
        h.capture.create_sessions(at - Duration::from_nanos(1));
        assert!(h.capture.state.outputs[&4].session.is_none());
        h.capture.create_sessions(at);
        h.constraints(4, 2, 1);
        assert_eq!(h.session(5), healthy);
    }
    let current = h.session(4);
    h.event(4, &current, session::Event::Stopped);
    let at = h.capture.state.outputs[&4].retry.at.unwrap();
    h.registry_event(wl_registry::Event::GlobalRemove { name: 4 });
    h.capture.create_sessions(at);
    assert_eq!(h.capture.monitors().len(), 1);
    assert_eq!(h.session(5), healthy);
    assert!(!h.capture.state.outputs.contains_key(&4));
}
