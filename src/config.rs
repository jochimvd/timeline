use anyhow::{Context, Result, ensure};
use std::{env, path::PathBuf};

pub const RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;
pub const CLEANUP_INTERVAL_MS: i64 = 60 * 60 * 1000;
// Leave headroom for bounded socket/storage work before the five-second limit.
pub const CHECKPOINT_INTERVAL_MS: i64 = 4_000;

pub fn history_directory() -> Result<PathBuf> {
    if let Some(value) = env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        let directory = PathBuf::from(value);
        if directory.is_absolute() {
            return Ok(directory.join("timeline"));
        }
    }
    let home = PathBuf::from(
        env::var_os("HOME").context("HOME or an absolute XDG_DATA_HOME is required")?,
    );
    ensure!(home.is_absolute(), "HOME must be absolute");
    Ok(home.join(".local/share/timeline"))
}

pub const CAPTURE_INTERVAL_MS: i64 = 5_000;
pub const RAW_PIXEL_BUDGET: usize = 96 * 1024 * 1024;
pub const PRESSURE_STOP_BYTES: u64 = 20 * 1024 * 1024 * 1024;
pub const PRESSURE_RESUME_BYTES: u64 = 25 * 1024 * 1024 * 1024;
pub const WEBP_QUALITY: f32 = 80.0;
pub const WEBP_METHOD: i32 = 0;

/// IANA timezone used to resolve activity dates in the shared query service.
pub const ACTIVITY_TIMEZONE: &str = "Europe/Oslo";
