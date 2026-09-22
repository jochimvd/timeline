use timeline::recording::{Input, Recorder, Stamp, Window};

fn at(seconds: i64) -> Stamp {
    Stamp {
        unix_ms: 1_000_000 + seconds * 1000,
        monotonic_ms: seconds * 1000,
        boot_ms: seconds * 1000,
    }
}

fn window(title: &str) -> Window {
    Window {
        address: "abc".into(),
        application: "editor".into(),
        title: title.into(),
    }
}

#[test]
fn manual_pause_survives_restart_and_unlock_without_recording_hidden_focus() {
    let directory = tempfile::tempdir().unwrap();
    {
        let mut recorder = Recorder::open(directory.path(), at(0)).unwrap();
        recorder
            .apply(
                Input::Connected {
                    locked: false,
                    idle: false,
                },
                at(1),
            )
            .unwrap();
        recorder
            .apply(Input::Focus(Some(window("visible"))), at(2))
            .unwrap();
        recorder.apply(Input::Pause, at(3)).unwrap();
        recorder
            .apply(Input::Focus(Some(window("secret"))), at(4))
            .unwrap();
    }
    let mut recorder = Recorder::open(directory.path(), at(5)).unwrap();
    recorder
        .apply(
            Input::Connected {
                locked: false,
                idle: false,
            },
            at(6),
        )
        .unwrap();
    recorder
        .apply(Input::Focus(Some(window("still secret"))), at(7))
        .unwrap();
    assert!(recorder.status().manual_paused);
    recorder.apply(Input::Resume, at(8)).unwrap();
    assert!(recorder.status().focus.is_none());
    recorder
        .apply(Input::Focus(Some(window("fresh"))), at(9))
        .unwrap();
    let observations = recorder.history().observations(0, 100).unwrap();
    let text = serde_json::to_string(&observations).unwrap();
    assert!(!text.contains("secret"));
    assert!(text.contains("fresh"));
    assert!(text.contains("restart"));
}

#[test]
fn checkpoints_cover_known_focus_but_never_bridge_lock_or_disconnection() {
    let directory = tempfile::tempdir().unwrap();
    let mut recorder = Recorder::open(directory.path(), at(0)).unwrap();
    recorder
        .apply(Input::Focus(Some(window("unknown lock state"))), at(1))
        .unwrap();
    recorder
        .apply(
            Input::Connected {
                locked: false,
                idle: false,
            },
            at(2),
        )
        .unwrap();
    recorder
        .apply(Input::Focus(Some(window("work"))), at(3))
        .unwrap();
    recorder.apply(Input::Tick, at(8)).unwrap();
    assert_eq!(
        recorder.history().latest().unwrap().unwrap().evidence["kind"],
        "checkpoint"
    );
    recorder.apply(Input::Locked(true), at(9)).unwrap();
    recorder
        .apply(Input::Focus(Some(window("locked secret"))), at(10))
        .unwrap();
    recorder.apply(Input::Locked(false), at(11)).unwrap();
    recorder.apply(Input::Tick, at(16)).unwrap();
    assert!(!recorder.status().focus_known);
    recorder.apply(Input::Focus(None), at(17)).unwrap();
    assert!(recorder.status().focus_known);
    recorder.apply(Input::Disconnected, at(18)).unwrap();
    assert!(!recorder.status().focus_known);
    let text = serde_json::to_string(&recorder.history().observations(0, 100).unwrap()).unwrap();
    assert!(!text.contains("unknown lock state"));
    assert!(!text.contains("locked secret"));
    assert!(text.contains("disconnect"));
}

