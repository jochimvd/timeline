mod common;
use chrono::NaiveDate;
use timeline::{
    evidence::{BoundaryKind, Evidence, FocusState, StateChange},
    history::{History, ScreenshotSample},
    queries::{Client::*, *},
    recording::Window,
};

fn range(start_ms: i64, end_ms: i64) -> Range {
    Range { start_ms, end_ms }
}
fn state(title: Option<&str>, idle: bool) -> FocusState {
    FocusState {
        manual_paused: false,
        connected: true,
        locked: false,
        idle,
        focus: title.map(|title| Window {
            address: "window".into(),
            application: "editor".into(),
            title: title.into(),
        }),
        focus_known: true,
        screenshot_recording: true,
        screenshot_pressure: false,
        screenshot_cleanup: false,
    }
}
fn put(
    h: &mut History,
    wall: i64,
    mono: i64,
    segment: i64,
    kind: StateChange,
    state: FocusState,
) -> i64 {
    h.commit_evidence(wall, mono, segment, &Evidence::State { kind, state }, None)
        .unwrap()
}
fn boundary(h: &mut History, wall: i64, mono: i64, segment: i64) {
    h.commit_evidence(
        wall,
        mono,
        segment,
        &Evidence::Boundary {
            kind: BoundaryKind::Boundary,
            reason: "clock_discontinuity".into(),
            previous_durable_observation: None,
            previous_durable_unix_ms: None,
        },
        None,
    )
    .unwrap();
}
fn page(limit: usize, cursor: Option<String>) -> PageRequest {
    PageRequest {
        limit: Some(limit),
        cursor,
    }
}
fn sample(wall: i64, monitor: &str) -> ScreenshotSample {
    ScreenshotSample {
        unix_ms: wall,
        monotonic_ms: wall,
        segment: 1,
        monitor: monitor.into(),
        width: 2,
        height: 1,
        transform: 0,
    }
}
fn assert_error<T>(result: anyhow::Result<T>, expected: QueryError) {
    match result {
        Err(error) => assert_eq!(
            error.downcast_ref::<QueryError>(),
            Some(&expected),
            "{error:#}"
        ),
        Ok(_) => panic!("expected {expected}"),
    }
}

#[test]
fn grouping_is_replaceable_but_exact_totals_idle_and_originals_are_not() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    let first = put(&mut h, 0, 0, 1, StateChange::Focus, state(Some("A"), false));
    put(&mut h, 10, 10, 1, StateChange::Idle, state(Some("A"), true));
    put(
        &mut h,
        20,
        20,
        1,
        StateChange::Focus,
        state(Some("B"), true),
    );
    put(
        &mut h,
        28,
        28,
        1,
        StateChange::Focus,
        state(Some("A"), false),
    );
    put(&mut h, 40, 40, 1, StateChange::Focus, state(None, false));
    put(
        &mut h,
        50,
        50,
        1,
        StateChange::Checkpoint,
        state(None, false),
    );
    let q = Queries::open(dir.path()).unwrap();
    let blocks = q
        .blocks(Browser, range(5, 50), page(100, None))
        .unwrap()
        .items;
    assert_eq!(blocks.len(), 3);
    assert_eq!((blocks[0].elapsed_ms, blocks[0].idle_ms), (15, 10));
    assert_eq!(blocks[0].first_observation, first);
    let clipped = q
        .blocks(Browser, range(12, 18), page(100, None))
        .unwrap()
        .items;
    assert_eq!(clipped[0].id, blocks[0].id);
    assert_eq!(clipped[0].elapsed_ms, 6);
    struct ByApplication;
    impl Grouping for ByApplication {
        fn version(&self) -> &str {
            "application-v1"
        }
        fn key(&self, window: &Window) -> GroupKey {
            GroupKey {
                application: window.application.clone(),
                title: "all titles".into(),
            }
        }
    }
    let alternative =
        Queries::with_policy(dir.path(), "Europe/Oslo", Box::new(ByApplication)).unwrap();
    assert_eq!(
        alternative
            .blocks(Browser, range(5, 50), page(100, None))
            .unwrap()
            .items
            .len(),
        1
    );
    let overview = alternative
        .overview(Mcp, Selection::Range(range(5, 60)), page(100, None))
        .unwrap();
    assert_eq!(
        (
            overview.totals.focus_ms,
            overview.totals.no_focus_ms,
            overview.totals.idle_ms
        ),
        (35, 10, 18)
    );
    let titles: Vec<_> = overview
        .breakdown
        .items
        .iter()
        .filter_map(|r| match r {
            Breakdown::ExactTitle {
                title, elapsed_ms, ..
            } => Some((title.as_str(), *elapsed_ms)),
            _ => None,
        })
        .collect();
    assert_eq!(titles, [("A", 27), ("B", 8)]);
    assert!(overview.breakdown.items.iter().any(|r| matches!(
        r,
        Breakdown::FocusCoverage(Coverage {
            focus: FocusCoverage::Unknown,
            ..
        })
    )));
    let observations = alternative
        .observations(Mcp, range(0, 50), page(100, None))
        .unwrap();
    assert_eq!(observations.items.len(), 5);
    assert_eq!(observations.items[0].id, first);
}

