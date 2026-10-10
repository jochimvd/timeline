# Timeline

A local Rust desktop activity recorder for Linux, Wayland, and Hyprland.

The recorder stores original active-window observations, input-idle state, and
per-monitor screenshots in durable local history. It includes persistent
pause/resume, explicit recording boundaries, bounded evidence inspection, and
30-day retention. A shared Rust evidence service provides activity aggregation
and coverage-aware screenshot selection, shared by a bundled browser timeline
and read-only local MCP server. Integrated acceptance measurements and remaining gates are recorded in
[the acceptance report](docs/acceptance/2026-09-22.md).

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
./target/release/timeline open
./target/release/timeline mcp-config
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
records and 1 MiB of evidence payload per page. Aggregate evidence is available
through the browser and MCP. A missing history directory produces an error rather than creating one.
Pause/resume report success only after the corresponding state is committed.
Stopping the process with SIGINT/SIGTERM preserves committed evidence.

History lives under `$XDG_DATA_HOME/timeline` or `~/.local/share/timeline`.
The recorder makes that directory private and takes a single-writer file lock.
Its control socket is private to the user. HTTP binds only to
`127.0.0.1:47831` and authenticates evidence access. Never point experiments at retained history; use a temporary
`XDG_DATA_HOME` instead. Defaults live in Rust source.

## Evidence and recovery

Hyprland focus/title events are collected independently of screenshot sampling,
with bounded socket reads and explicit failure on overflow. Event times are local
receipt times; Hyprland IPC does not supply original event timestamps. Commas in
titles are preserved. Original observations retain application, address, and title;
the shared evidence service derives activity blocks without rewriting them.

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
database file. Cleanup drops expired screenshot maps and decrements their tile
references. A background worker removes empty packs and compacts packs with more
than 25% unused space. Tiles survive as long as any retained map references them,
even if first recorded before the retention cutoff. Shared publication/read
leases prevent reclamation while prepared captures or admitted readers need the
old pack offsets. The two cross-process image slots also bound export work;
a third read returns busy. Missing or corrupt tile data never substitutes
another screenshot.

## Shared evidence queries

`timeline::queries::Queries` is the common read service for the browser
and MCP adapters. Open one connection per worker with `Queries::open(history_path)`.
All operations take `Client::Browser` or `Client::Mcp`; adapters must supply their
own client class rather than accepting it from a request. The service offers:

- `overview`: activity date or an explicit half-open range, at most 31 days;
  application/exact-title totals, idle/no-focus totals, and paginated independent
  focus/image coverage. Totals include recorded idle focus and do not imply task time.
- `overview_display`: optional display-point budget (1–2048) for lossy timeline
  coverage. The browser sends `display_points` to the overview endpoint. Totals
  remain exact; the default overview and MCP contract continue to return originals.
- `blocks`, `observations`, `screenshots`: ranges up to 48 hours, default 100 and
  maximum 500 records per page. Screenshot listings can filter by monitor.
- `observation`: retrieve one retained original observation by ID.
- `cursor_evidence`: focus and per-monitor screenshot evidence for a selected UTC
  millisecond, including actual capture timestamps, unavailable samples and stale
  flags. Repeated wall times return distinct timing segments.
- `image`: retrieve the exact screenshot ID and its metadata with a bounded byte
  stream; missing/expired references or files return `QueryError::Unavailable`.