#[test]
fn clock_jump_suspend_and_stalled_observation_end_focus_in_new_segments() {
    for (stamp, reason) in [
        (
            Stamp {
                unix_ms: 900_000,
                ..at(4)
            },
            "clock_discontinuity",
        ),
        (
            Stamp {
                boot_ms: 40_000,
                ..at(4)
            },
            "suspend",
        ),
        (at(30), "observation_stall"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut recorder = Recorder::open(directory.path(), at(0)).unwrap();
        recorder
            .apply(
                Input::Connected {
                    locked: false,
                    idle: false,
                },
                at(1),
            )
            .unwrap();
        recorder
            .apply(Input::Focus(Some(window("work"))), at(2))
            .unwrap();
        let prior_segment = recorder.history().latest().unwrap().unwrap().segment;
        recorder.apply(Input::Tick, stamp).unwrap();
        let boundary = recorder.history().latest().unwrap().unwrap();
        assert_eq!(boundary.evidence["reason"], reason);
        assert!(boundary.segment > prior_segment);
        assert!(!recorder.status().focus_known);
        assert!(boundary.evidence["previous_durable_observation"].is_i64());
    }
}

#[test]
fn idle_is_an_overlay_and_does_not_end_known_focus() {
    let directory = tempfile::tempdir().unwrap();
    let mut recorder = Recorder::open(directory.path(), at(0)).unwrap();
    recorder
        .apply(
            Input::Connected {
                locked: false,
                idle: false,
            },
            at(1),
        )
        .unwrap();
    recorder
        .apply(Input::Focus(Some(window("meeting"))), at(2))
        .unwrap();
    recorder.apply(Input::Idle(true), at(3)).unwrap();
    recorder.apply(Input::Tick, at(8)).unwrap();
    assert!(recorder.status().idle);
    assert_eq!(recorder.status().focus.as_ref().unwrap().title, "meeting");
    recorder.apply(Input::Idle(false), at(9)).unwrap();
    assert!(!recorder.status().idle);
    assert!(recorder.status().focus_known);
}

#[test]
fn startup_and_hourly_cleanup_remove_old_metadata_without_losing_pause_state() {
    let directory = tempfile::tempdir().unwrap();
    {
        let mut recorder = Recorder::open(directory.path(), at(0)).unwrap();
        recorder.apply(Input::Pause, at(1)).unwrap();
    }
    let thirty_one_days = 31 * 24 * 60 * 60;
    let mut recorder = Recorder::open(directory.path(), at(thirty_one_days)).unwrap();
    assert!(recorder.status().manual_paused);
    assert!(
        recorder
            .history()
            .observations(0, 100)
            .unwrap()
            .iter()
            .all(|o| o.unix_ms >= at(thirty_one_days - 30 * 86400).unix_ms)
    );
    recorder
        .apply(Input::Tick, at(thirty_one_days * 2))
        .unwrap();
    assert!(
        recorder
            .history()
            .observations(0, 100)
            .unwrap()
            .iter()
            .all(|o| o.unix_ms >= at(thirty_one_days * 2 - 30 * 86400).unix_ms)
    );
}

#[test]
fn a_forward_wall_clock_jump_does_not_expire_young_history_even_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let original_id;
    let jumped = Stamp {
        unix_ms: 50 * 86400 * 1000,
        ..at(3601)
    };
    {
        let mut recorder = Recorder::open(directory.path(), at(0)).unwrap();
        original_id = recorder.history().latest().unwrap().unwrap().id;
        recorder.apply(Input::Tick, jumped).unwrap();
        assert!(
            recorder
                .history()
                .observations(0, 100)
                .unwrap()
                .iter()
                .any(|o| o.id == original_id)
        );
    }
    let recorder = Recorder::open(
        directory.path(),
        Stamp {
            unix_ms: jumped.unix_ms + 1000,
            ..at(3602)
        },
    )
    .unwrap();
    assert!(
        recorder
            .history()
            .observations(0, 100)
            .unwrap()
            .iter()
            .any(|o| o.id == original_id)
    );
}

#[test]
fn publication_checks_for_lock_between_each_focus_observation() {
    use timeline::daemon::{now, publish_focus_changes};
    for lock_before in [0, 1] {
        let directory = tempfile::tempdir().unwrap();
        let mut recorder = Recorder::open(directory.path(), now().unwrap()).unwrap();
        recorder
            .apply(
                Input::Connected {
                    locked: false,
                    idle: false,
                },
                now().unwrap(),
            )
            .unwrap();
        let changes = vec![Some(window("first")), Some(window("must not publish"))];
        let mut polls = 0;
        publish_focus_changes(&mut recorder, changes, || {
            let events = if polls == lock_before {
                vec![Input::Locked(true)]
            } else {
                vec![]
            };
            polls += 1;
            Ok(events)
        })
        .unwrap();
        assert!(recorder.status().locked);
        assert!(!recorder.status().focus_known);
        let text =
            serde_json::to_string(&recorder.history().observations(0, 100).unwrap()).unwrap();
        assert!(!text.contains("must not publish"));
        if lock_before == 0 {
            assert!(!text.contains("first"));
        }
    }
}

#[test]
fn a_focus_result_delayed_across_suspend_cannot_reopen_coverage() {
    let directory = tempfile::tempdir().unwrap();
    let mut recorder = Recorder::open(directory.path(), at(0)).unwrap();
    recorder
        .apply(
            Input::Connected {
                locked: false,
                idle: false,
            },
            at(1),
        )
        .unwrap();
    recorder
        .apply(Input::Focus(Some(window("work"))), at(2))
        .unwrap();
    recorder
        .apply(
            Input::Focus(Some(window("delayed before suspend"))),
            Stamp {
                boot_ms: 40_000,
                ..at(4)
            },
        )
        .unwrap();
    assert!(!recorder.status().focus_known);
    let text = serde_json::to_string(&recorder.history().observations(0, 100).unwrap()).unwrap();
    assert!(!text.contains("delayed before suspend"));
}
