//! Synthetic browser acceptance fixture. Never connects to a desktop or captures it.
//!
//! `cargo run --example web_fixture -- --demo [address]` instead serves a week
//! of varied synthetic focus with demo categories, for looking at the UI.
use std::{
    io::{BufRead, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use timeline::{
    history::{History, ScreenshotSample},
    recording::{Input, Recorder, Stamp, Window},
    web::Web,
};
fn stamp(ms: i64, base: i64) -> Stamp {
    Stamp {
        unix_ms: ms,
        monotonic_ms: ms - base,
        boot_ms: ms - base,
    }
}
const CATEGORIES: &str = r##"
[[categories]]
name = "Planning"
[[categories.rules]]
application = "Editor"

[[categories]]
name = "Research"
color = "#4f8cff"
[[categories.rules]]
application = "Browser"

[[categories]]
name = "Communication"
[[categories.rules]]
application = "Chat"
"##;

fn main() -> anyhow::Result<()> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().map(String::as_str) == Some("--demo") {
        return demo(arguments.get(1).map_or("127.0.0.1:47832", String::as_str));
    }
    let dir = tempfile::tempdir()?;
    let config = tempfile::tempdir()?;
    let settings = config.path().join("config.toml");
    std::fs::write(&settings, CATEGORIES)?;
    let base = 1790042400000; // 2026-09-22 04:00 Europe/Oslo
    let mut recorder = Recorder::open(dir.path(), stamp(base, base))?;
    recorder.apply(
        Input::Connected {
            locked: false,
            idle: false,
        },
        stamp(base, base),
    )?;
    recorder.apply(
        Input::Focus(Some(Window {
            address: "a".into(),
            application: "Editor".into(),
            title: "<script>Planning notes</script>".into(),
        })),
        stamp(base, base),
    )?;
    // Frequent checkpoints establish continuous evidence without stall boundaries.
    for i in 1..=900 {
        recorder.apply(Input::Tick, stamp(base + i * 4000, base))?;
    }
    recorder.apply(Input::Pause, stamp(base + 3600001, base))?;
    drop(recorder);
    let mut history = History::open(dir.path())?;
    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        "UklGRiIAAABXRUJQVlA4IBYAAAAwAQCdASoBAAEADsD+JaQAA3AAAAAA",
    )?;
    for monitor in ["Display A", "Display B"] {
        let prepared = History::prepare_image(dir.path(), &bytes)?;
        history.commit_screenshot(
            &ScreenshotSample {
                unix_ms: base,
                monotonic_ms: 0,
                segment: 1,
                monitor: monitor.into(),
                width: 1,
                height: 1,
                transform: 0,
            },
            "fixture",
            &prepared,
        )?;
        let image = history.find_image("fixture")?.unwrap();
        for tick in 1..120 {
            let sample = ScreenshotSample {
                unix_ms: base + tick * 5000,
                monotonic_ms: tick * 5000,
                segment: 1,
                monitor: monitor.into(),
                width: 1,
                height: 1,
                transform: 0,
            };
            if tick == 60 {
                history.screenshot_gap(&sample, "fixture_gap")?;
            } else {
                history.reuse_screenshot(&sample, &image)?;
            }
        }
    }
    drop(history);
    let socket = std::os::unix::net::UnixListener::bind(dir.path().join("control.sock"))?;
    let stop = Arc::new(AtomicBool::new(false));
    let control_stop = stop.clone();
    socket.set_nonblocking(true)?;
    std::thread::spawn(move || {
        let mut paused = false;
        while !control_stop.load(Ordering::Relaxed) {
            if let Ok((mut stream, _)) = socket.accept() {
                let mut command = String::new();
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(1)));
                let _ = std::io::BufReader::new(&stream).read_line(&mut command);
                match command.trim() {
                    "pause" => paused = true,
                    "resume" => paused = false,
                    _ => (),
                }
                let _ = writeln!(
                    stream,
                    "{}",
                    serde_json::json!({"recording":{"manual_paused":paused,"locked":false,"connected":true},"ok":true})
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    });
    let web = Web::configured(dir.path(), "127.0.0.1:47832", Some(settings))?;
    let server = timeline::web::start(web.clone(), stop.clone())?;
    println!("{}", web.launch_url()?);
    std::io::stdout().flush()?;
    for input in std::io::stdin().lock().lines() {
        if input? != "append" {
            break;
        }
        let at = base + 3_605_000;
        let mut recorder = Recorder::open(dir.path(), stamp(at, base))?;
        recorder.apply(
            Input::Connected {
                locked: false,
                idle: false,
            },
            stamp(at, base),
        )?;
        recorder.apply(Input::Resume, stamp(at, base))?;
        recorder.apply(
            Input::Focus(Some(Window {
                address: "live".into(),
                application: "Browser".into(),
                title: "Added while open".into(),
            })),
            stamp(at, base),
        )?;
        for tick in 1..=30 {
            recorder.apply(Input::Tick, stamp(at + tick * 4000, base))?;
        }
        let segment = recorder.history().latest()?.unwrap().segment;
        drop(recorder);
        let mut history = History::open(dir.path())?;
        let prepared = History::prepare_image(dir.path(), &bytes)?;
        history.commit_screenshot(
            &ScreenshotSample {
                unix_ms: at,
                monotonic_ms: at - base,
                segment,
                monitor: "Display C".into(),
                width: 1,
                height: 1,
                transform: 0,
            },
            "live-fixture",
            &prepared,
        )?;
        println!("appended");
        std::io::stdout().flush()?;
    }
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap()?;
    Ok(())
}

const DEMO_CATEGORIES: &str = r#"
[[title_rules]]
pattern = '^[\x{2800}-\x{28FF}✳]\s+'

[[categories]]
name = "Timeline"
[[categories.rules]]
application = "code"
title = '(?i)timeline'
[[categories.rules]]
application = "kitty"
title = 'timeline'
[[categories.rules]]
application = "firefox"
title = 'localhost:47831|timeline'

[[categories]]
name = "Communication"
[[categories.rules]]
application = "Slack"
[[categories.rules]]
application = "thunderbird"
[[categories.rules]]
application = "zoom"

[[categories]]
name = "Writing"
[[categories.rules]]
application = "obsidian"
[[categories.rules]]
application = "code"
title = 'notes'

[[categories]]
name = "Browsing"
[[categories.rules]]
application = "firefox"
"#;

/// Seven activity days ending today, 09:00–17:30 local, with lunch pauses,
/// idle periods and brief glances at chat. Deterministic, so screenshots of the UI are comparable.
fn demo(address: &str) -> anyhow::Result<()> {
    use chrono::{TimeZone, Utc};
    const WINDOWS: &[(&str, &str)] = &[
        ("code", "queries.rs - timeline - Visual Studio Code"),
        ("code", "● overview.rs - timeline - Visual Studio Code"),
        ("code", "README.md - notes - Visual Studio Code"),
        ("kitty", "~/repos/timeline: cargo test"),
        ("kitty", "⠋ Claude Code — timeline"),
        ("kitty", "~: htop"),
        ("firefox", "Timeline — localhost:47831 — Mozilla Firefox"),
        (
            "firefox",
            "Pull request #42 · example/timeline — Mozilla Firefox",
        ),
        ("firefox", "std::collections - Rust — Mozilla Firefox"),
        ("firefox", "Hacker News — Mozilla Firefox"),
        ("Slack", "#general - Slack"),
        ("Slack", "Direct message - Slack"),
        ("thunderbird", "Inbox - Mozilla Thunderbird"),
        ("zoom", "Zoom Meeting"),
        ("obsidian", "Daily note - Obsidian"),
    ];
    let dir = tempfile::tempdir()?;
    let config = tempfile::tempdir()?;
    let settings = config.path().join("config.toml");
    std::fs::write(&settings, DEMO_CATEGORIES)?;
    let zone: chrono_tz::Tz = timeline::config::activity_timezone().parse()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64;
    let today = Utc
        .timestamp_millis_opt(now)
        .unwrap()
        .with_timezone(&zone)
        .date_naive();
    let local = |date: chrono::NaiveDate, hour: u32, minute: u32| {
        zone.from_local_datetime(&date.and_hms_opt(hour, minute, 0).unwrap())
            .earliest()
            .unwrap()
            .timestamp_millis()
    };
    let first = today - chrono::Days::new(6);
    let base = local(first, 0, 0);
    let mut seed: u64 = 0x5eed;
    let mut random = move |limit: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % limit
    };
    for offset in 0..7 {
        let date = first + chrono::Days::new(offset);
        let (start, end) = (local(date, 9, 0), local(date, 17, 30).min(now));
        if start >= end {
            continue;
        }
        let (lunch, back) = (local(date, 12, 0), local(date, 12, 45));
        let mut recorder = Recorder::open(dir.path(), stamp(start, base))?;
        recorder.apply(
            Input::Connected {
                locked: false,
                idle: false,
            },
            stamp(start, base),
        )?;
        recorder.apply(Input::Resume, stamp(start, base))?;
        let mut at = start;
        while at < end {
            if (lunch..back).contains(&at) {
                recorder.apply(Input::Pause, stamp(at, base))?;
                while at + 4000 < back {
                    at += 4000;
                    recorder.apply(Input::Tick, stamp(at, base))?;
                }
                at = back;
                recorder.apply(Input::Resume, stamp(at, base))?;
            }
            // Occasional glances at chat, mostly short enough to be bridged
            // within a focus stretch.
            if at > start && random(4) == 0 {
                recorder.apply(
                    Input::Focus(Some(Window {
                        address: "Slack-glance".into(),
                        application: "Slack".into(),
                        title: "Direct message - Slack".into(),
                    })),
                    stamp(at, base),
                )?;
                let until = (at + 1000 * (8 + random(70)) as i64).min(end);
                while at + 4000 < until {
                    at += 4000;
                    recorder.apply(Input::Tick, stamp(at, base))?;
                }
                at = until;
            }
            let (application, title) = WINDOWS[random(WINDOWS.len() as u64) as usize];
            recorder.apply(
                Input::Focus(Some(Window {
                    address: format!("{application}-{}", random(3)),
                    application: application.into(),
                    title: title.into(),
                })),
                stamp(at, base),
            )?;
            let length = 60_000 * (1 + random(if application == "zoom" { 45 } else { 18 })) as i64;
            let idle_from = (random(6) == 0).then(|| at + length / 2);
            let until = (at + length).min(end);
            while at + 4000 < until {
                at += 4000;
                if idle_from.is_some_and(|idle| (idle..idle + 4000).contains(&at)) {
                    recorder.apply(Input::Idle(true), stamp(at, base))?;
                } else {
                    recorder.apply(Input::Tick, stamp(at, base))?;
                }
            }
            if idle_from.is_some() {
                recorder.apply(Input::Idle(false), stamp(until, base))?;
            }
            at = until;
        }
        recorder.apply(Input::Pause, stamp(end, base))?;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let web = Web::configured(dir.path(), address, Some(settings.clone()))?;
    let server = timeline::web::start(web.clone(), stop)?;
    println!("{}", web.launch_url()?);
    println!("history {}", dir.path().display());
    println!("settings {}", settings.display());
    std::io::stdout().flush()?;
    server.join().unwrap()
}
