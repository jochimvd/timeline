use anyhow::{Context, Result, ensure};
use std::{
    env, fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::OnceLock,
};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// Observations are small (a few MB per day) and feed trends, so they outlive
/// screenshots, which dominate storage.
pub const METADATA_RETENTION_MS: i64 = 365 * DAY_MS;
pub const SCREENSHOT_RETENTION_MS: i64 = 30 * DAY_MS;
pub const CLEANUP_INTERVAL_MS: i64 = 60 * 60 * 1000;
/// Healthy operation loses at most this much focus evidence.
pub const LOSS_BOUND_MS: i64 = 5_000;
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

/// User settings: `$XDG_CONFIG_HOME/timeline/config.toml`, else under `~/.config`.
pub fn settings_file() -> Result<PathBuf> {
    if let Some(value) = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        let directory = PathBuf::from(value);
        if directory.is_absolute() {
            return Ok(directory.join("timeline/config.toml"));
        }
    }
    let home = PathBuf::from(
        env::var_os("HOME").context("HOME or an absolute XDG_CONFIG_HOME is required")?,
    );
    ensure!(home.is_absolute(), "HOME must be absolute");
    Ok(home.join(".config/timeline/config.toml"))
}

/// Default HTTP endpoint; `TIMELINE_HTTP_ADDRESS` may choose another loopback one.
pub const HTTP_ADDRESS: &str = "127.0.0.1:47831";

pub fn http_address() -> Result<String> {
    match env::var_os("TIMELINE_HTTP_ADDRESS").filter(|value| !value.is_empty()) {
        Some(value) => loopback_address(
            value
                .to_str()
                .context("TIMELINE_HTTP_ADDRESS must be valid UTF-8")?,
        ),
        None => Ok(HTTP_ADDRESS.into()),
    }
}

/// Browser and MCP clients use this authority verbatim, so it must be concrete.
fn loopback_address(value: &str) -> Result<String> {
    let address: SocketAddr = value
        .parse()
        .with_context(|| format!("invalid HTTP address {value:?}"))?;
    ensure!(
        address.ip().is_loopback() && address.port() != 0,
        "HTTP address must be a loopback address with a fixed port"
    );
    Ok(address.to_string())
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
    fn http_address_override_must_be_a_fixed_loopback_port() {
        assert_eq!(
            loopback_address("127.0.0.1:47900").unwrap(),
            "127.0.0.1:47900"
        );
        assert_eq!(loopback_address("[::1]:47900").unwrap(), "[::1]:47900");
        for invalid in [
            "0.0.0.0:47900",
            "192.168.1.2:47900",
            "127.0.0.1:0",
            "localhost:1",
        ] {
            assert!(loopback_address(invalid).is_err(), "{invalid}");
        }
    }

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
