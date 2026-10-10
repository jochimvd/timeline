use anyhow::{Context, Result, ensure};
use std::{env, net::SocketAddr, path::PathBuf};

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

/// IANA timezone used to resolve activity dates in the shared query service.
pub const ACTIVITY_TIMEZONE: &str = "Europe/Oslo";

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
}
