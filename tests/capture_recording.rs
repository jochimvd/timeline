mod common;
use timeline::{
    capture::{CaptureTrigger, Monitor},
    recording::{Input, Recorder, Stamp, Window},
};
fn at(s: i64) -> Stamp {
    Stamp {
        unix_ms: 1_000_000 + s * 1000,
        monotonic_ms: s * 1000,
        boot_ms: s * 1000,
    }
}
fn monitor(name: &str) -> Monitor {
    Monitor {
        name: name.into(),
        width: 2,
        height: 1,
        revision: 1,
    }
}
#[test]
fn pending_monitor_is_missing_evidence_while_other_monitors_and_metadata_continue() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = Recorder::open(dir.path(), at(0)).unwrap();
    r.apply(
        Input::Connected {
            locked: false,
            idle: false,
        },
        at(0),
    )
    .unwrap();
    r.set_monitors(vec![monitor("DP-1"), monitor("DP-2")], at(0))
        .unwrap();
    let first = r.request_captures(CaptureTrigger::Timer, at(0)).unwrap();
    assert_eq!(first.len(), 2);
    r.capture_failed(&first[1], "capture_failed", at(1))
        .unwrap();
    r.apply(Input::Focus(None), at(2)).unwrap();
    let next = r.request_captures(CaptureTrigger::Timer, at(5)).unwrap();
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].monitor.name, "DP-2");
    assert!(r.status().focus_known);
    let evidence = r.history().screenshots(0, 100).unwrap();
    assert!(
        evidence
            .iter()
            .any(|s| s.sample.monitor == "DP-1"
                && s.unavailable.as_deref() == Some("capture_pending"))
    );
    assert!(evidence.iter().all(|s| s.image.is_none()));
}

#[test]
fn delayed_encoded_images_never_cross_recording_or_monitor_boundaries() {
    use timeline::history::History;
    for boundary in 0..6 {
        let dir = tempfile::tempdir().unwrap();
        let mut r = Recorder::open(dir.path(), at(0)).unwrap();
        r.apply(
            Input::Connected {
                locked: false,
                idle: false,
            },
            at(0),
        )
        .unwrap();
        r.set_monitors(vec![monitor("DP-1"), monitor("DP-2")], at(0))
            .unwrap();
        let requests = r.request_captures(CaptureTrigger::Timer, at(0)).unwrap();
        let prepared = History::prepare_image(dir.path(), &common::webp(b"encoded")).unwrap();
        match boundary {
            0 => r.apply(Input::Pause, at(1)).unwrap(),
            1 => r.apply(Input::Locked(true), at(1)).unwrap(),
            2 => r.apply(Input::Disconnected, at(1)).unwrap(),
            3 => r
                .apply(
                    Input::Tick,
                    Stamp {
                        boot_ms: 5000,
                        ..at(1)
                    },
                )
                .unwrap(),
            4 => r
                .apply(
                    Input::Tick,
                    Stamp {
                        unix_ms: 3_000_000,
                        ..at(1)
                    },
                )
                .unwrap(),
            _ => r.set_monitors(vec![monitor("DP-2")], at(1)).unwrap(),
        }
        // Use the same clock as the boundary to avoid an unrelated reversal.
        let stamp = match boundary {
            3 => Stamp {
                boot_ms: 5000,
                ..at(1)
            },
            4 => Stamp {
                unix_ms: 3_000_000,
                ..at(1)
            },
            _ => at(1),
        };
        assert!(
            !r.publish_capture(&requests[0], "pixels", &prepared, stamp)
                .unwrap()
        );
        if boundary == 5 {
            assert!(
                r.publish_capture(&requests[1], "pixels", &prepared, stamp)
                    .unwrap()
            );
        } else {
            assert!(
                !r.publish_capture(&requests[1], "pixels", &prepared, stamp)
                    .unwrap()
            );
        }
    }
}

