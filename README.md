# 🕰️ timeline

> *"Wait, what did I actually do all day?"*: you, around 6pm, every day.

**timeline** is a tiny, local-only flight recorder for your Linux desktop. It
quietly notes which window has focus, whether you're actually at the keyboard,
and grabs a screenshot of each monitor now and then. Later you can scroll back
through your day like a video timeline, or let an AI assistant read it over MCP
and write your standup for you.

No cloud. No account. No telemetry. Your history lives in a SQLite file in your
home directory and never leaves it.

```
 09:00        10:00        11:00        12:00        13:00
 ├────────────┼────────────┼────────────┼────────────┼──────
 ████████▓▓▓▓▓▓▓▓░░░░░░████████████████        ▓▓▓▓▓▓▓▓████
 editor  │ browser │ slack │   editor (deep focus 🔥) │ lunch 🥪
```

## ✨ What you get

- **🪟 Focus history.** Every window switch, with the app and its cleaned-up
  title. Locked screens and idle stretches are recorded as gaps instead of
  pretending you worked through lunch.
- **📸 Screenshots.** Per-monitor WebP captures, deduplicated in tiles so
  they barely take up space. Kept for 30 days; activity metadata is kept for a year.
- **🖥️ A browser timeline.** Day, week and month views with sessions, a
  screenshot scrubber, focus metrics and heatmaps. Run `timeline open` and it's there.
- **🏷️ Categories.** A few regex rules sort your time into *Work › Project X*
  or *Personal › Dotfiles*. Edit the rules and all of your history is
  re-sorted on the spot.
- **🌳 Git commits.** Your commits show up on the timeline next to what you
  were looking at when you made them.
- **🤖 MCP server.** Claude (or any MCP client) can ask what you worked on
  Tuesday and get real answers that cite the evidence.
- **⏸️ Pause button.** `timeline pause`. You know why.

Days run from 04:00 to 04:00, because nobody's day ends at midnight.

## 🚀 Quick start

You'll need Linux, **Hyprland**, Rust, a C compiler, `pkg-config` and the
libwebp headers.

```sh
cargo build --release --locked
./target/release/timeline run      # start recording
```

Then, in another terminal:

```sh
./target/release/timeline open     # 🎉 open the timeline in your browser
```

Run it inside your Hyprland session so it can see `WAYLAND_DISPLAY` and
`HYPRLAND_INSTANCE_SIGNATURE`.

## 🎛️ Commands

| Command | What it does |
|---|---|
| `timeline run` | Record until you hit Ctrl-C |
| `timeline open` | Open the browser UI with a single-use login link |
| `timeline status` | Is it recording? Is it happy? |
| `timeline pause` / `resume` | Stop and start recording, and it remembers across restarts |
| `timeline mcp-config` | Print the config for hooking up an MCP client |
| `timeline observations` | Dump raw evidence as paged JSON |
| `timeline screenshots` | List captured screenshots |
| `timeline image <id>` | Stream one screenshot as WebP to stdout |

## 🏷️ Make it yours

Drop a `~/.config/timeline/config.toml` next to your dotfiles:

```toml
[[categories]]
name = "Work"
  [[categories.children]]
  name = "Project X"
  rules = [
    { application = "code", title = '(?i)project-x' },
    { title = 'Jira' },
  ]

[[categories]]
name = "Personal"
  [[categories.children]]
  name = "Dotfiles"
  rules = [{ title = '(?i)dotfiles' }]

[git]
paths = ["~/code"]   # scanned for repos so your commits land on the timeline
```

The first matching rule wins, and categories can nest four levels deep.

## 🔒 Privacy, seriously

- Everything stays in `~/.local/share/timeline`, readable only by you.
- The web UI listens on `127.0.0.1:47831` only, behind single-use login links
  and signed cookies.
- Pause whenever you like. Paused means nothing is recorded.

## 🐧 Start it with your session

```sh
install -Dm755 target/release/timeline ~/.local/bin/timeline
install -Dm644 deploy/timeline.service ~/.config/systemd/user/timeline.service
systemctl --user import-environment WAYLAND_DISPLAY HYPRLAND_INSTANCE_SIGNATURE
systemctl --user daemon-reload
systemctl --user enable --now timeline.service
```

## 🛠️ Hacking on it

It's Rust on the backend and Svelte 5 + TypeScript on the frontend. The built
`web/app.js` is checked in and baked into the binary, so you only need Node if
you change the UI:

```sh
npm ci && npm run build:web && npm run check:web
```

Tests:

```sh
cargo test --locked
npx playwright install chromium && npm run test:web
```

---

<sub>Made because "I was… busy?" stopped being an acceptable answer at standup.</sub>
