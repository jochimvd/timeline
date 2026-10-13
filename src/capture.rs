//! Screenshot policy at the Recording seam. Triggers do not know the backend,
//! encoder, or file layout; completion always rechecks the recording generation.
//! Every requested capture, timer or window switch, restarts the interval.
use crate::{
    history::{PreparedImage, ScreenshotSample},
    recording::{Input, Recorder, Stamp},
};
use anyhow::{Result, ensure};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Monitor {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub revision: u64,
}

pub enum CaptureTrigger {
    Timer,
    /// A window switch whose focus has settled; never records gaps.
    Switch,
}

#[derive(Clone, Debug)]
pub struct CaptureRequest {
    pub monitor: Monitor,
    pub sample: ScreenshotSample,
    serial: u64,
}

#[derive(Default)]
pub(crate) struct CaptureState {
    pub(crate) monitors: BTreeMap<String, Monitor>,
    pub(crate) pending: BTreeMap<String, u64>,
    pub(crate) serial: u64,
    pub(crate) next_ms: i64,
    pub(crate) last_request_ms: Option<i64>,
    pub(crate) switch_ms: Option<i64>,
    pub(crate) rotation: usize,
    pub pressure: bool,
    pub pressure_cleanup: bool,
}

impl Recorder {
    pub fn set_monitors(&mut self, monitors: Vec<Monitor>, stamp: Stamp) -> Result<()> {
        ensure!(monitors.len() <= 32, "too many monitors");
        let mut next = BTreeMap::new();
        for monitor in monitors {
            ensure!(
                !monitor.name.is_empty() && monitor.name.len() <= 256,
                "invalid monitor name"
            );
            ensure!(!next.contains_key(&monitor.name), "duplicate monitor name");
            next.insert(monitor.name.clone(), monitor);
        }
        let changed: Vec<_> = self
            .capture
            .monitors
            .values()
            .filter(|m| next.get(&m.name) != Some(m))
            .cloned()
            .collect();
        for monitor in changed {
            self.capture.pending.remove(&monitor.name);
            self.history
                .screenshot_gap(&self.sample(&monitor, stamp), "monitor_changed")?;
        }
        self.capture.monitors = next;
        self.update_capture_status();
        Ok(())
    }

    pub(crate) fn invalidate_captures(&mut self, reason: &str, stamp: Stamp) -> Result<()> {
        self.capture.pending.clear();
        self.capture.switch_ms = None;
        for monitor in self.capture.monitors.values() {
            self.history
                .screenshot_gap(&self.sample(monitor, stamp), reason)?;
        }
        self.capture.next_ms = stamp.monotonic_ms;
        Ok(())
    }

    pub(crate) fn update_capture_status(&mut self) {
        self.status.screenshot_recording = self.status.connected
            && !self.status.locked
            && !self.status.manual_paused
            && !self.capture.pressure
            && !self.capture.pressure_cleanup
            && !self.capture.monitors.is_empty();
        self.status.screenshot_pressure = self.capture.pressure;
        self.status.screenshot_cleanup = self.capture.pressure_cleanup;
    }

    fn sample(&self, monitor: &Monitor, stamp: Stamp) -> ScreenshotSample {
        ScreenshotSample {
            unix_ms: stamp.unix_ms,
            monotonic_ms: stamp.monotonic_ms,
            segment: self.segment,
            monitor: monitor.name.clone(),
            width: monitor.width,
            height: monitor.height,
            transform: 0,
        }
    }

