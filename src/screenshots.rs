//! Runtime wiring for the Recording image interfaces. Capture/encoding failures
//! degrade only image coverage; persistence failures remain terminal.
use crate::{
    capture::{CaptureTrigger, EncodedCapture, PixelBudget, RecordingImages},
    capture_desktop::{Captured, DesktopCapture},
    daemon::{SourceResult, apply_session_events, now},
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

    /// Session failures are marked `SourceError`, so the caller ends desktop
    /// coverage; worker and persistence failures stay terminal.
    pub fn step(&mut self, recorder: &mut Recorder, session: Option<&mut Session>) -> Result<()> {
        if Instant::now() >= self.pressure_at {
            self.check_pressure(recorder)?;
            self.pressure_at = Instant::now() + Duration::from_secs(5);
        }
        let Some(session) = session else {
            return self.disconnected(recorder);
        };
        if self.backend.is_none() && Instant::now() >= self.retry {
            self.connect();
        }
        self.poll_backend(recorder)?;
        if let Some(result) = self.worker.poll()? {
            self.commit(recorder, session, result)?;
        }
        self.submit_ready(recorder)?;
        self.request_captures(recorder, session)
    }

    fn check_pressure(&self, recorder: &mut Recorder) -> Result<()> {
        recorder.check_storage_pressure(now()?, || History::available_bytes(&self.directory))
    }

    fn disconnected(&mut self, recorder: &mut Recorder) -> Result<()> {
        self.backend = None;
        self.ready.clear();
        recorder.set_monitors(vec![], now()?)?;
        // Consume obsolete worker output even while disconnected.
        self.worker.poll()?;
        Ok(())
    }

    fn connect(&mut self) {
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

    fn poll_backend(&mut self, recorder: &mut Recorder) -> Result<()> {
        let Some(backend) = self.backend.as_mut() else {
            return Ok(());
        };
        match backend.poll() {
            Ok(frames) => {
                recorder.set_monitors(backend.monitors(), now()?)?;
                backend.discard_obsolete(recorder);
                self.ready.extend(frames);
                Ok(())
            }
            Err(error) => self.backend_failed(recorder, error),
        }
    }

    /// Refresh monitor constraints and lock state AFTER image preparation,
    /// before the sole thread authorized to commit references proceeds.
    fn commit(
        &mut self,
        recorder: &mut Recorder,
        session: &mut Session,
        result: EncodedCapture,
    ) -> Result<()> {
        if let Some(backend) = self.backend.as_mut() {
            match backend.synchronize() {
                Ok(()) => recorder.set_monitors(backend.monitors(), now()?)?,
                Err(error) => self.backend_failed(recorder, error)?,
            }
        }
        self.check_pressure(recorder)?;
        session.synchronize().source_failure()?;
        apply_session_events(recorder, session)?;
        self.publish(recorder, result)
    }

    fn submit_ready(&mut self, recorder: &mut Recorder) -> Result<()> {
        self.ready
            .retain(|frame| recorder.capture_valid(&frame.request));
        if self.worker.busy() {
            return Ok(());
        }
        while let Some(frame) = self.ready.pop_front() {
            if let Some(reason) = frame.failure {
                recorder.capture_failed(&frame.request, reason, now()?)?;
            } else if let Some(pixels) = frame.pixels {
                self.worker.submit(frame.request, pixels)?;
                break;
            }
        }
        Ok(())
    }

    fn request_captures(&mut self, recorder: &mut Recorder, session: &mut Session) -> Result<()> {
        let Some(backend) = self.backend.as_mut() else {
            return Ok(());
        };
        // Applying current lock events also invalidates captures while the
        // worker is busy; no new sample starts based on old lock state.
        apply_session_events(recorder, session)?;
        backend.discard_obsolete(recorder);
        // The timer goes first so overdue gaps are still recorded; its capture
        // also covers a switch that has already settled.
        for trigger in [CaptureTrigger::Timer, CaptureTrigger::Switch] {
            for request in recorder.request_captures(trigger, now()?)? {
                match backend.request(request.clone()) {
                    Ok(Some(reason)) => recorder.capture_failed(&request, reason, now()?)?,
                    Ok(None) => (),
                    Err(error) => {
                        recorder.capture_failed(&request, "capture_disconnected", now()?)?;
                        return self.backend_failed(recorder, error);
                    }
                }
            }
        }
        Ok(())
    }

    fn backend_failed(&mut self, recorder: &mut Recorder, error: anyhow::Error) -> Result<()> {
        self.error = Some(format!("{error:#}"));
        self.backend = None;
        self.ready.clear();
        self.retry = Instant::now() + Duration::from_secs(2);
        recorder.set_monitors(vec![], now()?)
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