#[test]
fn title_rules_merge_cleaned_titles_but_observations_stay_raw() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    for (wall, title) in [(0, "⠋ Review"), (10, "⠙ Review"), (20, "◑ Review")] {
        put(
            &mut h,
            wall,
            wall,
            1,
            StateChange::Focus,
            state(Some(title), false),
        );
    }
    put(&mut h, 30, 30, 1, StateChange::Focus, state(None, false));
    let rules = timeline::titles::TitleRules::parse(
        "[[title_rules]]\napplication = 'editor'\npattern = '^[\\x{2800}-\\x{28FF}◑]'",
    )
    .unwrap();
    let raw = Queries::open(dir.path()).unwrap();
    let cleaned = Queries::open(dir.path()).unwrap().with_titles(rules.into());
    let blocks = |q: &Queries| {
        q.blocks(Browser, range(0, 30), page(100, None))
            .unwrap()
            .items
    };
    let (raw_blocks, cleaned_blocks) = (blocks(&raw), blocks(&cleaned));
    assert_eq!(raw_blocks.len(), 3);
    assert_eq!(cleaned_blocks.len(), 1);
    assert_eq!(cleaned_blocks[0].group.title, "Review");
    assert_eq!(cleaned_blocks[0].elapsed_ms, 30);
    assert_ne!(cleaned_blocks[0].id, raw_blocks[0].id);
    let observations = cleaned
        .observations(Browser, range(0, 30), page(100, None))
        .unwrap();
    assert!(matches!(
        &observations.items[0].evidence,
        Evidence::State { state, .. } if state.focus.as_ref().unwrap().title == "⠋ Review"
    ));
}

#[test]
fn activity_days_follow_oslo_dst_and_half_open_elapsed_clips_are_additive() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    let q = Queries::with_policy(dir.path(), "Europe/Oslo", Box::new(ExactTitle)).unwrap();
    for (date, hours) in [("2026-03-28", 23), ("2026-10-24", 25)] {
        let resolved = q
            .resolve(Selection::ActivityDate(
                NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
            ))
            .unwrap();
        assert_eq!(
            resolved.range.end_ms - resolved.range.start_ms,
            hours * 3_600_000
        );
        assert_eq!(resolved.timezone, "Europe/Oslo");
    }
    put(&mut h, 0, 0, 1, StateChange::Focus, state(Some("A"), false));
    put(
        &mut h,
        7,
        10,
        1,
        StateChange::Checkpoint,
        state(Some("A"), false),
    );
    let duration = |r| {
        q.blocks(Browser, r, page(100, None))
            .unwrap()
            .items
            .iter()
            .map(|b| b.elapsed_ms)
            .sum::<i64>()
    };
    assert_eq!(duration(range(0, 3)) + duration(range(3, 7)), 10);
    assert_eq!(duration(range(3, 7)), 6);
    assert_error(
        q.observations(Browser, range(0, 48 * 3_600_000 + 1), page(100, None)),
        QueryError::InvalidRequest,
    );
    assert_error(
        q.overview(
            Browser,
            Selection::Range(range(0, 31 * 24 * 3_600_000 + 1)),
            page(100, None),
        ),
        QueryError::InvalidRequest,
    );
    assert_error(
        q.blocks(Browser, range(i64::MIN, i64::MAX), page(100, None)),
        QueryError::InvalidRequest,
    );
}

