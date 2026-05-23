---
name: run-lmstui
description: run, start, launch, screenshot, drive, test the lmstui TUI app; observe the terminal UI, send keystrokes, verify navigation and playback controls
---

lmstui is a Rust ratatui TUI client for Lyrion Music Server. It is driven via `.claude/skills/run-lmstui/driver.sh`, which wraps the running process in a detached tmux session so an agent can send keys and capture screen output programmatically.

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

All commands are run from the repo root. The driver manages a tmux session named `lmstui-driver`.

### Launch

```sh
.claude/skills/run-lmstui/driver.sh launch
```

Prints `ready` when the Navigation panel appears (up to 10 s). Optionally pass dimensions: `launch 160x50`.

### Capture screen

```sh
.claude/skills/run-lmstui/driver.sh ss
```

Prints the current terminal content to stdout — use this to observe state after any action.

### Send keystrokes

```sh
.claude/skills/run-lmstui/driver.sh send "j" "j" "Enter"
```

Each argument is one key sent via `tmux send-keys`. After all keys are sent, the driver waits 300 ms and then captures the screen. Key names follow tmux syntax: `"Enter"`, `"Escape"`, `"Up"`, `"Down"`, `" "` (space), etc.

### Quit

```sh
.claude/skills/run-lmstui/driver.sh quit
```

Sends `q` to the app and kills the tmux session.

### Status check

```sh
.claude/skills/run-lmstui/driver.sh status
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
.claude/skills/run-lmstui/driver.sh launch

# 2. Observe starting state
.claude/skills/run-lmstui/driver.sh ss

# 3. Navigate: select Artists → first artist → albums
.claude/skills/run-lmstui/driver.sh send "Enter"   # drill into Artists
.claude/skills/run-lmstui/driver.sh ss              # verify Albums panel

# 4. Go back
.claude/skills/run-lmstui/driver.sh send "Escape"
.claude/skills/run-lmstui/driver.sh ss

# 5. Done
.claude/skills/run-lmstui/driver.sh quit
```

## Gotchas

- **The app starts in Artists view.** The sidebar focus is on "Artists" by default; pressing `Enter` immediately drills into the first artist's albums. Press `Left` first to focus the sidebar if you want to navigate to a different section.
- **Albums panel may show `(empty)` briefly** when navigating to an artist's albums — the API call is async and completes within ~1 s. Re-run `ss` to see the populated list.
- **No LMS = no library data.** The app starts fine but shows "Disconnected" in the Status box and "No player selected" in Now Playing. The sidebar and structure are still visible and navigable.
- **`capture-pane` strips terminal color codes** — the output is plain text box-drawing characters, which is exactly what you want for assertions.
- **tmux session name is `lmstui-driver`.** If another session with that name exists from a previous run, `launch` kills it first.

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| `ready` never printed, timeout message | Check `/tmp/lmstui-err.txt` for Rust panics or missing config |
| `tmux: command not found` | `sudo apt-get install tmux` |
| `cargo: command not found` | Install Rust via `rustup` |
| Screen shows garbage / misaligned boxes | Terminal width is too narrow; use `launch 160x50` |
