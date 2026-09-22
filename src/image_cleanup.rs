//! One coalescing image-reclamation worker owned by History. No queued backlog.
use crate::history::History;
use anyhow::Result;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::Duration,
};
pub(crate) struct ImageCleanup {
    sender: SyncSender<()>,
    receiver: Receiver<Result<()>>,
    stop: Arc<AtomicBool>,
    busy: bool,
    repeat: bool,
}
impl Drop for ImageCleanup {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
impl ImageCleanup {
    pub fn start(directory: PathBuf) -> Result<Self> {
        let (sender, work) = mpsc::sync_channel(1);
        let (results, receiver) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        std::thread::Builder::new()
            .name("image-cleanup".into())
            .spawn(move || {
                while work.recv().is_ok() {
                    let result = (|| {
                        let history = History::read(&directory)?;
                        while !stopping.load(Ordering::Relaxed)
                            && !history.reclaim_images(&stopping)?
                        {
                            std::thread::sleep(Duration::from_millis(100));
                        }
                        Ok(())
                    })();
                    if results.send(result).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            sender,
            receiver,
            stop,
            busy: false,
            repeat: false,
        })
    }
    pub fn request(&mut self) -> Result<()> {
        if self.busy {
            self.repeat = true;
        } else {
            self.sender.send(())?;
            self.busy = true;
        }
        Ok(())
    }
    pub fn pending(&mut self) -> Result<bool> {
        if self.busy {
            match self.receiver.try_recv() {
                Ok(result) => {
                    result?;
                    self.busy = false;
                    if self.repeat {
                        self.repeat = false;
                        self.request()?;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => (),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(self.busy)
    }
}