#[test]
fn pressure_cleans_expired_evidence_then_uses_strict_hysteresis_and_fresh_generations() {
    use timeline::config::{PRESSURE_RESUME_BYTES as HIGH, PRESSURE_STOP_BYTES as LOW};
    let dir = tempfile::tempdir().unwrap();
    let mut r = Recorder::open(dir.path(), at(0)).unwrap();
    r.apply(
        Input::Connected {
            locked: false,
            idle: false,
        },
        at(0),
    )
    .unwrap();
    r.set_monitors(vec![monitor("DP-1")], at(0)).unwrap();
    let old = r
        .request_captures(CaptureTrigger::Timer, at(0))
        .unwrap()
        .remove(0);
    r.check_storage_pressure(at(1), || Ok(LOW)).unwrap();
    assert!(r.status().screenshot_recording);
    r.check_storage_pressure(at(2), || Ok(LOW - 1)).unwrap();
    assert!(r.status().screenshot_pressure);
    r.apply(Input::Focus(None), at(2)).unwrap();
    assert!(r.status().focus_known);
    for bytes in [LOW, HIGH] {
        r.check_storage_pressure(at(3), || Ok(bytes)).unwrap();
        assert!(r.status().screenshot_pressure);
    }
    r.check_storage_pressure(at(4), || Ok(HIGH + 1)).unwrap();
    assert!(r.status().screenshot_recording);
    assert!(!r.capture_valid(&old));
    assert_eq!(
        r.request_captures(CaptureTrigger::Timer, at(4))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn completed_identical_pixels_reuse_a_webp_but_changed_pixels_encode_new_evidence() {
    use std::{
        io::Read,
        time::{Duration, Instant},
    };
    use timeline::capture::{PixelBudget, RecordingImages};
    let dir = tempfile::tempdir().unwrap();
    let mut r = Recorder::open(dir.path(), at(0)).unwrap();
    r.apply(
        Input::Connected {
            locked: false,
            idle: false,
        },
        at(0),
    )
    .unwrap();
    r.set_monitors(vec![monitor("DP-1")], at(0)).unwrap();
    let budget = PixelBudget::default();
    let mut worker =
        RecordingImages::with_space_probe(dir.path().to_owned(), || Ok(100 * 1024 * 1024 * 1024))
            .unwrap();
    for (time, color, duplicate) in [(0, 40, false), (5, 40, true), (10, 200, false)] {
        let request = r
            .request_captures(CaptureTrigger::Timer, at(time))
            .unwrap()
            .remove(0);
        let mut pixels = budget.allocate(2, 1).unwrap().unwrap();
        pixels
            .bytes_mut()
            .copy_from_slice(&[color, 10, 20, 0, color, 10, 20, 123]);
        worker.submit(request, pixels).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let completed = loop {
            if let Some(result) = worker.poll().unwrap() {
                break result;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        };
        let (fingerprint, image) = completed.result.unwrap();
        assert_eq!(image.is_none(), duplicate);
        if let Some(image) = image {
            assert!(
                r.publish_capture(&completed.request, &fingerprint, &image, at(time))
                    .unwrap()
            );
        } else {
            assert!(
                r.reuse_capture(&completed.request, &fingerprint, at(time))
                    .unwrap()
            );
        }
    }
    let records = r.history().screenshots(0, 100).unwrap();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].image, records[1].image);
    assert_ne!(records[1].image, records[2].image);
    let mut webp = Vec::new();
    r.history()
        .image(records[0].id)
        .unwrap()
        .unwrap()
        .read_to_end(&mut webp)
        .unwrap();
    assert_eq!(&webp[0..4], b"RIFF");
    assert_eq!(&webp[8..12], b"WEBP");
}

#[test]
fn raw_pixels_are_globally_bounded_and_capacity_returns_after_cancellation() {
    use timeline::capture::PixelBudget;
    let budget = PixelBudget::default();
    let a = budget.allocate(3840, 2160).unwrap().unwrap();
    let b = budget.clone().allocate(3840, 2160).unwrap().unwrap();
    let c = budget.allocate(3840, 2160).unwrap().unwrap();
    assert!(budget.allocate(3840, 2160).unwrap().is_none());
    drop(b);
    assert!(budget.allocate(3840, 2160).unwrap().is_some());
    drop(a);
    drop(c);
    assert!(budget.allocate(0, 2160).is_err());
}

#[test]
fn pressure_survives_restart_inside_the_hysteresis_band() {
    use timeline::config::{PRESSURE_RESUME_BYTES as HIGH, PRESSURE_STOP_BYTES as LOW};
    let dir = tempfile::tempdir().unwrap();
    {
        let mut r = Recorder::open(dir.path(), at(0)).unwrap();
        r.check_storage_pressure(at(1), || Ok(LOW - 1)).unwrap();
        assert!(r.status().screenshot_pressure);
    }
    let mut r = Recorder::open(dir.path(), at(2)).unwrap();
    r.check_storage_pressure(at(2), || Ok(HIGH)).unwrap();
    assert!(r.status().screenshot_pressure);
    r.check_storage_pressure(at(3), || Ok(HIGH + 1)).unwrap();
    assert!(!r.status().screenshot_pressure);
}

#[test]
fn background_reclamation_does_not_block_metadata_and_pressure_waits_for_cleanup() {
    use std::time::{Duration, Instant};
    use timeline::config::PRESSURE_STOP_BYTES as LOW;
    let dir = tempfile::tempdir().unwrap();
    let mut r = Recorder::open(dir.path(), at(0)).unwrap();
    r.enable_background_cleanup().unwrap();
    r.apply(
        Input::Connected {
            locked: false,
            idle: false,
        },
        at(0),
    )
    .unwrap();
    r.set_monitors(vec![monitor("DP-1")], at(0)).unwrap();
    // Real orphan files plus a real active-read lease hold cleanup open.
    for index in 0..1000 {
        std::fs::write(
            dir.path().join(format!("tiles/pack-orphan-{index}.part")),
            b"interrupted",
        )
        .unwrap();
    }
    let lease = std::fs::File::open(dir.path().join("images.lock")).unwrap();
    lease.lock_shared().unwrap();
    r.check_storage_pressure(at(1), || Ok(LOW - 1)).unwrap();
    assert!(r.status().screenshot_cleanup);
    assert!(!r.status().screenshot_recording);
    r.apply(Input::Focus(None), at(2)).unwrap();
    assert!(r.status().focus_known);
    assert_eq!(
        r.history().latest().unwrap().unwrap().evidence["kind"],
        "focus"
    );
    lease.unlock().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    // Cleanup recovers enough headroom: don't incorrectly demand 25 GiB when
    // pressure wasn't entered before the cleanup attempt finished.
    loop {
        r.check_storage_pressure(at(3), || Ok(LOW + 1)).unwrap();
        if !r.status().screenshot_cleanup {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!r.status().screenshot_pressure);
    assert!(r.status().screenshot_recording);
    assert_eq!(common::payload_files(dir.path()).len(), 0);
}

#[test]
fn monitors_rotate_admission_and_reconnect_requires_a_fresh_request() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = Recorder::open(dir.path(), at(0)).unwrap();
    r.apply(
        Input::Connected {
            locked: false,
            idle: false,
        },
        at(0),
    )
    .unwrap();
    r.set_monitors(vec![monitor("DP-1"), monitor("DP-2")], at(0))
        .unwrap();
    let old = r.request_captures(CaptureTrigger::Timer, at(0)).unwrap();
    assert_eq!(old[0].monitor.name, "DP-1");
    for request in &old {
        r.capture_failed(request, "raw_pixel_budget", at(1))
            .unwrap();
    }
    let next = r.request_captures(CaptureTrigger::Timer, at(5)).unwrap();
    assert_eq!(next[0].monitor.name, "DP-2");
    let mut resized = monitor("DP-1");
    resized.width = 4;
    resized.revision = 2;
    r.set_monitors(vec![resized.clone(), monitor("DP-2")], at(6))
        .unwrap();
    assert!(r.capture_valid(&next[0]));
    assert!(!r.capture_valid(&next[1]));
    r.set_monitors(vec![monitor("DP-2")], at(7)).unwrap();
    r.set_monitors(vec![resized, monitor("DP-2")], at(8))
        .unwrap();
    assert!(!r.capture_valid(&next[1]));
    let fresh = r.request_captures(CaptureTrigger::Timer, at(10)).unwrap();
    assert_eq!(fresh.len(), 1);
    assert_eq!(fresh[0].monitor.width, 4);
    assert!(r.capture_valid(&fresh[0]));
}

fn at_ms(ms: i64) -> Stamp {
    Stamp {
        unix_ms: 1_000_000 + ms,
        monotonic_ms: ms,
        boot_ms: ms,
    }
}
fn window(address: &str, title: &str) -> Input {
    Input::Focus(Some(Window {
        address: address.into(),
        application: "app".into(),
        title: title.into(),
    }))
}
/// Connected on `monitors` with focus on window `a` and the first timer
/// capture requested at 0; pending captures are released when `release`.
fn switching(dir: &std::path::Path, monitors: &[&str], release: bool) -> Recorder {
    let mut r = Recorder::open(dir, at(0)).unwrap();
    r.apply(
        Input::Connected {
            locked: false,
            idle: false,
        },
        at(0),
    )
    .unwrap();
    r.apply(window("a", "one"), at(0)).unwrap();
    r.set_monitors(monitors.iter().map(|m| monitor(m)).collect(), at(0))
        .unwrap();
    let first = r.request_captures(CaptureTrigger::Timer, at(0)).unwrap();
    assert_eq!(first.len(), monitors.len());
    if release {
        for request in &first {
            r.capture_failed(request, "capture_failed", at_ms(100))
                .unwrap();
        }
    }
    r
}
fn trigger_gaps(r: &Recorder) -> usize {
    r.history()
        .screenshots(0, 100)
        .unwrap()
        .iter()
        .filter(|s| {
            matches!(
                s.unavailable.as_deref(),
                Some("trigger_overdue" | "capture_pending")
            )
        })
        .count()
}

#[test]
fn settled_switch_captures_and_restarts_the_timer() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = switching(dir.path(), &["DP-1"], true);
    r.apply(window("b", "two"), at(2)).unwrap();
    assert!(
        r.request_captures(CaptureTrigger::Switch, at_ms(2_999))
            .unwrap()
            .is_empty()
    );
    let switch = r.request_captures(CaptureTrigger::Switch, at(3)).unwrap();
    assert_eq!(switch.len(), 1);
    assert_eq!(switch[0].sample.monotonic_ms, 3_000);
    r.capture_failed(&switch[0], "capture_failed", at_ms(3_100))
        .unwrap();
    assert!(
        r.request_captures(CaptureTrigger::Timer, at(5))
            .unwrap()
            .is_empty()
    );
    assert!(
        r.request_captures(CaptureTrigger::Switch, at(6))
            .unwrap()
            .is_empty(),
        "a switch is captured once"
    );
    assert_eq!(
        r.request_captures(CaptureTrigger::Timer, at(8))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn burst_of_switches_captures_once_after_the_last_settles() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = switching(dir.path(), &["DP-1"], true);
    for (address, ms) in [("b", 2_000), ("c", 2_400), ("d", 2_800)] {
        r.apply(window(address, "title"), at_ms(ms)).unwrap();
        assert!(
            r.request_captures(CaptureTrigger::Switch, at_ms(ms + 300))
                .unwrap()
                .is_empty()
        );
    }
    assert!(
        r.request_captures(CaptureTrigger::Switch, at_ms(3_799))
            .unwrap()
            .is_empty()
    );
    let switch = r
        .request_captures(CaptureTrigger::Switch, at_ms(3_800))
        .unwrap();
    assert_eq!(switch.len(), 1);
    assert_eq!(r.status().focus.as_ref().unwrap().address, "d");
    r.capture_failed(&switch[0], "capture_failed", at(4))
        .unwrap();
    assert!(
        r.request_captures(CaptureTrigger::Switch, at(5))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn switch_inside_the_minimum_gap_is_deferred_and_restarts_the_timer() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = switching(dir.path(), &["DP-1"], true);
    r.apply(window("b", "two"), at_ms(4_500)).unwrap();
    let timer = r.request_captures(CaptureTrigger::Timer, at(5)).unwrap();
    assert_eq!(timer.len(), 1, "the timer fires before the switch settles");
    r.capture_failed(&timer[0], "capture_failed", at_ms(5_100))
        .unwrap();
    assert!(
        r.request_captures(CaptureTrigger::Switch, at_ms(5_999))
            .unwrap()
            .is_empty()
    );
    let switch = r.request_captures(CaptureTrigger::Switch, at(6)).unwrap();
    assert_eq!(switch.len(), 1, "the settled switch waits out the gap");
    r.capture_failed(&switch[0], "capture_failed", at_ms(6_100))
        .unwrap();
    assert!(
        r.request_captures(CaptureTrigger::Timer, at_ms(10_999))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        r.request_captures(CaptureTrigger::Timer, at(11))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn switch_blocked_by_a_busy_monitor_is_covered_by_the_next_capture() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = Recorder::open(dir.path(), at(0)).unwrap();
    r.apply(
        Input::Connected {
            locked: false,
            idle: false,
        },
        at(0),
    )
    .unwrap();
    r.apply(window("a", "one"), at(0)).unwrap();
    r.set_monitors(vec![monitor("DP-1")], at(0)).unwrap();
    let first = r.request_captures(CaptureTrigger::Timer, at(0)).unwrap();
    r.apply(window("b", "two"), at(2)).unwrap();
    assert!(
        r.request_captures(CaptureTrigger::Switch, at(3))
            .unwrap()
            .is_empty()
    );
    r.capture_failed(&first[0], "capture_failed", at_ms(4_900))
        .unwrap();
    let timer = r.request_captures(CaptureTrigger::Timer, at(5)).unwrap();
    assert_eq!(timer.len(), 1);
    r.capture_failed(&timer[0], "capture_failed", at_ms(5_100))
        .unwrap();
    assert!(
        r.request_captures(CaptureTrigger::Switch, at(7))
            .unwrap()
            .is_empty(),
        "the timer captured the settled window"
    );
}

#[test]
fn title_changes_and_the_first_focus_after_connecting_do_not_trigger() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = switching(dir.path(), &["DP-1"], true);
    r.apply(window("a", "renamed"), at(1)).unwrap();
    r.apply(window("a", "again"), at(2)).unwrap();
    assert!(
        r.request_captures(CaptureTrigger::Switch, at(4))
            .unwrap()
            .is_empty()
    );
    r.apply(Input::Locked(true), at(4)).unwrap();
    r.apply(Input::Locked(false), at(4)).unwrap();
    r.apply(window("b", "two"), at(4)).unwrap();
    assert!(
        r.request_captures(CaptureTrigger::Switch, at(6))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn switches_skip_pending_monitors_and_never_record_trigger_gaps() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = switching(dir.path(), &["DP-1"], false);
    r.apply(window("b", "two"), at(2)).unwrap();
    assert!(
        r.request_captures(CaptureTrigger::Switch, at(3))
            .unwrap()
            .is_empty(),
        "nothing to capture defers the switch and leaves the timer alone"
    );
    assert_eq!(
        r.request_captures(CaptureTrigger::Timer, at(5))
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        trigger_gaps(&r),
        1,
        "only the timer reports a pending monitor"
    );

    // Release one monitor: the switch captures only the other, silently.
    let dir = tempfile::tempdir().unwrap();
    let mut r = Recorder::open(dir.path(), at(0)).unwrap();
    r.apply(
        Input::Connected {
            locked: false,
            idle: false,
        },
        at(0),
    )
    .unwrap();
    r.apply(window("a", "one"), at(0)).unwrap();
    r.set_monitors(vec![monitor("DP-1"), monitor("DP-2")], at(0))
        .unwrap();
    let first = r.request_captures(CaptureTrigger::Timer, at(0)).unwrap();
    let released = first.iter().find(|q| q.monitor.name == "DP-2").unwrap();
    r.capture_failed(released, "capture_failed", at_ms(100))
        .unwrap();
    r.apply(window("b", "two"), at(2)).unwrap();
    let switch = r.request_captures(CaptureTrigger::Switch, at(3)).unwrap();
    assert_eq!(switch.len(), 1);
    assert_eq!(switch[0].monitor.name, "DP-2");
    assert_eq!(trigger_gaps(&r), 0);
    // A switch long after the last capture does not report the timer overdue.
    for request in [&first[0], &switch[0]] {
        r.capture_failed(request, "capture_failed", at(8)).unwrap();
    }
    let before = trigger_gaps(&r);
    r.apply(window("c", "three"), at(17)).unwrap();
    assert_eq!(
        r.request_captures(CaptureTrigger::Switch, at(18))
            .unwrap()
            .len(),
        2
    );
    assert_eq!(trigger_gaps(&r), before);
}
