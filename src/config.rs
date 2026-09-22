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
