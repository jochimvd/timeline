use std::process::Command;
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

#[test]
fn daemon_pause_is_durable_after_forced_exit_and_duplicate_recorder_is_rejected() {
    use std::{
        process::{Child, Stdio},
        thread,
        time::{Duration, Instant},
    };
    struct Running(Child);
    impl Drop for Running {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let root = tempfile::tempdir().unwrap();
    let command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_timeline"));
        command
            .env("XDG_DATA_HOME", root.path())
            .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
            .env_remove("XDG_RUNTIME_DIR");
        command
    };
    let start = || {
        let child = Running(
            command()
                .arg("run")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            let output = command().arg("status").output().unwrap();
            if output.status.success() {
                break;
            }
            assert!(Instant::now() < deadline, "recorder did not start");
            thread::sleep(Duration::from_millis(20));
        }
        child
    };
    let child = start();
    assert!(command().arg("pause").output().unwrap().status.success());
    let second = command().arg("run").output().unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("another recorder"));
    drop(child); // SIGKILL, intentionally bypassing orderly shutdown.
    let _restarted = start();
    let output = command().arg("status").output().unwrap();
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["recording"]["manual_paused"], true);
    assert_eq!(status["recording"]["focus_known"], false);
    assert_eq!(status["recording"]["screenshot_recording"], false);
    assert!(command().arg("resume").output().unwrap().status.success());
    let output = command().arg("observations").output().unwrap();
    let page: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        page["observations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["evidence"]["reason"] == "restart")
    );
}

#[test]
fn screenshot_inspection_streams_only_the_requested_retained_identifier() {
    use timeline::history::{History, ScreenshotSample};
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("timeline");
    let mut history = History::open(&dir).unwrap();
    let image = History::prepare_image(&dir, b"encoded fixture").unwrap();
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
    assert_eq!(output.stdout, b"encoded fixture");
    drop(image);
    history.cleanup_before(200).unwrap();
    let output = call(&["image", &id.to_string()]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unavailable or expired"));
}
