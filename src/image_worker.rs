//! Bounded raw pixel ownership and a single hashing/encoding/publication worker.
use crate::{
    capture::CaptureRequest,
    config,
    history::{History, PreparedImage},
};
use anyhow::{Result, ensure};
use memmap2::MmapMut;
use std::{
    fs::File,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
};

#[derive(Clone)]
pub struct PixelBudget(Arc<AtomicUsize>);
impl Default for PixelBudget {
    fn default() -> Self {
        Self(Arc::new(AtomicUsize::new(0)))
    }
}

pub struct Pixels {
    pub(crate) file: File,
    pub(crate) map: MmapMut,
    width: u32,
    height: u32,
    budget: PixelBudget,
}
impl Drop for Pixels {
    fn drop(&mut self) {
        self.budget.0.fetch_sub(self.map.len(), Ordering::AcqRel);
    }
}
impl Pixels {
    /// Native little-endian XRGB8888/BGRA buffer. Callers must own the completed
    /// compositor frame before accessing pixels; alpha is normalized to opaque.
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.map
    }
}
impl PixelBudget {
    pub fn allocate(&self, width: u32, height: u32) -> Result<Option<Pixels>> {
        ensure!(
            width > 0 && height > 0 && width <= 16383 && height <= 16383,
            "unsupported image dimensions"
        );
        let bytes = width as usize * height as usize * 4;
        if self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|total| *total <= config::RAW_PIXEL_BUDGET)
            })
            .is_err()
        {
            return Ok(None);
        }
        let allocation = (|| -> Result<_> {
            let file = tempfile::tempfile()?;
            file.set_len(bytes as u64)?;
            // This private file is never resized; only the compositor writes it
            // while a frame is pending, and only the worker reads after ready.
            let map = unsafe { MmapMut::map_mut(&file)? };
            Ok(Pixels {
                file,
                map,
                width,
                height,
                budget: self.clone(),
            })
        })();
        if allocation.is_err() {
            self.0.fetch_sub(bytes, Ordering::AcqRel);
        }
        allocation.map(Some)
    }
}

pub struct EncodedCapture {
    pub request: CaptureRequest,
    pub result: Result<(String, Option<PreparedImage>)>,
}

/// One work item total, including a completed result awaiting publication.
/// The worker has no authority to commit screenshot references.
pub struct RecordingImages {
    sender: SyncSender<(CaptureRequest, Pixels)>,
    receiver: Receiver<EncodedCapture>,
    busy: bool,
}
impl RecordingImages {
    pub fn start(directory: PathBuf) -> Result<Self> {
        let filesystem = directory.clone();
        Self::with_space_probe(directory, move || History::available_bytes(&filesystem))
    }

    /// External filesystem probe, shared with deterministic Recording tests.
    pub fn with_space_probe(
        directory: PathBuf,
        available: impl Fn() -> Result<u64> + Send + 'static,
    ) -> Result<Self> {
        let (sender, work) = mpsc::sync_channel::<(CaptureRequest, Pixels)>(1);
        let (results, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("image-encoder".into())
            .spawn(move || {
                while let Ok((request, mut pixels)) = work.recv() {
                    let result = (|| {
                        ensure!(
                            request.monitor.width == pixels.width
                                && request.monitor.height == pixels.height,
                            "capture dimensions changed"
                        );
                        for pixel in pixels.map.chunks_exact_mut(4) {
                            pixel[3] = 255;
                        }
                        let mut hasher = blake3::Hasher::new();
                        hasher.update(&pixels.width.to_le_bytes());
                        hasher.update(&pixels.height.to_le_bytes());
                        hasher.update(&request.sample.transform.to_le_bytes());
                        hasher.update(&pixels.map);
                        let fingerprint = hasher.finalize().to_hex().to_string();
                        let history = History::read(&directory)?;
                        if history.find_image(&fingerprint)?.is_some() {
                            return Ok((fingerprint, None));
                        }
                        ensure!(available()? >= config::PRESSURE_STOP_BYTES, "disk_pressure");
                        let prepared = History::prepare_pixels(
                            &directory,
                            pixels.width,
                            pixels.height,
                            &pixels.map,
                        )?;
                        Ok((fingerprint, Some(prepared)))
                    })();
                    drop(pixels);
                    if results.send(EncodedCapture { request, result }).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            sender,
            receiver,
            busy: false,
        })
    }
    pub fn submit(&mut self, request: CaptureRequest, pixels: Pixels) -> Result<bool> {
        if self.busy {
            return Ok(false);
        }
        self.sender
            .send((request, pixels))
            .map_err(|_| anyhow::anyhow!("image worker stopped"))?;
        self.busy = true;
        Ok(true)
    }
    pub fn busy(&self) -> bool {
        self.busy
    }

    pub fn poll(&mut self) -> Result<Option<EncodedCapture>> {
        match self.receiver.try_recv() {
            Ok(result) => {
                self.busy = false;
                Ok(Some(result))
            }
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}