#[test]
fn repeated_wall_times_and_zero_length_boundaries_never_merge_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    put(
        &mut h,
        100,
        0,
        1,
        StateChange::Focus,
        state(Some("A"), false),
    );
    put(
        &mut h,
        110,
        10,
        1,
        StateChange::Checkpoint,
        state(Some("A"), false),
    );
    boundary(&mut h, 100, 20, 2);
    put(
        &mut h,
        100,
        20,
        2,
        StateChange::Focus,
        state(Some("A"), false),
    );
    put(
        &mut h,
        110,
        30,
        2,
        StateChange::Checkpoint,
        state(Some("A"), false),
    );
    let mut paused = state(None, false);
    paused.manual_paused = true;
    paused.focus_known = false;
    put(&mut h, 110, 30, 2, StateChange::Pause, paused);
    put(&mut h, 110, 30, 2, StateChange::Resume, state(None, false));
    put(
        &mut h,
        110,
        30,
        2,
        StateChange::Focus,
        state(Some("A"), false),
    );
    put(
        &mut h,
        120,
        40,
        2,
        StateChange::Checkpoint,
        state(Some("A"), false),
    );
    let q = Queries::open(dir.path()).unwrap();
    let blocks = q
        .blocks(Browser, range(100, 120), page(100, None))
        .unwrap()
        .items;
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks.iter().map(|b| b.elapsed_ms).sum::<i64>(), 30);
    let selected = q.cursor_evidence(Browser, 105, None).unwrap();
    assert_eq!(selected.focus.len(), 2);
    assert_ne!(selected.focus[0].segment, selected.focus[1].segment);
}

#[test]
fn screenshot_selection_discloses_per_monitor_gaps_staleness_and_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    put(&mut h, 0, 0, 1, StateChange::Focus, state(Some("A"), false));
    let image = History::prepare_image(dir.path(), &common::webp(b"exact image bytes")).unwrap();
    let a = h
        .commit_screenshot(&sample(1, "A"), "pixels", &image)
        .unwrap();
    drop(image);
    let image = History::prepare_image(dir.path(), &common::webp(b"monitor B")).unwrap();
    h.commit_screenshot(&sample(2, "B"), "other pixels", &image)
        .unwrap();
    drop(image);
    h.screenshot_gap(&sample(5, "B"), "capture_pending")
        .unwrap();
    put(
        &mut h,
        6000,
        6000,
        1,
        StateChange::Checkpoint,
        state(Some("A"), false),
    );
    let mut paused = state(None, false);
    paused.manual_paused = true;
    paused.focus_known = false;
    put(&mut h, 7000, 7000, 1, StateChange::Pause, paused);
    put(
        &mut h,
        8000,
        8000,
        1,
        StateChange::Resume,
        state(None, false),
    );
    put(
        &mut h,
        8001,
        8001,
        1,
        StateChange::Focus,
        state(Some("A"), false),
    );
    put(
        &mut h,
        9000,
        9000,
        1,
        StateChange::Checkpoint,
        state(Some("A"), false),
    );
    let q = Queries::open(dir.path()).unwrap();
    let selected = q.cursor_evidence(Browser, 6000, None).unwrap();
    assert_eq!(selected.screenshots.len(), 2);
    assert_eq!(selected.screenshots[0].screenshot.id, a);
    assert!(selected.screenshots[0].stale);
    assert_eq!(
        selected.screenshots[1].screenshot.unavailable.as_deref(),
        Some("capture_pending")
    );
    assert!(matches!(
        selected.focus[0].focus,
        FocusCoverage::Focus { .. }
    ));
    assert!(
        q.cursor_evidence(Mcp, 7000, None)
            .unwrap()
            .screenshots
            .is_empty()
    );
    assert!(
        q.cursor_evidence(Browser, 8500, None)
            .unwrap()
            .screenshots
            .is_empty()
    );
    assert_eq!(
        q.screenshots(Mcp, range(0, 10), Some("A"), page(100, None))
            .unwrap()
            .items
            .len(),
        1
    );
    let mut bytes = vec![];
    std::io::Read::read_to_end(&mut q.image(Mcp, a).unwrap().stream, &mut bytes).unwrap();
    assert_eq!(bytes, common::webp(b"exact image bytes"));
}