Activity dates start at 04:00 in the system's local IANA timezone: `TZ` when it
names one, otherwise the `/etc/localtime` link target, falling back to
`Europe/Oslo` when neither resolves. The zone is read once per process.
Resolved bounds account for DST. Exceptional ambiguous/nonexistent 04:00 boundaries
require an explicit range, using [Chrono's local-time resolution](https://docs.rs/chrono/latest/chrono/offset/enum.LocalResult.html).
Elapsed durations use monotonic time only within a timing segment. Adjacent clips
are additive; uncertain intervals expose no inferred elapsed duration.

`Grouping` supplies a versioned key over original window details. `ExactTitle`
is the default. A replacement policy affects blocks, while exact-title and
application totals remain based on original evidence. Block IDs preserve the
first retained observation across range clipping and include the policy version.

`PageRequest` accepts an opaque continuation cursor. Each page discloses
`next_cursor` and byte-budget `truncated`; evidence payloads are capped at 1 MiB.
Cursors pin the query and append watermarks, so recording cannot change an ongoing
listing. Retention invalidates outstanding cursors explicitly; restart the listing
on `QueryError::CursorInvalidated`. `InvalidRequest`, `LimitExceeded`, `Busy`, and
`Unavailable` are distinct typed errors carried by `anyhow::Error`.

Two cross-process slots cover metadata queries and image streams together; MCP
can occupy one. There is no waiting queue. Hold an image stream only while serving
it and drop it on cancellation. Its lease lets it finish even if retention removes
the reference in the meantime. Metadata queries use a single SQLite read snapshot.
Rows stream through aggregation; pages and cursor-selection responses have a
1 MiB payload ceiling, breakdown/context maps have an 8 MiB accounting budget,
and each SQLite read connection has a 4 MiB page cache with disk-backed temporary
sorting. An over-budget result returns `LimitExceeded` rather than partial totals.

Focus coverage distinguishes observed focus, confirmed no focus, gaps with reasons,
and unknown time. Image coverage intervals refer to actual samples or explicit
missing-sample reasons; portions without an image interval are unknown. Images
never carry across recording/monitor boundaries or replace unavailable evidence.
Known focus is not extended beyond its last durable checkpoint. A screenshot can
be stale within valid coverage, with its original timestamp preserved.

Queries keep a byte-bounded immutable observation snapshot (24 MiB), extending it
at append watermarks and rebuilding after retention. Oversized histories fall back
to streaming with reuse of the last small parsed checkpoint state. Overview
pagination caches serialized rows (8 MiB per result, 16 MiB globally), keyed by
database identity, range, watermarks and retention epoch. Active admitted reads can
retain evicted entries until they finish. These caches preserve original IDs and
pinned cursor semantics; they do not retain decoded screenshots. See the
[integrated measurements](docs/acceptance/2026-09-22.md) for measured costs and the
remaining acceptance limitations.
The existing CLI inspection commands retain their original behavior. The query
contract is exercised through real temporary SQLite histories in `tests/queries.rs`.

## Screenshot capture

Persistent `ext-image-copy-capture-v1` sessions capture each named Wayland output
independently every five seconds. Version 4 `wl_output` names and SHM XRGB8888 or
ARGB8888 support are required. Images preserve native buffer dimensions and carry
the Wayland transform for readers to apply. UTC capture time is mapped from the
protocol's monotonic presentation timestamp; identifiers preserve ordering.

If the compositor stops a capture session while its output remains registered,
the recorder destroys that session and its pending frame, then retries only that
output after 2, 4, 8, 16, and at most every 30 seconds. Retries continue while the
output exists; removal cancels them. A replacement waits for fresh buffer
constraints before becoming available, and queued events from retired sessions
are ignored. Only a valid completed frame resets the backoff. Other outputs and
focus recording continue, and the affected screenshot lane records its gap.

One background worker normalizes opaque pixels and hashes the full frame for
exact image reuse. Otherwise it divides the frame into 256 × 256 regions, hashes
each region's dimensions, encoder settings, and exact pixels with BLAKE3, and
looks up matches across all retained history. Position and monitor transform do
not affect tile identity. This reuse survives restarts and workspace switches.
Only new tiles are encoded with libwebp lossy quality 80/method 0.

New regions are written together into append-only packs targeting 16 MiB, with
one pack sync per capture containing new data. Larger single batches are bounded
at 64 MiB. A newly created pack and its directory are synced before SQLite commits
tile offsets, the complete screenshot map, and the screenshot evidence together.
Interrupted batches leave unreferenced bytes for compaction. A generation/monitor/
lock check still precedes reference publication. Region payload checksums detect
corrupt or truncated packs. Readers acquire a physical lease before their SQLite
snapshot so compaction cannot leave them with obsolete offsets.

The runtime reads only schema 5. On recorder startup, schema-4 tile maps are
converted atomically to positive unsigned LEB128 region IDs (typically 2–3 bytes
instead of 8). Image payloads, references and screenshot IDs are unchanged. This
metadata-only conversion does not create a backup or rewrite packs. Freed SQLite
space is available for later writes; the database file need not shrink immediately.
To convert a schema-3 installation, stop the
recorder and run `timeline migrate-images` before starting the new binary. The
resumable offline conversion packs existing WebP files as single full-frame
regions, without decoding/re-encoding or changing screenshot IDs/timestamps.
It verifies all copied bytes before atomically replacing the database and
removing obsolete image files. `history-before-tiles.sqlite3` is a metadata
recovery artifact, not a supported runtime format. Interrupted cleanup can be
completed by rerunning the migration command. New captures use the smaller grid.

Raw SHM buffers, including completed work waiting for the single encoder, share
a 96 MiB budget. Each monitor has at most one outstanding sample. Rotation gives
outputs fair admission when capacity is tight. Pending samples are recorded as
missing coverage at the next trigger; ten-second capture stalls are cancelled.
Neither pending frames nor gaps count as unchanged screens. Focus recording
continues independently. Pause, lock, suspend, timing boundaries, reconnect and
monitor changes invalidate delayed results; fresh samples establish new coverage.

Below 20 GiB available on the history filesystem, expired history is cleaned
first. Screenshot storage then stops if space remains low and resumes only above
25 GiB; this pressure state survives restart. While a pressure-triggered cleanup
is still running, screenshots wait and metadata continues. There is no recording-size cap and younger history is retained. `status`
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
The ignored `live_capture` test requires two explicitly provisioned headless
outputs named `TIMELINE-TEST-A/B`, on unfocused `timeline-test-*` workspaces.
It requests only those outputs and checks workspace placement/focus throughout.
A 12-second two-output run passed; physical-monitor performance and integrated
resource budgets remain unverified. Ordinary `cargo test` never runs live capture.

The completed planning map and current build ticket live under `.scratch/`.
Screenshot corpora and throwaway build artifacts remain untracked.

Protocol references: [Hyprland IPC](https://wiki.hypr.land/IPC/), the vendored
Hyprland lock-notification XML (including its license), and the `wayland-protocols`
input-idle bindings. SQLite access uses [rusqlite](https://docs.rs/rusqlite/).

## Browser and MCP

With `timeline run` or the user service running, `timeline open` uses `xdg-open`
to launch the timeline. The launch URL contains a random, single-use credential
in its fragment, valid for 60 seconds. The page clears the fragment and exchanges
it for a 12-hour HttpOnly, SameSite=Strict session cookie. The bearer token never
appears in a URL. Sessions expire on process restart. Public HTML/CSS/JavaScript
contain no recorded evidence; evidence endpoints require authentication.

The timeline sits above screenshots and supports hover previews, committed
scrubbing, expandable application lanes, focus and screenshot coverage, a date
picker, zoom/panning, visual application/exact-title breakdowns, monitor layouts,
and image enlargement. Breakdown totals follow the visible range. Date changes
preserve zoom and monitor layout. Superseded requests are aborted and their
results discarded, including images still fetching or decoding. Server-side
selection scans also cancel when a newer generation arrives from the same
authenticated browser tab; other tabs and MCP remain independent. Adjacent
identical coverage bands are combined only for painting, preserving evidence IDs,
gaps and timing boundaries. Images show
actual capture times, stale status, missing coverage, and stable evidence IDs.
Pause/resume uses the recorder's existing durable control path. Closing the
browser leaves the process running.

`timeline mcp-config` prints the local MCP URL and Authorization header for a
client's Streamable HTTP setup. Its output contains the private bearer credential;
the file is `http.token` in the history directory, created with mode 0600.
Host must match the configured loopback authority exactly. Supplied Origin must
match its HTTP origin. Browser POSTs additionally require Origin and a custom
request header; no cross-origin access is enabled. MCP accepts bearer credentials,
not browser cookies, and offers no recording controls or task storage.

The server implements stateless JSON responses over
[MCP Streamable HTTP](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports),
with protocol versions 2025-03-26, 2025-06-18 and 2025-11-25. GET on `/mcp`
returns 405 (no unsolicited SSE stream). Initialize, ping, tool discovery,
tool calls and initialized/cancelled notifications are supported. There are no
server-initiated requests or session IDs. Cancellation notifications are accepted;
MCP reads remain bounded. Superseded bundled-browser selection scans check
cancellation and release their admission slots; canceled work reports busy to
obsolete clients whose results are discarded. POST requests need `Accept: application/json, text/event-stream`.

Read-only tools:

| Tool | Arguments |
| --- | --- |
| `overview` | `date` (YYYY-MM-DD) **or** `range: {start_ms, end_ms}`, optional `cursor`, `limit` |
| `blocks` / `observations` | `range`, optional `cursor`, `limit` |
| `observation` | original observation `id` |
| `screenshots` | `range`, optional `monitor`, `cursor`, `limit` |
| `image` | screenshot `id`; returns metadata plus selected WebP image content |

Metadata tools return JSON in both text and `structuredContent`; query failures
are tool results with `isError: true` and an explicit error code. Unknown tools
and malformed arguments are JSON-RPC errors. Read metadata first, follow all
continuation cursors, and fetch images only where useful. Focus is evidence rather
than task attribution; keep ambiguity unassigned, gaps unknown, and avoid counting
the same elapsed time once per monitor. Tasks, estimates and corrections belong
in the agent conversation.

Query admission remains two expensive reads globally, at most one from MCP.
Adapters allow four blocking operations and sixteen active request handlers,
with no admission queue, 16 KiB request bodies and a 15-second handler deadline.
Browser `/api/tiles/{id}` responses contain a bounded JSON map followed by
unique encoded WebP regions, streamed through two 64 KiB buffers. Disconnecting
drops the response. The browser draws decoded regions directly into canvas and
reuses active tiles across selected frames without re-encoding. Hover uses
`/api/thumbnail/{id}`: a WebP preview with a maximum edge of 640 pixels, generated
on demand with libwebp rescaling and quality 80. A process-wide LRU retains at
most 128 encoded previews / 16 MiB in RAM, with no thumbnail files or database
records. Each read validates that the screenshot is still retained before using
the cache and shares existing read admission and reclamation leases. Cold previews
require tile decoding and resizing; cache hits avoid that work. Full-size viewing,
CLI, MCP,
and `/api/image/{id}` exports reconstruct a lossless WebP; a single-region image
is returned byte-for-byte. Export reconstruction adds on-demand CPU and is
separate from the browser path. MCP image content is limited to 24 MiB before base64 encoding and
returns `limit_exceeded` for larger files. Browser metadata is limited to 20,000
rows / 16 MiB per visible range; it asks the user to zoom in on overflow. Current
monitor images are limited to 96 MiB of declared pixels and 32 MiB of encoded
bytes per displayed set. Shared decoded tiles and canvas backing stores each
have a 96 MiB budget across active and pending views, including enlargement.
Network response assembly and browser/GPU overhead are additional. There is no
unbounded historical image cache. These bounds are not measured
performance guarantees.

### Frontend development and browser checks

The UI uses Svelte 5 and TypeScript. The Rust binary embeds the checked-in
`web/app.js`, HTML and CSS, so Node is not needed to build or run an unchanged
checkout. Svelte and esbuild compile the frontend into one self-contained browser
module; no separate frontend server is required. After changing Svelte or TypeScript:

```sh
npm ci
npm run build:web
npm run check:web
npx playwright install chromium
npm run test:web
cargo test --locked
```

The Svelte views live in `web/App.svelte`, `Timeline.svelte`,
`Breakdown.svelte` and `EvidenceViewer.svelte`. `web/model.ts` owns application
state, request cancellation and hover scheduling; `web/images.ts` owns tile
decoding, reuse and bitmap cleanup; `ScreenshotCanvas.svelte` owns canvas surfaces. Views render state without manually
rebuilding DOM trees. Range updates preserve the displayed evidence while
replacement data loads.

Timeline hover shows a floating preview with the time, application/window details
and available full-resolution screenshots displayed at popup size. Hover and
selected evidence use independent requests and share decoded image content when
possible. The large preview changes only on an explicit selection (timeline
click, screenshot navigation, or Now); hovering, panning and live refresh leave
it and its enlarged dialog unchanged.
The browser requests roughly one display bucket per two CSS pixels (budget rounded
in steps of 64, maximum 2048), recalculated when the timeline resizes. At wide
ranges the server merges focus and per-monitor screenshot coverage before
pagination; adjacent equivalent buckets merge further. Mixed focus is striped,
with a representative application rather than an exact title. Gap-containing
buckets stay marked, including gaps too narrow to show individually. Totals and
point selections remain exact. When the bucket width reaches five seconds or
less, original intervals are returned. The viewer derives its bands from these
rows instead of fetching every activity block separately.
Opening the viewer selects the latest available screenshot and its activity day.
With no screenshots, it opens the current activity day without a selection.
Timeline zoom supports windows down to one minute via the menu or mouse wheel.
The navigation bar groups activity date, Now, the time-window menu (including
Fit activity), and the application-lanes toggle. Screenshot stepping skips unavailable
samples and duplicate monitor timestamps, respects a selected monitor, and can
cross activity days. Dragging also pans across days, preserving the selected
image and fetching history on release. Arrow keys step screenshots; Shift+arrows
pan, +/- zoom, and Home jumps to now. Fit activity frames observed focus and
available screenshot coverage rather than empty hours.
Click an enlarged screenshot to toggle between fitting the window and native
resolution; scroll to inspect a large native image. Click outside the image,
use the overlaid ×, or press Escape to dismiss it.
The visible range checks for changes every five seconds while the tab is visible,
and when focus returns. `overview_updates` accepts the last overview revision and
returns paginated row additions/removals and exact totals. The client applies all
pages atomically, preserving unchanged rows and ignoring responses for an old
range. Unchanged watermarks need no coverage scan. Changed watermarks build a
bounded cached overview and diff it; server coverage scans are not yet fully
incremental. Retention or cache eviction requests a full range reload. Large
selected evidence is never replaced by polling.
The hover line and popup follow every pointer movement immediately. Popup data
samples the latest pointer every 100 ms while moving, with at most one image set
in flight. Leaving cancels only hover work. Hover and selection have separate
server cancellation identities, so neither can supersede the other. The preview
boundary shows loading only before evidence is first available, then retains
the previous content until the replacement is ready, without adding a pending-status line. Wheel gestures update the scale immediately and
coalesce history fetches after 220 ms without another zoom step. Date formatters
and unchanged lane data are reused, collapsed breakdown charts are mounted only
when opened, and displayed image content is shared across duplicate capture IDs.

For read-only profiling against a running recorder with retained screenshots:

```sh
node scripts/acceptance/viewer-resources.mjs
node scripts/acceptance/viewer-resources.mjs --source
node scripts/acceptance/viewer-resources.mjs --source --scroll
node scripts/acceptance/viewer-resources.mjs --load
node scripts/acceptance/viewer-resources.mjs --idle
```

`--source` tests the local compiled frontend against the installed backend.
The script prints aggregate resource measurements without credentials, activity
titles or screenshot contents. See the
[viewer performance measurements](docs/acceptance/2026-09-22-viewer.md).

Browser tests verify that the bundled JavaScript matches Svelte/TypeScript and exercise
the real HTTP/query/history stack in headless Chromium with two synthetic monitor
images. `examples/web_fixture.rs` uses temporary history and never connects to the
desktop. It stubs recorder status/control for UI interaction; Rust adapter tests
separately verify durable pause/resume against a real Recorder and reopened history.
Adapter tests cover authentication, credential expiry/replay, CSRF/Host/Origin,
MCP initialization/discovery, shared pagination, empty days, gaps, retention
invalidation, missing images and busy admission. Browser tests delay old date,
selection and image responses to check that newer evidence stays displayed.
They also cover pointer-anchored wheel zoom, stable rendering during range loads,
continuous hover previews and reuse of displayed images.
The browser fixture can append focus and screenshot evidence after page load;
tests verify automatic refresh and rejection of obsolete background responses.
The existing live-capture test remains opt-in. The [integrated acceptance report](docs/acceptance/2026-09-22.md) records the
physical trial, 30-day synthetic/replay measurements and remaining release gates.

The collapsed Storage details panel reads `/api/storage` only on opening or an
explicit Refresh. It keeps the previous snapshot visible and never polls. The
server caches aggregates for 30 seconds. Values show pack bytes, SQLite database
and journal bytes, compact map bytes, and the RAM preview cache. Tile reuse is
encoded bytes saved across references in unique tiled images, excluding imported
full-frame regions; it is not a comparison against the old whole-screen encoder.
File sizes are a point-in-time approximation while recording and reclamation run.
