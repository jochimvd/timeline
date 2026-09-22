# Timeline

A local Rust desktop activity recorder for Linux, Wayland, and Hyprland.

The recorder stores original active-window observations, input-idle state, and
per-monitor screenshots in durable local history. It includes persistent
pause/resume, explicit recording boundaries, bounded evidence inspection, and
30-day retention. Activity aggregation, the browser timeline, and MCP are later
implementation slices. Integrated resource and latency targets remain unmeasured.

## Build and run

Build dependencies include a C compiler, pkg-config, and libwebp development files.

```sh
cargo build --release --locked
./target/release/timeline run
```

Run inside the Hyprland session with `XDG_RUNTIME_DIR`, `WAYLAND_DISPLAY`, and
`HYPRLAND_INSTANCE_SIGNATURE` set. Hyprland lock notifications and version 2 of
the Wayland input-idle notification protocol are required. If either the compositor
or its lock state is unavailable, the process serves control requests, reports the
problem, and retries without recording focus.

In another terminal:

```sh
./target/release/timeline status
./target/release/timeline pause
./target/release/timeline resume
./target/release/timeline observations --limit 100
./target/release/timeline observations --after 123 --limit 100
./target/release/timeline screenshots --limit 100
./target/release/timeline image 42 > screenshot.webp
```

`observations` works while recording or offline. It returns raw evidence in
durable identifier order, with `next_after` and `has_more`; limits are 1–500
records and 1 MiB of evidence payload per page. This is an inspection command, not the later aggregate evidence
interface. A missing history directory produces an error rather than creating one.
Pause/resume report success only after the corresponding state is committed.
Stopping the process with SIGINT/SIGTERM preserves committed evidence.

History lives under `$XDG_DATA_HOME/timeline` or `~/.local/share/timeline`.
The recorder makes that directory private and takes a single-writer file lock.
Its control socket is private to the user; no HTTP listener is present in this
slice. Never point experiments at retained history; use a temporary
`XDG_DATA_HOME` instead. Defaults live in Rust source.

## Evidence and recovery

Hyprland focus/title events are collected independently of screenshot sampling,
with bounded socket reads and explicit failure on overflow. Event times are local
receipt times; Hyprland IPC does not supply original event timestamps. Commas in
titles are preserved. Original observations retain application, address, and title;
grouping into activity blocks is a later query concern.

Idle begins at the compositor's five-minute input-idle threshold, ignoring idle
inhibitors, and is an overlay on known focus. No input contents are collected.
Manual pause and lock stop focus evidence; resume/unlock require a fresh snapshot.
Unknown focus is distinct from a confirmed absence of a focused window.

Each observed change is committed with SQLite WAL and full synchronization.
Continued known focus is checkpointed every four seconds, leaving scheduling
headroom within the agreed five-second healthy-operation loss bound. A restart
references the last durable observation and starts fresh coverage. Suspend, event
stalls over ten seconds, and clock discontinuities over one second create new
timing segments; focus duration must never be inferred across these boundaries.
Disk/storage errors terminate the recorder with an error rather than acknowledging
unsaved observations. The user service can then restart it.

Metadata cleanup runs at startup and hourly for observations older than 30 elapsed
days. A persisted retention clock uses elapsed boot time, including suspend, so a
wall-clock jump cannot expire recent history during the same boot or recorder
restart. Across a machine reboot, nonnegative wall-clock elapsed time is the
fallback; elapsed time while powered off cannot be independently verified.
SQLite reuses reclaimed pages; metadata cleanup does not immediately shrink the
database file. Cleanup also removes expired screenshot references and reclaims
unreferenced image files. Two cross-process image-read slots bound streaming
reads to 64 MiB each; a shared lease lets active reads finish before reclamation.
A concurrent third read returns busy. Expired or missing images return an explicit
unavailable error, never a substitute screenshot.

## Screenshot capture

Persistent `ext-image-copy-capture-v1` sessions capture each named Wayland output
independently every five seconds. Version 4 `wl_output` names and SHM XRGB8888 or
ARGB8888 support are required. Images preserve native buffer dimensions and carry
the Wayland transform for readers to apply. UTC capture time is mapped from the
protocol's monotonic presentation timestamp; identifiers preserve ordering.

One background worker hashes completed opaque pixels with BLAKE3 (including size
and transform), reuses retained exact-content matches, and otherwise encodes
libwebp lossy quality 80/method 0. It writes and syncs a private temporary image,
publishes and syncs its directory, then returns to the recording thread for a
generation/monitor/lock check before committing the reference. Interrupted work
can leave an orphan; startup/hourly cleanup reclaims it. SQLite references never
intentionally precede durable images.

Raw SHM buffers, including completed work waiting for the single encoder, share
a 96 MiB budget. Each monitor has at most one outstanding sample. Rotation gives
outputs fair admission when capacity is tight. Pending samples are recorded as
missing coverage at the next trigger; ten-second capture stalls are cancelled.
Neither pending frames nor gaps count as unchanged screens. Focus recording
continues independently. Pause, lock, suspend, timing boundaries, reconnect and
monitor changes invalidate delayed results; fresh samples establish new coverage.

Below 20 GiB available on the history filesystem, expired history is cleaned
first. Screenshot storage then stops if space remains low and resumes only above
25 GiB. There is no recording-size cap and younger history is retained. `status`
exposes pressure, desktop/capture errors and screenshot availability; `screenshots`
lists stable screenshot IDs and per-monitor missing-sample reasons. Its page size
is 1–500; `image ID` streams a single WebP while recording or offline.

## Optional desktop startup

The repository includes a systemd user-service template. Installation and
enablement are explicit; building or testing does not enable recording at login.

```sh
install -Dm755 target/release/timeline ~/.local/bin/timeline
install -Dm644 deploy/timeline.service ~/.config/systemd/user/timeline.service
systemctl --user import-environment WAYLAND_DISPLAY HYPRLAND_INSTANCE_SIGNATURE
systemctl --user daemon-reload
systemctl --user enable --now timeline.service
```

Import the session variables from Hyprland's startup configuration before the
graphical session starts this service, including after a compositor restart that
changes its instance signature. The service follows `graphical-session.target`;
the desktop must manage that target. Logs are available with
`journalctl --user -u timeline.service`.

## Development

```sh
cargo test --locked
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
```

Tests use real temporary SQLite databases, controlled recording events/clocks,
synthetic Hyprland sockets, and child processes. The daemon process test removes
desktop connection variables, so it never records the developer's real windows.
It verifies durable pause after SIGKILL, singleton recording, and inspection.
These checks do not establish real-desktop performance or live compositor behavior.

The completed planning map and current build ticket live under `.scratch/`.
Screenshot corpora and throwaway build artifacts remain untracked.

Protocol references: [Hyprland IPC](https://wiki.hypr.land/IPC/), the vendored
Hyprland lock-notification XML (including its license), and the `wayland-protocols`
input-idle bindings. SQLite access uses [rusqlite](https://docs.rs/rusqlite/).