#[test]
fn pagination_pins_appends_and_retention_invalidates_continuations() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    for time in 0..6 {
        put(
            &mut h,
            time,
            time,
            1,
            StateChange::Focus,
            state(Some(if time % 2 == 0 { "A" } else { "B" }), false),
        );
    }
    let q = Queries::open(dir.path()).unwrap();
    let first = q.observations(Mcp, range(0, 20), page(2, None)).unwrap();
    let token = first.next_cursor.clone().unwrap();
    put(
        &mut h,
        6,
        6,
        1,
        StateChange::Focus,
        state(Some("new"), false),
    );
    let mut ids: Vec<_> = first.items.iter().map(|o| o.id).collect();
    let mut next = first.next_cursor;
    while next.is_some() {
        let result = q.observations(Mcp, range(0, 20), page(2, next)).unwrap();
        ids.extend(result.items.iter().map(|o| o.id));
        next = result.next_cursor;
    }
    assert_eq!(ids.len(), 6);
    assert_error(
        q.observations(Mcp, range(0, 19), page(2, Some(token.clone()))),
        QueryError::InvalidRequest,
    );
    h.cleanup_before(3).unwrap();
    assert_error(
        q.observations(Mcp, range(0, 20), page(2, Some(token))),
        QueryError::CursorInvalidated,
    );
    assert_error(
        q.observations(Mcp, range(0, 20), page(501, None)),
        QueryError::InvalidRequest,
    );
    assert_error(
        q.observations(Mcp, range(0, 20), page(2, Some("not a cursor".into()))),
        QueryError::InvalidRequest,
    );
}

#[test]
fn image_streams_share_global_and_mcp_admission_and_survive_concurrent_retention() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    let prepared = History::prepare_image(dir.path(), &common::webp(b"retained bytes")).unwrap();
    let id = h
        .commit_screenshot(&sample(1, "A"), "pixels", &prepared)
        .unwrap();
    drop(prepared);
    let q = Queries::open(dir.path()).unwrap();
    let mut mcp = q.image(Mcp, id).unwrap();
    assert_error(
        q.observations(Mcp, range(0, 2), page(100, None)),
        QueryError::Busy,
    );
    let browser = q.image(Browser, id).unwrap();
    assert_error(
        q.overview(Browser, Selection::Range(range(0, 2)), page(100, None)),
        QueryError::Busy,
    );
    // A second service connection and legacy image reads share the same slots.
    let another = Queries::open(dir.path()).unwrap();
    assert_error(another.cursor_evidence(Browser, 1, None), QueryError::Busy);
    assert!(h.image(id).is_err());
    drop(browser);
    std::thread::scope(|scope| {
        scope.spawn(|| h.cleanup_before(2).unwrap()).join().unwrap();
    });
    assert_error(q.image(Browser, id), QueryError::Unavailable);
    let mut bytes = vec![];
    std::io::Read::read_to_end(&mut mcp.stream, &mut bytes).unwrap();
    assert_eq!(bytes, common::webp(b"retained bytes"));
    drop(mcp);
    h.cleanup_before(2).unwrap();
    let newer = h.screenshot_gap(&sample(3, "A"), "missing").unwrap();
    assert!(newer > id);
    assert_error(q.image(Mcp, newer), QueryError::Unavailable);
}

#[test]
fn compatible_typed_decoding_keeps_legacy_and_unknown_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.append(0, 0, 1, serde_json::json!({"kind":"focus", "state":{"manual_paused":false,"connected":true,"locked":false,"idle":false,"focus":{"address":"old","application":"editor","title":"legacy"},"focus_known":true}})).unwrap();
    h.append(
        10,
        10,
        1,
        serde_json::json!({"kind":"future-kind", "payload":42}),
    )
    .unwrap();
    put(
        &mut h,
        20,
        20,
        1,
        StateChange::Checkpoint,
        state(None, false),
    );
    let q = Queries::open(dir.path()).unwrap();
    let observations = q
        .observations(Browser, range(0, 30), page(100, None))
        .unwrap()
        .items;
    assert!(matches!(observations[0].evidence, Evidence::State { .. }));
    assert!(
        matches!(&observations[1].evidence, Evidence::Unknown(value) if value["payload"] == 42)
    );
    let overview = q
        .overview(Browser, Selection::Range(range(0, 30)), page(100, None))
        .unwrap();
    assert_eq!(overview.totals.focus_ms, 10);
}

