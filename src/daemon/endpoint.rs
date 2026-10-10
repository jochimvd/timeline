//! HTTP endpoint supervision. Like the desktop source, an unavailable endpoint
//! is reported and retried; it never stops recording.
use crate::web::Web;
use anyhow::{Result, anyhow};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    thread::JoinHandle,
    time::{Duration, Instant},
};

const RETRY: Duration = Duration::from_secs(2);

pub(super) struct Endpoint {
    web: Arc<Web>,
    stop: Arc<AtomicBool>,
    server: Option<JoinHandle<Result<()>>>,
    retry_at: Instant,
    pub error: Option<String>,
}

impl Endpoint {
    /// Configuration errors are terminal; binding happens in `step`.
    pub fn new(directory: &Path, address: &str, stop: Arc<AtomicBool>) -> Result<Self> {
        Ok(Self {
            web: Web::configured(directory, address, crate::config::settings_file().ok())?,
            stop,
            server: None,
            retry_at: Instant::now(),
            error: None,
        })
    }

    /// Reap a failed server and rebind when the retry interval has elapsed.
    pub fn step(&mut self) {
        if let Some(server) = self.server.take_if(|server| server.is_finished()) {
            self.failed(failure(server).unwrap_or_else(|| "HTTP server stopped".into()));
        }
        if self.server.is_none() && Instant::now() >= self.retry_at {
            match crate::web::start(self.web.clone(), self.stop.clone()) {
                Ok(server) => {
                    self.server = Some(server);
                    self.error = None;
                }
                Err(error) => self.failed(format!("{error:#}")),
            }
        }
    }

    pub fn launch_url(&self) -> Result<String> {
        if self.server.is_none() {
            return Err(anyhow!(
                "HTTP endpoint is not serving: {}",
                self.error.as_deref().unwrap_or("not started")
            ));
        }
        self.web.launch_url()
    }

    /// Wait for the server's graceful shutdown; the caller has set `stop`.
    pub fn shutdown(self) {
        if let Some(message) = self.server.and_then(failure) {
            eprintln!("HTTP endpoint failed: {message}");
        }
    }

    fn failed(&mut self, message: String) {
        if self.error.as_ref() != Some(&message) {
            eprintln!("HTTP endpoint unavailable: {message}");
        }
        self.error = Some(message);
        self.retry_at = Instant::now() + RETRY;
    }
}

fn failure(server: JoinHandle<Result<()>>) -> Option<String> {
    match server.join() {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(format!("{error:#}")),
        Err(_) => Some("HTTP server panicked".into()),
    }
}
