//! Synthetic browser acceptance fixture. Never connects to a desktop or captures it.
//!
//! `cargo run --example web_fixture -- --demo [address]` instead serves three weeks
//! of varied synthetic focus with demo categories and commits, for looking at the UI.
//!
//! Commits come from temporary git repositories. Git is isolated from the
//! developer's configuration, and both fixtures use only temporary directories.
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

[[categories]]
name = "Mail"
parent = "Communication"
[[categories.rules]]
application = "Mail"
"##;

/// Git for this process and its fixture repositories: a temporary global
/// configuration with a demo identity, never the developer's.
fn isolate_git() -> anyhow::Result<tempfile::TempDir> {
    let directory = tempfile::tempdir()?;
    let global = directory.path().join("gitconfig");
    std::fs::write(
        &global,
        "[user]\n\tname = Demo\n\temail = demo@example.com\n[init]\n\tdefaultBranch = main\n",
    )?;
    // SAFETY: called first in main, before any other thread exists.
    unsafe {
        std::env::set_var("GIT_CONFIG_GLOBAL", &global);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    }
    Ok(directory)
}

/// Runs git in `directory` as if at `ms`, which dates the commit and its reflog entry.
fn git(directory: &std::path::Path, ms: i64, arguments: &[&str]) -> anyhow::Result<()> {
    let date = format!("@{} +0000", ms / 1000);
    let output = std::process::Command::new("git")
        .current_dir(directory)
        .args(arguments)
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

/// A repository at `directory` with one commit per `(ms, subject)`.
fn repository(directory: &std::path::Path, commits: &[(i64, &str)]) -> anyhow::Result<()> {
    std::fs::create_dir_all(directory)?;
    git(directory, 0, &["init", "-q"])?;
    for (index, (ms, subject)) in commits.iter().enumerate() {
        std::fs::write(directory.join("work.txt"), format!("{index}\n"))?;
        git(directory, *ms, &["add", "work.txt"])?;
        git(directory, *ms, &["commit", "-q", "-m", subject])?;
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let _git = isolate_git()?;
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().map(String::as_str) == Some("--demo") {
        return demo(arguments.get(1).map_or("127.0.0.1:47832", String::as_str));
    }
    let dir = tempfile::tempdir()?;
    let config = tempfile::tempdir()?;
    let settings = config.path().join("config.toml");
    let base = 1790042400000; // 2026-09-22 04:00 Europe/Oslo
    // Two commits close together cluster at full-day scale, others stay apart;
    // one repository is broken.
    let work = tempfile::tempdir()?;
    repository(
        &work.path().join("projects/timeline"),
        &[
            (base + 600_000, "Plan the commit track"),
            (base + 630_000, "Fix marker spacing"),
            (base + 3_300_000, "Write the browser test"),
            (base + 86_400_000 + 10_800_000, "Follow up on review"),
        ],
    )?;
    repository(
        &work.path().join("projects/notes"),
        &[(base + 2_100_000, "Notes on markers")],
    )?;
    std::fs::create_dir_all(work.path().join("broken"))?;
    std::fs::write(work.path().join("broken/.git"), "gitdir: ../missing\n")?;
    std::fs::write(
        &settings,
        format!(
            "{CATEGORIES}\n[git]\npaths = [{:?}]\n",
            work.path().display().to_string()
        ),
    )?;
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

const DEMO_CATEGORIES: &str = r##"
[[title_rules]]
pattern = '^[\x{2800}-\x{28FF}✳]\s+'

[[categories]]
name = "Projects"

[[categories]]
name = "Timeline"
parent = "Projects"
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
name = "Writing"
parent = "Projects"
[[categories.rules]]
application = "obsidian"
[[categories.rules]]
application = "code"
title = 'notes'

[[categories]]
name = "Communication"
[[categories.rules]]
application = "thunderbird"

[[categories]]
name = "Chat"
parent = "Communication"
color = "#c45ae0"
[[categories.rules]]
application = "Slack"

[[categories]]
name = "Meetings"
parent = "Communication"
[[categories.rules]]
application = "zoom"

[[categories]]
name = "Browsing"
[[categories.rules]]
application = "firefox"
"##;

/// Three weeks of activity days ending today: weekdays 09:00–17:30 local with
/// lunch pauses, short weekend mornings, idle periods, brief glances at chat,
/// and one weekday without any evidence. Deterministic, so screenshots of the UI
/// are comparable.
fn demo(address: &str) -> anyhow::Result<()> {
    use chrono::{Datelike, TimeZone, Utc};
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
    let work = tempfile::tempdir()?;
    std::fs::write(
        &settings,
        format!(
            "{DEMO_CATEGORIES}\n[git]\npaths = [{:?}]\n",
            work.path().display().to_string()
        ),
    )?;
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
    let first = today - chrono::Days::new(20);
    let base = local(first, 0, 0);
    let mut seed: u64 = 0x5eed;
    let mut random = move |limit: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % limit
    };
    for offset in 0..21 {
        let date = first + chrono::Days::new(offset);
        let weekend = date.weekday().number_from_monday() > 5;
        let (start, end) = if weekend {
            (local(date, 10, 0), local(date, 12, 30).min(now))
        } else {
            (local(date, 9, 0), local(date, 17, 30).min(now))
        };
        // A weekday ten days ago was never recorded.
        if start >= end || offset == 10 {
            continue;
        }
        let (lunch, back) = if weekend {
            (end, end)
        } else {
            (local(date, 12, 0), local(date, 12, 45))
        };
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
    demo_commits(work.path(), first, now, local)?;
    let stop = Arc::new(AtomicBool::new(false));
    let web = Web::configured(dir.path(), address, Some(settings.clone()))?;
    let server = timeline::web::start(web.clone(), stop)?;
    println!("{}", web.launch_url()?);
    println!("history {}", dir.path().display());
    println!("settings {}", settings.display());
    println!("repositories {}", work.path().display());
    std::io::stdout().flush()?;
    server.join().unwrap()
}

/// Commits in two repositories during working hours, including on the day
/// without recorded evidence: commits remain evidence when the recorder is off.
fn demo_commits(
    work: &std::path::Path,
    first: chrono::NaiveDate,
    now: i64,
    local: impl Fn(chrono::NaiveDate, u32, u32) -> i64,
) -> anyhow::Result<()> {
    use chrono::Datelike;
    const SUBJECTS: &[&str] = &[
        "Show commits on the timeline",
        "Cluster markers that overlap",
        "Count commits per day",
        "Fix the tooltip position",
        "Read reflogs instead of commit dates",
        "Handle repositories without reflogs",
        "Tidy the settings parser",
        "Explain missing evidence in the week view",
        "Speed up repository discovery",
        "Add a test for worktrees",
    ];
    let mut seed: u64 = 0xc0ffee;
    let mut random = move |limit: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % limit
    };
    let mut timeline = Vec::new();
    let mut notes = Vec::new();
    for offset in 0..21 {
        let date = first + chrono::Days::new(offset);
        let weekend = date.weekday().number_from_monday() > 5;
        let (count, from, minutes) = if weekend {
            (random(2), local(date, 10, 15), 120)
        } else {
            (random(7), local(date, 9, 20), 460)
        };
        for _ in 0..count {
            let at = from + 60_000 * random(minutes) as i64;
            timeline.push((at, SUBJECTS[random(SUBJECTS.len() as u64) as usize]));
        }
        if !weekend && random(3) == 0 {
            notes.push((from + 60_000 * random(minutes) as i64, "Daily notes"));
        }
    }
    for (name, mut commits) in [("projects/timeline", timeline), ("notes", notes)] {
        commits.retain(|(at, _)| *at <= now);
        commits.sort();
        repository(&work.join(name), &commits)?;
    }
    Ok(())
}