#[test]
fn pages_disclose_byte_truncation_and_all_listings_resume_without_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    for time in 0..3 {
        h.append(
            time,
            time,
            1,
            serde_json::json!({"kind":"unknown", "payload":"x".repeat(550_000)}),
        )
        .unwrap();
        h.screenshot_gap(&sample(time, "A"), "capture_pending")
            .unwrap();
    }
    let q = Queries::open(dir.path()).unwrap();
    let result = q
        .observations(Browser, range(0, 10), PageRequest::default())
        .unwrap();
    assert_eq!(result.items.len(), 1);
    assert!(result.truncated);
    let result = q
        .observations(Browser, range(0, 10), page(100, result.next_cursor))
        .unwrap();
    assert_eq!(result.items[0].id, 2);
    let screenshots = q
        .screenshots(Browser, range(0, 10), None, page(1, None))
        .unwrap();
    let second = q
        .screenshots(
            Browser,
            range(0, 10),
            None,
            page(1, screenshots.next_cursor),
        )
        .unwrap();
    assert_ne!(screenshots.items[0].id, second.items[0].id);
    assert_eq!(q.observation(Mcp, 1).unwrap().id, 1);
    h.cleanup_before(1).unwrap();
    assert_error(q.observation(Mcp, 1), QueryError::Unavailable);
}

#[test]
fn overview_and_block_pages_pin_their_endpoint_while_recording_continues() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    for time in 0..5 {
        put(
            &mut h,
            time,
            time,
            1,
            StateChange::Focus,
            state(Some(if time % 2 == 0 { "A" } else { "B" }), false),
        );
    }
    let q = Queries::open(dir.path()).unwrap();
    let expected = q
        .overview(Browser, Selection::Range(range(0, 10)), page(500, None))
        .unwrap();
    let first = q
        .overview(Browser, Selection::Range(range(0, 10)), page(1, None))
        .unwrap();
    let blocks = q.blocks(Browser, range(0, 10), page(1, None)).unwrap();
    put(
        &mut h,
        6,
        6,
        1,
        StateChange::Checkpoint,
        state(Some("A"), false),
    );
    let mut rows = first.breakdown.items;
    let mut next = first.breakdown.next_cursor;
    while next.is_some() {
        let response = q
            .overview(Browser, Selection::Range(range(0, 10)), page(1, next))
            .unwrap();
        assert_eq!(response.totals.focus_ms, 4);
        rows.extend(response.breakdown.items);
        next = response.breakdown.next_cursor;
    }
    assert_eq!(
        serde_json::to_value(rows).unwrap(),
        serde_json::to_value(expected.breakdown.items).unwrap()
    );
    let mut all_blocks = blocks.items;
    let mut next = blocks.next_cursor;
    while next.is_some() {
        let response = q.blocks(Browser, range(0, 10), page(1, next)).unwrap();
        all_blocks.extend(response.items);
        next = response.next_cursor;
    }
    assert_eq!(all_blocks.len(), 4);
    assert_eq!(all_blocks.iter().map(|b| b.elapsed_ms).sum::<i64>(), 4);
}

#[test]
fn missing_files_are_unavailable_and_cleanup_lease_returns_busy_without_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    let prepared = History::prepare_image(dir.path(), &common::webp(b"bytes")).unwrap();
    let id = h
        .commit_screenshot(&sample(0, "A"), "pixels", &prepared)
        .unwrap();
    drop(prepared);
    let q = Queries::open(dir.path()).unwrap();
    let cleanup = std::fs::File::open(dir.path().join("images.lock")).unwrap();
    cleanup.try_lock().unwrap();
    assert_error(q.image(Browser, id), QueryError::Busy);
    drop(cleanup);
    common::remove_packs(dir.path());
    assert_error(q.image(Mcp, id), QueryError::Unavailable);
}

#[test]
fn forward_clock_jump_exposes_uncertain_time_without_inventing_image_or_focus_coverage() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    put(&mut h, 0, 0, 1, StateChange::Focus, state(Some("A"), false));
    let prepared = History::prepare_image(dir.path(), &common::webp(b"bytes")).unwrap();
    h.commit_screenshot(&sample(0, "A"), "pixels", &prepared)
        .unwrap();
    drop(prepared);
    put(
        &mut h,
        5,
        5,
        1,
        StateChange::Checkpoint,
        state(Some("A"), false),
    );
    h.screenshot_gap(
        &ScreenshotSample {
            monotonic_ms: 10,
            ..sample(1000, "A")
        },
        "clock_discontinuity",
    )
    .unwrap();
    boundary(&mut h, 1000, 10, 2);
    put(
        &mut h,
        1000,
        10,
        2,
        StateChange::Focus,
        state(Some("A"), false),
    );
    put(
        &mut h,
        1010,
        20,
        2,
        StateChange::Checkpoint,
        state(Some("A"), false),
    );
    let q = Queries::open(dir.path()).unwrap();
    let overview = q
        .overview(Browser, Selection::Range(range(0, 1010)), page(100, None))
        .unwrap();
    assert_eq!(overview.totals.focus_ms, 15);
    assert!(overview.breakdown.items.iter().any(|r| matches!(r, Breakdown::FocusCoverage(Coverage { focus: FocusCoverage::Gap { reason }, elapsed_ms: None, .. }) if reason == "clock_discontinuity")));
    assert!(
        q.cursor_evidence(Browser, 500, None)
            .unwrap()
            .screenshots
            .is_empty()
    );
}

