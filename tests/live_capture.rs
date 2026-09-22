//! Opt-in live protocol check. The harness must create these two headless outputs
//! on separate unfocused workspaces; the test never requests a physical output.
use std::{
    collections::VecDeque,
    process::Command,
    time::{Duration, Instant},
};
use timeline::{
    capture::{CaptureTrigger, PixelBudget, RecordingImages},
    capture_desktop::DesktopCapture,
    daemon::now,
    recording::{Input, Recorder},
    session::Session,
};
const OUTPUTS: [&str; 2] = ["TIMELINE-TEST-A", "TIMELINE-TEST-B"];
fn check_unfocused() {
    let result = Command::new("hyprctl")
        .args(["-j", "monitors"])
        .output()
        .unwrap();
    assert!(result.status.success());
    let monitors: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    for name in OUTPUTS {
        let monitor = monitors
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["name"] == name)
            .expect("headless test output missing");
        assert_eq!(monitor["focused"], false, "refusing focused output");
        assert!(
            monitor["activeWorkspace"]["name"]
                .as_str()
                .unwrap()
                .starts_with("timeline-test-")
        );
        assert_eq!(monitor["make"], "");
    }
}
#[test]
#[ignore = "requires two explicitly provisioned, unfocused headless workspaces"]
fn captures_both_headless_outputs_through_recording_and_history() {
    check_unfocused();
    let dir = tempfile::tempdir_in("target").unwrap();
    let mut recorder = Recorder::open(dir.path(), now().unwrap()).unwrap();
    let mut session = Session::connect().unwrap();
    assert!(!session.locked());
    recorder
        .apply(
            Input::Connected {
                locked: false,
                idle: session.idle(),
            },
            now().unwrap(),
        )
        .unwrap();
    let mut backend = DesktopCapture::connect(PixelBudget::default()).unwrap();
    let mut worker = RecordingImages::start(dir.path().to_owned()).unwrap();
    let mut ready = VecDeque::new();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(12) {
        check_unfocused();
        for event in session.events().unwrap() {
            recorder.apply(event, now().unwrap()).unwrap();
        }
        assert!(!recorder.status().locked, "lock ended live trial");
        let frames = backend.poll().unwrap();
        recorder
            .set_monitors(
                backend
                    .monitors()
                    .into_iter()
                    .filter(|m| OUTPUTS.contains(&m.name.as_str()))
                    .collect(),
                now().unwrap(),
            )
            .unwrap();
        backend.discard_obsolete(&recorder);
        ready.extend(frames);
        if let Some(done) = worker.poll().unwrap() {
            session.synchronize().unwrap();
            for event in session.events().unwrap() {
                recorder.apply(event, now().unwrap()).unwrap();
            }
            let (hash, image) = done.result.unwrap();
            if let Some(image) = image {
                assert!(
                    recorder
                        .publish_capture(&done.request, &hash, &image, now().unwrap())
                        .unwrap()
                );
            } else {
                assert!(
                    recorder
                        .reuse_capture(&done.request, &hash, now().unwrap())
                        .unwrap()
                );
            }
        }
        if !worker.busy()
            && let Some(frame) = ready.pop_front()
        {
            assert!(
                frame.failure.is_none(),
                "capture failed: {:?}",
                frame.failure
            );
            worker.submit(frame.request, frame.pixels.unwrap()).unwrap();
        }
        for request in recorder
            .request_captures(CaptureTrigger::Timer, now().unwrap())
            .unwrap()
        {
            assert!(OUTPUTS.contains(&request.monitor.name.as_str()));
            if let Some(reason) = backend.request(request.clone()).unwrap() {
                recorder
                    .capture_failed(&request, reason, now().unwrap())
                    .unwrap();
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let evidence = recorder.history().screenshots(0, 100).unwrap();
    for name in OUTPUTS {
        assert!(
            evidence
                .iter()
                .any(|s| s.sample.monitor == name && s.image.is_some()),
            "no completed image for {name}: {evidence:?}"
        );
    }
    println!(
        "headless functional evidence: {}",
        serde_json::to_string(&evidence).unwrap()
    );
    check_unfocused();
}
