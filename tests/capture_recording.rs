use timeline::{
    capture::{CaptureTrigger, Monitor},
    recording::{Input, Recorder, Stamp},
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
        let prepared = History::prepare_image(dir.path(), b"encoded").unwrap();
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