#[test]
fn cached_evidence_is_private_to_its_history_and_new_reads_include_appends() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let mut a = History::open(first.path()).unwrap();
    let mut b = History::open(second.path()).unwrap();
    for (history, title) in [(&mut a, "private A"), (&mut b, "private B")] {
        for time in [0, 10] {
            put(
                history,
                time,
                time,
                1,
                StateChange::Checkpoint,
                state(Some(title), false),
            );
        }
    }
    let read = |directory: &std::path::Path| {
        Queries::open(directory)
            .unwrap()
            .overview(Browser, Selection::Range(range(0, 30)), page(500, None))
            .unwrap()
    };
    for _ in 0..3 {
        let response = read(first.path());
        assert_eq!(response.totals.focus_ms, 10);
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("private A"));
        assert!(!json.contains("private B"));
        let response = read(second.path());
        assert_eq!(response.totals.focus_ms, 10);
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("private B"));
        assert!(!json.contains("private A"));
    }
    put(
        &mut a,
        20,
        20,
        1,
        StateChange::Checkpoint,
        state(Some("private A"), false),
    );
    assert_eq!(read(first.path()).totals.focus_ms, 20);
    let records = Queries::open(first.path())
        .unwrap()
        .observations(Browser, range(0, 30), page(500, None))
        .unwrap();
    assert_eq!(
        records
            .items
            .iter()
            .map(|record| (record.id, record.unix_ms))
            .collect::<Vec<_>>(),
        vec![(1, 0), (2, 10), (3, 20)]
    );
    a.cleanup_before(15).unwrap();
    assert_eq!(read(first.path()).totals.focus_ms, 0);
}

