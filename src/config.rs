use anyhow::{Context, Result, ensure};
use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

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

/// Fallback IANA timezone for activity dates when the system zone is unknown.
pub const ACTIVITY_TIMEZONE: &str = "Europe/Oslo";

/// IANA timezone used to resolve activity dates: the system local zone from
/// an IANA `TZ` or the /etc/localtime link, else `ACTIVITY_TIMEZONE`.
/// Resolved once per process.
pub fn activity_timezone() -> &'static str {
    static ZONE: OnceLock<String> = OnceLock::new();
    ZONE.get_or_init(|| {
        let tz = env::var("TZ").ok();
        let localtime = fs::read_link("/etc/localtime").ok();
        system_timezone(tz.as_deref(), localtime.as_deref())
            .unwrap_or_else(|| ACTIVITY_TIMEZONE.into())
    })
}

fn system_timezone(tz: Option<&str>, localtime: Option<&Path>) -> Option<String> {
    tz.and_then(iana_name)
        .or_else(|| localtime.and_then(Path::to_str).and_then(iana_name))
}

/// An IANA name from `Area/City`, `:Area/City` or a zoneinfo path. POSIX
/// rule strings and unknown names are rejected.
fn iana_name(value: &str) -> Option<String> {
    let value = value.strip_prefix(':').unwrap_or(value);
    let name = value
        .rsplit_once("zoneinfo/")
        .map_or(value, |(_, name)| name);
    let name = ["posix/", "right/"]
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))
        .unwrap_or(name);
    name.parse::<chrono_tz::Tz>()
        .ok()
        .map(|zone| zone.name().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_timezone_prefers_an_iana_tz_then_the_localtime_link() {
        let link = Path::new("../usr/share/zoneinfo/Europe/Oslo");
        let resolve = |tz| system_timezone(tz, Some(link));
        assert_eq!(
            resolve(Some("America/New_York")).unwrap(),
            "America/New_York"
        );
        assert_eq!(resolve(Some(":Asia/Tokyo")).unwrap(), "Asia/Tokyo");
        assert_eq!(
            resolve(Some("/usr/share/zoneinfo/posix/Europe/Paris")).unwrap(),
            "Europe/Paris"
        );
        for fallback in [
            None,
            Some(""),
            Some("CET-1CEST,M3.5.0,M10.5.0/3"),
            Some("Mars/Base"),
        ] {
            assert_eq!(resolve(fallback).unwrap(), "Europe/Oslo");
        }
        assert!(system_timezone(None, Some(Path::new("/etc/zoneinfo-copy"))).is_none());
        assert!(system_timezone(None, None).is_none());
    }
}