    pub fn request_captures(
        &mut self,
        trigger: CaptureTrigger,
        stamp: Stamp,
    ) -> Result<Vec<CaptureRequest>> {
        use crate::config::{CAPTURE_INTERVAL_MS, CAPTURE_MIN_GAP_MS, FOCUS_SETTLE_MS};
        self.apply(Input::Tick, stamp)?;
        let timer = matches!(trigger, CaptureTrigger::Timer);
        if !self.status.screenshot_recording {
            if !timer {
                self.capture.switch_ms = None;
            }
            return Ok(vec![]);
        }
        let settled = self
            .capture
            .switch_ms
            .is_some_and(|at| stamp.monotonic_ms - at >= FOCUS_SETTLE_MS);
        if timer {
            if stamp.monotonic_ms < self.capture.next_ms {
                return Ok(vec![]);
            }
        } else {
            // A settled switch waits out the minimum gap and busy monitors.
            if !settled {
                return Ok(vec![]);
            }
            let recent = self
                .capture
                .last_request_ms
                .is_some_and(|at| stamp.monotonic_ms - at < CAPTURE_MIN_GAP_MS);
            let pending = &self.capture.pending;
            if recent
                || self
                    .capture
                    .monitors
                    .keys()
                    .all(|m| pending.contains_key(m))
            {
                return Ok(vec![]);
            }
        }
        let missed = timer && stamp.monotonic_ms - self.capture.next_ms >= CAPTURE_INTERVAL_MS;
        self.capture.next_ms = stamp.monotonic_ms + CAPTURE_INTERVAL_MS;
        self.capture.last_request_ms = Some(stamp.monotonic_ms);
        let mut monitors: Vec<_> = self.capture.monitors.values().cloned().collect();
        let count = monitors.len();
        monitors.rotate_left(self.capture.rotation % count);
        self.capture.rotation = (self.capture.rotation + 1) % count;
        let mut requests = Vec::new();
        for monitor in monitors {
            let sample = self.sample(&monitor, stamp);
            if missed {
                self.history.screenshot_gap(&sample, "trigger_overdue")?;
            }
            if self.capture.pending.contains_key(&monitor.name) {
                if timer {
                    self.history.screenshot_gap(&sample, "capture_pending")?;
                }
                continue;
            }
            self.capture.serial += 1;
            self.capture
                .pending
                .insert(monitor.name.clone(), self.capture.serial);
            requests.push(CaptureRequest {
                monitor,
                sample,
                serial: self.capture.serial,
            });
        }
        // Any capture of the settled window covers the switch.
        if settled && !requests.is_empty() {
            self.capture.switch_ms = None;
        }
        Ok(requests)
    }

    pub fn capture_valid(&self, request: &CaptureRequest) -> bool {
        self.status.screenshot_recording
            && request.sample.segment == self.segment
            && self.capture.monitors.get(&request.monitor.name) == Some(&request.monitor)
            && self.capture.pending.get(&request.monitor.name) == Some(&request.serial)
    }

    pub fn capture_failed(
        &mut self,
        request: &CaptureRequest,
        reason: &str,
        stamp: Stamp,
    ) -> Result<()> {
        self.apply(Input::Tick, stamp)?;
        if self.capture_valid(request) {
            self.capture.pending.remove(&request.monitor.name);
            self.history
                .screenshot_gap(&self.sample(&request.monitor, stamp), reason)?;
        }
        Ok(())
    }

    /// Called only for completed pixels; an outstanding request never reuses an image.
    pub fn reuse_capture(
        &mut self,
        request: &CaptureRequest,
        fingerprint: &str,
        stamp: Stamp,
    ) -> Result<bool> {
        self.apply(Input::Tick, stamp)?;
        if !self.capture_valid(request) {
            return Ok(false);
        }
        if let Some(image) = self.history.find_image(fingerprint)?
            && self
                .history
                .reuse_screenshot(&request.sample, &image)?
                .is_some()
        {
            self.capture.pending.remove(&request.monitor.name);
            return Ok(true);
        }
        Ok(false)
    }

    pub fn publish_capture(
        &mut self,
        request: &CaptureRequest,
        fingerprint: &str,
        image: &PreparedImage,
        stamp: Stamp,
    ) -> Result<bool> {
        self.apply(Input::Tick, stamp)?;
        if !self.capture_valid(request) {
            return Ok(false);
        }
        self.history
            .commit_screenshot(&request.sample, fingerprint, image)?;
        self.capture.pending.remove(&request.monitor.name);
        Ok(true)
    }
}

pub use crate::image_worker::{EncodedCapture, PixelBudget, Pixels, RecordingImages};