#[test]
fn large_overviews_preserve_pagination_when_the_cache_budget_is_exceeded() {
    let dir = tempfile::tempdir().unwrap();
    let mut history = History::open(dir.path()).unwrap();
    let title = "a".repeat(64 * 1024);
    for time in 0..160 {
        put(
            &mut history,
            time,
            time,
            1,
            StateChange::Checkpoint,
            state(Some(&title), false),
        );
    }
    let queries = Queries::open(dir.path()).unwrap();
    let mut cursor = None;
    let mut observed = Vec::new();
    loop {
        let response = queries
            .overview(Browser, Selection::Range(range(0, 160)), page(25, cursor))
            .unwrap();
        assert_eq!(response.totals.focus_ms, 159);
        for row in response.breakdown.items {
            if let Breakdown::FocusCoverage(coverage) = row
                && let Some(id) = coverage.observation
            {
                observed.push(id);
            }
        }
        cursor = response.breakdown.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(observed, (1..160).collect::<Vec<_>>());
}

#[test]
fn display_resolution_bounds_rows_preserves_totals_gaps_and_exact_selection() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    let prepared = History::prepare_image(dir.path(), &common::webp(b"pixels")).unwrap();
    for i in 0..=240 {
        let ms = i * 5000;
        let mut current = state(Some(if i % 2 == 0 { "A" } else { "B" }), i % 3 == 0);
        current.manual_paused = i == 122;
        put(&mut h, ms, ms, 1, StateChange::Focus, current);
        if i == 123 {
            h.screenshot_gap(&sample(ms, "A"), "tiny_gap").unwrap();
        } else {
            if let Some(image) = h.find_image("pixels").unwrap() {
                h.reuse_screenshot(&sample(ms, "A"), &image)
                    .unwrap()
                    .unwrap();
            } else {
                h.commit_screenshot(&sample(ms, "A"), "pixels", &prepared)
                    .unwrap();
            }
        }
    }
    let q = Queries::open(dir.path()).unwrap();
    let visible = range(0, 1_200_000);
    let raw = q
        .overview(Browser, Selection::Range(visible), page(500, None))
        .unwrap();
    let display = q
        .overview_display(
            Browser,
            Selection::Range(visible),
            page(500, None),
            Some(32),
        )
        .unwrap();
    assert_eq!(
        serde_json::to_value(&raw.totals).unwrap(),
        serde_json::to_value(&display.totals).unwrap()
    );
    let summary = |overview: &Overview| {
        overview
            .breakdown
            .items
            .iter()
            .filter_map(|row| match row {
                Breakdown::Application { .. } | Breakdown::ExactTitle { .. } => {
                    Some(serde_json::to_value(row).unwrap())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(summary(&raw), summary(&display));
    let focus: Vec<_> = display
        .breakdown
        .items
        .iter()
        .filter_map(|row| match row {
            Breakdown::DisplayFocus(row) => Some(row),
            _ => None,
        })
        .collect();
    assert!(!focus.is_empty() && focus.len() <= 32);
    assert!(focus.iter().all(|row| row.mixed));
    assert!(focus.iter().any(|row| row.has_gap));
    let images: Vec<_> = display
        .breakdown
        .items
        .iter()
        .filter_map(|row| match row {
            Breakdown::DisplayImage(row) => Some(row),
            _ => None,
        })
        .collect();
    assert!(!images.is_empty() && images.len() <= 32);
    assert!(images.iter().any(|row| row.unavailable.is_some()));
    assert!(display.breakdown.next_cursor.is_none());
    assert!(raw.breakdown.items.len() > display.breakdown.items.len() * 6);
    let exact = q.cursor_evidence(Browser, 617_000, None).unwrap();
    assert!(
        matches!(&exact.focus[0].focus, FocusCoverage::Focus { window, .. } if window.title == "B")
    );
    assert_eq!(
        exact.screenshots[0].screenshot.unavailable.as_deref(),
        Some("tiny_gap")
    );
    let zoomed = q
        .overview_display(
            Browser,
            Selection::Range(range(610_000, 670_000)),
            page(500, None),
            Some(640),
        )
        .unwrap();
    assert!(
        zoomed
            .breakdown
            .items
            .iter()
            .any(|row| matches!(row, Breakdown::ImageCoverage(_)))
    );
    assert!(
        !zoomed
            .breakdown
            .items
            .iter()
            .any(|row| matches!(row, Breakdown::DisplayFocus(_) | Breakdown::DisplayImage(_)))
    );
    // Cursor pages must keep their resolution; do not reuse cached pages from a different budget.
    let first = q
        .overview_display(Browser, Selection::Range(visible), page(3, None), Some(32))
        .unwrap();
    let cursor = first.breakdown.next_cursor.unwrap();
    assert_error(
        q.overview_display(
            Browser,
            Selection::Range(visible),
            page(3, Some(cursor.clone())),
            Some(64),
        ),
        QueryError::InvalidRequest,
    );
    let mut rows = first.breakdown.items;
    let mut cursor = Some(cursor);
    while cursor.is_some() {
        let next = q
            .overview_display(
                Browser,
                Selection::Range(visible),
                page(3, cursor),
                Some(32),
            )
            .unwrap();
        rows.extend(next.breakdown.items);
        cursor = next.breakdown.next_cursor;
    }
    assert_eq!(
        serde_json::to_value(rows).unwrap(),
        serde_json::to_value(&display.breakdown.items).unwrap()
    );
    for points in [0, 2049] {
        assert_error(
            q.overview_display(
                Browser,
                Selection::Range(visible),
                page(100, None),
                Some(points),
            ),
            QueryError::InvalidRequest,
        );
    }
}

#[test]
fn navigation_uses_real_activity_and_distinct_capture_times_across_days() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    put(
        &mut h,
        1000,
        1000,
        1,
        StateChange::Focus,
        state(Some("A"), false),
    );
    put(
        &mut h,
        9000,
        9000,
        1,
        StateChange::Checkpoint,
        state(Some("A"), false),
    );
    let image = History::prepare_image(dir.path(), &common::webp(b"pixels")).unwrap();
    h.commit_screenshot(&sample(2000, "A"), "pixels", &image)
        .unwrap();
    let name = h.find_image("pixels").unwrap().unwrap();
    h.reuse_screenshot(&sample(2000, "B"), &name)
        .unwrap()
        .unwrap();
    h.screenshot_gap(&sample(3000, "A"), "missing").unwrap();
    h.reuse_screenshot(&sample(86_402_000, "A"), &name)
        .unwrap()
        .unwrap();
    let q = Queries::open(dir.path()).unwrap();
    assert_eq!(
        q.adjacent_screenshot(Browser, 2000, 1, None).unwrap(),
        Some(86_402_000)
    );
    assert_eq!(
        q.adjacent_screenshot(Browser, 86_402_000, -1, None)
            .unwrap(),
        Some(2000)
    );
    assert_eq!(
        q.adjacent_screenshot(Browser, 2000, -1, None).unwrap(),
        None
    );
    assert_eq!(
        q.adjacent_screenshot(Browser, 2000, 1, Some("B")).unwrap(),
        None
    );
    assert_error(
        q.adjacent_screenshot(Browser, 0, 0, None),
        QueryError::InvalidRequest,
    );
    let bounds = q
        .activity_bounds(Browser, Selection::Range(range(0, 10_000)))
        .unwrap();
    assert_eq!(bounds.activity, Some(range(1000, 9000)));
    assert!(
        q.activity_bounds(Browser, Selection::Range(range(20_000, 30_000)))
            .unwrap()
            .activity
            .is_none()
    );
}

#[test]
fn incremental_overviews_pin_pages_and_reconstruct_the_exact_next_snapshot() {
    use std::collections::BTreeSet;
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    for i in 0..=10 {
        put(
            &mut h,
            i * 5000,
            i * 5000,
            1,
            StateChange::Focus,
            state(Some("A"), false),
        );
    }
    let q = Queries::open(dir.path()).unwrap();
    let visible = range(0, 120_000);
    let first = q
        .overview_display(
            Browser,
            Selection::Range(visible),
            page(500, None),
            Some(16),
        )
        .unwrap();
    let unchanged = q
        .overview_updates(
            Browser,
            Selection::Range(visible),
            page(500, None),
            Some(16),
            &first.revision,
        )
        .unwrap();
    assert!(!unchanged.reset);
    assert!(unchanged.changes.items.is_empty());
    assert_eq!(unchanged.revision, first.revision);
    put(
        &mut h,
        55_000,
        55_000,
        1,
        StateChange::Focus,
        state(Some("B"), false),
    );
    put(
        &mut h,
        60_000,
        60_000,
        1,
        StateChange::Checkpoint,
        state(Some("B"), false),
    );
    let next = q
        .overview_display(
            Browser,
            Selection::Range(visible),
            page(500, None),
            Some(16),
        )
        .unwrap();
    let mut update = q
        .overview_updates(
            Browser,
            Selection::Range(visible),
            page(1, None),
            Some(16),
            &first.revision,
        )
        .unwrap();
    assert!(!update.reset);
    assert!(update.changes.next_cursor.is_some());
    let mut reconstructed: BTreeSet<String> = first
        .breakdown
        .items
        .iter()
        .map(|row| serde_json::to_string(row).unwrap())
        .collect();
    // A later append must not leak into the remaining pages of this update.
    put(
        &mut h,
        65_000,
        65_000,
        1,
        StateChange::Focus,
        state(Some("C"), false),
    );
    loop {
        assert_eq!(update.revision, next.revision);
        for change in update.changes.items {
            let encoded = serde_json::to_string(&change.row).unwrap();
            if change.remove {
                reconstructed.remove(&encoded);
            } else {
                reconstructed.insert(encoded);
            }
        }
        if update.changes.next_cursor.is_none() {
            break;
        }
        update = q
            .overview_updates(
                Browser,
                Selection::Range(visible),
                page(1, update.changes.next_cursor),
                Some(16),
                &first.revision,
            )
            .unwrap();
    }
    assert_eq!(
        reconstructed,
        next.breakdown
            .items
            .iter()
            .map(|row| serde_json::to_string(row).unwrap())
            .collect()
    );
    assert_eq!(
        serde_json::to_value(update.totals.unwrap()).unwrap(),
        serde_json::to_value(next.totals).unwrap()
    );
    assert_error(
        q.overview_updates(
            Browser,
            Selection::Range(visible),
            page(500, None),
            Some(32),
            &first.revision,
        ),
        QueryError::InvalidRequest,
    );
    h.cleanup_before(10_000).unwrap();
    assert!(
        q.overview_updates(
            Browser,
            Selection::Range(visible),
            page(500, None),
            Some(16),
            &next.revision
        )
        .unwrap()
        .reset
    );
}
