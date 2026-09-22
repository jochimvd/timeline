//! Screenshot policy at the Recording seam. Triggers do not know the backend,
//! encoder, or file layout; completion always rechecks the recording generation.
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
}

#[derive(Clone, Debug)]
pub struct CaptureRequest {
    pub monitor: Monitor,
    pub sample: ScreenshotSample,
    serial: u64,
}

#[derive(Default)]
pub(crate) struct CaptureState {
    monitors: BTreeMap<String, Monitor>,
    pending: BTreeMap<String, u64>,
    serial: u64,
    next_ms: i64,
    rotation: usize,
    pub pressure: bool,
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
            && !self.capture.monitors.is_empty();
        self.status.screenshot_pressure = self.capture.pressure;
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
        _: CaptureTrigger,
        stamp: Stamp,
    ) -> Result<Vec<CaptureRequest>> {
        self.apply(Input::Tick, stamp)?;
        if !self.status.screenshot_recording || stamp.monotonic_ms < self.capture.next_ms {
            return Ok(vec![]);
        }
        let missed =
            stamp.monotonic_ms - self.capture.next_ms >= crate::config::CAPTURE_INTERVAL_MS;
        self.capture.next_ms = stamp.monotonic_ms + crate::config::CAPTURE_INTERVAL_MS;
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
                self.history.screenshot_gap(&sample, "capture_pending")?;
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
