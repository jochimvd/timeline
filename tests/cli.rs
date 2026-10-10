mod common;
use std::{
    net::{SocketAddr, TcpListener},
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use timeline::recording::{Input, Recorder, Stamp, Window};

#[test]
fn inspection_reads_committed_evidence_with_explicit_pagination() {
    let root = tempfile::tempdir().unwrap();
    let stamp = Stamp {
        unix_ms: 1000,
        monotonic_ms: 1,
        boot_ms: 1,
    };
    let mut recorder = Recorder::open(&root.path().join("timeline"), stamp).unwrap();
    recorder
        .apply(
            Input::Connected {
                locked: false,
                idle: false,
            },
            stamp,
        )
        .unwrap();
    recorder
        .apply(
            Input::Focus(Some(Window {
                address: "abc".into(),
                application: "editor".into(),
                title: "test document".into(),
            })),
            stamp,
        )
        .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_timeline"))
        .env("XDG_DATA_HOME", root.path())
        .args(["observations", "--limit", "2"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let page: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(page["observations"].as_array().unwrap().len(), 2);
    assert_eq!(page["has_more"], true);
    let output = Command::new(env!("CARGO_BIN_EXE_timeline"))
        .env("XDG_DATA_HOME", root.path())
        .args(["observations", "--after", &page["next_after"].to_string()])
        .output()
        .unwrap();
    let page: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        page["observations"][0]["evidence"]["state"]["focus"]["title"],
        "test document"
    );
    assert_eq!(page["has_more"], false);
}

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A recorder command isolated from the live desktop and from the default HTTP
/// port, which a real recorder may hold.
fn recorder(data: &Path, http: SocketAddr) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_timeline"));
    command
        .env("XDG_DATA_HOME", data)
        .env("TIMELINE_HTTP_ADDRESS", http.to_string())
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("XDG_RUNTIME_DIR");
    command
}

fn unused_address() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

fn start(command: impl Fn() -> Command) -> Running {
    let child = Running(
        command()
            .arg("run")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_for_status(&command, |_| true);
    child
}

fn wait_for_status(
    command: impl Fn() -> Command,
    ready: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        let output = command().arg("status").output().unwrap();
        if output.status.success() {
            let status = serde_json::from_slice(&output.stdout).unwrap();
            if ready(&status) {
                return status;
            }
        }
        assert!(Instant::now() < deadline, "recorder did not reach state");
        thread::sleep(Duration::from_millis(20));
    }
}

fn boundaries(command: impl Fn() -> Command) -> Vec<serde_json::Value> {
    let output = command().arg("observations").output().unwrap();
    assert!(output.status.success());
    let page: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    page["observations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["evidence"]["reason"] == "startup" || o["evidence"]["reason"] == "restart")
        .cloned()
        .collect()
}

#[test]
fn daemon_pause_is_durable_after_forced_exit_and_duplicate_recorder_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let http = unused_address();
    let command = || recorder(root.path(), http);
    let child = start(command);
    assert!(command().arg("pause").output().unwrap().status.success());
    let second = command().arg("run").output().unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("another recorder"));
    drop(child); // SIGKILL, intentionally bypassing orderly shutdown.
    let _restarted = start(command);
    let output = command().arg("status").output().unwrap();
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["recording"]["manual_paused"], true);
    assert_eq!(status["recording"]["focus_known"], false);
    assert_eq!(status["recording"]["screenshot_recording"], false);
    assert!(command().arg("resume").output().unwrap().status.success());
    let reasons: Vec<_> = boundaries(command)
        .iter()
        .map(|o| o["evidence"]["reason"].clone())
        .collect();
    assert_eq!(reasons, ["startup", "restart"]);
}

#[test]
fn occupied_http_port_degrades_access_but_not_recording() {
    let root = tempfile::tempdir().unwrap();
    let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
    let http = occupied.local_addr().unwrap();
    let command = || recorder(root.path(), http);
    let _child = start(command);
    let status = wait_for_status(command, |status| status["web_error"].is_string());
    assert!(
        status["web_error"]
            .as_str()
            .unwrap()
            .contains(&http.to_string())
    );
    // Without PATH, a regression cannot reach a real browser via xdg-open.
    let launch = command().arg("open").env("PATH", "").output().unwrap();
    assert!(!launch.status.success());
    assert!(String::from_utf8_lossy(&launch.stderr).contains("not serving"));
    assert!(command().arg("pause").output().unwrap().status.success());
    drop(occupied);
    wait_for_status(command, |status| status["web_error"].is_null());
    assert_eq!(boundaries(command).len(), 1);
}

#[test]
fn unbindable_control_socket_fails_before_recording_starts() {
    let root = tempfile::tempdir().unwrap();
    // Unix socket paths are limited to 108 bytes.
    let data = root.path().join("d".repeat(110));
    let command = || recorder(&data, unused_address());
    let output = command().arg("run").output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("control socket"));
    assert!(boundaries(command).is_empty());
}

#[test]
fn screenshot_inspection_streams_only_the_requested_retained_identifier() {
    use timeline::history::{History, ScreenshotSample};
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("timeline");
    let mut history = History::open(&dir).unwrap();
    let image = History::prepare_image(&dir, &common::webp(b"encoded fixture")).unwrap();
    let sample = ScreenshotSample {
        unix_ms: 100,
        monotonic_ms: 1,
        segment: 1,
        monitor: "DP-1".into(),
        width: 2,
        height: 1,
        transform: 0,
    };
    let id = history
        .commit_screenshot(&sample, "pixels", &image)
        .unwrap();
    let call = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_timeline"))
            .env("XDG_DATA_HOME", root.path())
            .args(args)
            .output()
            .unwrap()
    };
    let output = call(&["screenshots", "--limit", "1"]);
    assert!(output.status.success());
    let page: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(page["screenshots"][0]["id"], id);
    assert_eq!(page["has_more"], false);
    let output = call(&["image", &id.to_string()]);
    assert!(output.status.success());
    assert_eq!(output.stdout, common::webp(b"encoded fixture"));
    drop(image);
    history.cleanup_before(200).unwrap();
    let output = call(&["image", &id.to_string()]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unavailable or expired"));
}
