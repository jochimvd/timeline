//! Runtime wiring for the Recording image interfaces. Capture/encoding failures
//! degrade only image coverage; persistence failures remain terminal.
use crate::{
    capture::{CaptureTrigger, EncodedCapture, PixelBudget, RecordingImages},
    capture_desktop::{Captured, DesktopCapture},
    daemon::now,
    history::History,
    recording::Recorder,
    session::Session,
};
use anyhow::Result;
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(crate) struct Screenshots {
    backend: Option<DesktopCapture>,
    budget: PixelBudget,
    worker: RecordingImages,
    ready: VecDeque<Captured>,
    retry: Instant,
    pressure_at: Instant,
    directory: PathBuf,
    pub error: Option<String>,
}
impl Screenshots {
    pub fn new(directory: &Path) -> Result<Self> {
        Ok(Self {
            backend: None,
            budget: PixelBudget::default(),
            worker: RecordingImages::start(directory.to_owned())?,
            ready: VecDeque::new(),
            retry: Instant::now(),
            pressure_at: Instant::now(),
            directory: directory.to_owned(),
            error: None,
        })
    }
    pub fn step(&mut self, recorder: &mut Recorder, session: Option<&mut Session>) -> Result<()> {
        if Instant::now() >= self.pressure_at {
            recorder
                .check_storage_pressure(now()?, || History::available_bytes(&self.directory))?;
            self.pressure_at = Instant::now() + Duration::from_secs(5);
        }
        let Some(session) = session else {
            self.backend = None;
            self.ready.clear();
            recorder.set_monitors(vec![], now()?)?;
            // Consume obsolete worker output even while disconnected.
            self.worker.poll()?;
            return Ok(());
        };
        if self.backend.is_none() && Instant::now() >= self.retry {
            match DesktopCapture::connect(self.budget.clone()) {
                Ok(backend) => {
                    self.backend = Some(backend);
                    self.error = None;
                }
                Err(error) => {
                    self.error = Some(format!("{error:#}"));
                    self.retry = Instant::now() + Duration::from_secs(2);
                }
            }
        }
        if let Some(backend) = self.backend.as_mut() {
            match backend.poll() {
                Ok(frames) => {
                    recorder.set_monitors(backend.monitors(), now()?)?;
                    backend.discard_obsolete(recorder);
                    self.ready.extend(frames);
                }
                Err(error) => {
                    self.error = Some(format!("{error:#}"));
                    self.backend = None;
                    recorder.set_monitors(vec![], now()?)?;
                    self.retry = Instant::now() + Duration::from_secs(2);
                }
            }
        }
        if let Some(result) = self.worker.poll()? {
            // Refresh monitor constraints and lock state AFTER image preparation,
            // before the sole thread authorized to commit references proceeds.
            if let Some(backend) = self.backend.as_mut() {
                match backend.synchronize() {
                    Ok(()) => recorder.set_monitors(backend.monitors(), now()?)?,
                    Err(error) => {
                        self.error = Some(format!("{error:#}"));
                        self.backend = None;
                        recorder.set_monitors(vec![], now()?)?;
                    }
                }
            }
            recorder
                .check_storage_pressure(now()?, || History::available_bytes(&self.directory))?;
            session.synchronize()?;
            for event in session.events()? {
                recorder.apply(event, now()?)?;
            }
            self.publish(recorder, result)?;
        }
        self.ready
            .retain(|frame| recorder.capture_valid(&frame.request));
        if !self.worker.busy() {
            while let Some(frame) = self.ready.pop_front() {
                if let Some(reason) = frame.failure {
                    recorder.capture_failed(&frame.request, reason, now()?)?;
                } else if let Some(pixels) = frame.pixels {
                    self.worker.submit(frame.request, pixels)?;
                    break;
                }
            }
        }
        if let Some(backend) = self.backend.as_mut() {
            // Applying current lock events also invalidates captures while the
            // worker is busy; no new sample starts based on old lock state.
            for event in session.events()? {
                recorder.apply(event, now()?)?;
            }
            backend.discard_obsolete(recorder);
            for request in recorder.request_captures(CaptureTrigger::Timer, now()?)? {
                match backend.request(request.clone()) {
                    Ok(Some(reason)) => recorder.capture_failed(&request, reason, now()?)?,
                    Ok(None) => (),
                    Err(error) => {
                        self.error = Some(format!("{error:#}"));
                        recorder.capture_failed(&request, "capture_disconnected", now()?)?;
                        recorder.set_monitors(vec![], now()?)?;
                        self.backend = None;
                        break;
                    }
                }
            }
        }
        Ok(())
    }
    fn publish(&mut self, recorder: &mut Recorder, result: EncodedCapture) -> Result<()> {
        if !recorder.capture_valid(&result.request) {
            return Ok(());
        }
        match result.result {
            Ok((fingerprint, Some(image))) => {
                recorder.publish_capture(&result.request, &fingerprint, &image, now()?)?;
                self.error = None;
            }
            Ok((fingerprint, None)) => {
                if !recorder.reuse_capture(&result.request, &fingerprint, now()?)? {
                    recorder.capture_failed(&result.request, "duplicate_expired", now()?)?;
                }
            }
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                recorder.capture_failed(
                    &result.request,
                    "image_storage_or_encoding_failed",
                    now()?,
                )?;
            }
        }
        Ok(())
    }
}
