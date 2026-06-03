---
name: run-lyrtui
description: run, start, launch, screenshot, drive, test the lyrtui TUI app; observe the terminal UI, send keystrokes, verify navigation and playback controls
---

lyrtui is a Rust ratatui TUI client for Lyrion Music Server. It is driven via `.claude/skills/run-lyrtui/driver.sh`, which wraps the running process in a detached tmux session so an agent can send keys and capture screen output programmatically.

## Prerequisites

- `tmux` must be installed (`sudo apt-get install tmux` if missing)
- Rust toolchain (`cargo`) must be available
- Lyrion Music Server running at `localhost:9000` — if absent, the app starts but shows "Disconnected / Reconnecting..." in the Status panel instead of library data

## Build

```sh
cargo build
```

The dev build is fast (< 1s if already compiled). For a release build: `cargo build --release`.

## Run (agent path)

All commands are run from the repo root. The driver manages a tmux session named `lyrtui-driver`.

### Launch

```sh
.claude/skills/run-lyrtui/driver.sh launch
```

Prints `ready` when the Navigation panel appears (up to 10 s). Optionally pass dimensions: `launch 160x50`.

### Capture screen

```sh
.claude/skills/run-lyrtui/driver.sh ss
```

Prints the current terminal content to stdout — use this to observe state after any action.

### Send keystrokes

```sh
.claude/skills/run-lyrtui/driver.sh send "j" "j" "Enter"
```

Each argument is one key sent via `tmux send-keys`. After all keys are sent, the driver waits 300 ms and then captures the screen. Key names follow tmux syntax: `"Enter"`, `"Escape"`, `"Up"`, `"Down"`, `" "` (space), etc.

### Quit

```sh
.claude/skills/run-lyrtui/driver.sh quit
```

Sends `q` to the app and kills the tmux session.

### Status check

```sh
.claude/skills/run-lyrtui/driver.sh status
# → "running" or "stopped"
```

## Key bindings (for use with `send`)

| Key | Action |
|-----|--------|
| `j` / `k` | Navigate down / up |
| `Enter` or `l` | Select / drill in |
| `Escape` or `h` or `BSpace` | Back |
| `Left` | Focus sidebar |
| `Right` | Focus main panel |
| `" "` (space) | Play/pause |
| `n` | Next track |
| `p` | Previous track |
| `+` / `-` | Volume up / down |
| `a` | Add to queue |
| `x` | Clear queue (prompts confirmation) |
| `t` | Toggle player power |
| `c` | Open config modal |
| `q` | Quit |

## Run (human path)

```sh
cargo run
```

Opens the TUI full-screen. Press `q` to quit. This is useless headless or in a non-interactive terminal.

## Typical agent flow

```sh
# 1. Launch
.claude/skills/run-lyrtui/driver.sh launch

# 2. Observe starting state — sidebar focused on "My Music"
.claude/skills/run-lyrtui/driver.sh ss

# 3. Navigate to Artists list: Right focuses main panel, Enter selects
.claude/skills/run-lyrtui/driver.sh send "Right" "Enter"
.claude/skills/run-lyrtui/driver.sh ss              # verify Artists (N) panel loaded

# 4. Drill into an artist's albums
.claude/skills/run-lyrtui/driver.sh send "j" "j" "Enter"
sleep 1
.claude/skills/run-lyrtui/driver.sh ss              # verify Albums panel

# 5. Go back
.claude/skills/run-lyrtui/driver.sh send "Escape"
.claude/skills/run-lyrtui/driver.sh ss

# 6. Done
.claude/skills/run-lyrtui/driver.sh quit
```

## Gotchas

- **The app starts in My Music view with the sidebar focused.** The main panel shows the My Music submenu (Artists, Album Artists, Recently Played Artists, Albums, Popular Albums, Tracks, Playlists, Folders). Press `Right` to focus the main panel, then `Enter` to select a section. Pressing `Enter` from the sidebar also moves focus to the main panel (does not auto-select).
- **Use `Right` + `Enter` to enter a section from the sidebar, not two quick `Enter` presses.** From the sidebar, the first `Enter` moves focus to the main panel; a second `Enter` immediately after (300ms gap) may be eaten before the panel is ready. `Right` + `Enter` is reliable.
- **Albums panel may show `(empty)` briefly** when navigating to an artist's albums — the API call is async and completes within ~1 s. Add `sleep 1` then re-run `ss` to see the populated list.
- **No LMS = no library data.** The app starts fine but shows "Disconnected" in the Status box and "No player selected" in Now Playing. The sidebar and structure are still visible and navigable.
- **`capture-pane` strips terminal color codes** — the output is plain text box-drawing characters, which is exactly what you want for assertions.
- **tmux session name is `lyrtui-driver`.** If another session with that name exists from a previous run, `launch` kills it first.
- **Search view captures keystrokes.** Once in Search, `j`/`k` type into the search box rather than navigate. Press `Escape` to move to results, `Left` to return focus to the sidebar.

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| `ready` never printed, timeout message | Check `/tmp/lyrtui-err.txt` for Rust panics or missing config |
| `tmux: command not found` | `sudo apt-get install tmux` |
| `cargo: command not found` | Install Rust via `rustup` |
| Screen shows garbage / misaligned boxes | Terminal width is too narrow; use `launch 160x50` |
| `launch` times out but `status` says "running" | App started in art mode and "Navigation" text wasn't visible — driver now auto-exits art mode, but if it fails check `ss` for "exit art" and send a backtick manually |
