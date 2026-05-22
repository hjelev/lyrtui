# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Run Commands

```sh
cargo run                          # Run the TUI app
cargo build --release              # Build optimized binary
cargo test                         # Run all tests
cargo test <test_name>             # Run a single test
cargo clippy -- -D warnings        # Lint (warnings are errors)
cargo fmt                          # Format code
cargo check                        # Fast compile check without linking
```

The Lyrion Music Server must be running locally on `localhost:9000` for the app to connect. If it's not available, the TUI should show a "Disconnected / Reconnecting..." status rather than crashing.

## Architecture

The app is structured around a strict separation between UI and network layers, communicating via `tokio::sync::mpsc` channels:

- **`main.rs`** — entry point; initializes the terminal, spawns the async API polling task, and starts the event loop
- **`app.rs`** — holds all application state (current player, playlist, playback status, connection state); updated by messages received from the API task
- **`ui.rs`** — pure rendering logic using `ratatui`; reads from `app.rs` state, produces no side effects
- **`api.rs`** — all Lyrion JSON-RPC communication via `reqwest`; runs in a background `tokio` task; never touches UI types
- **`events.rs`** — handles `crossterm` keyboard/mouse events and translates them into app actions

## Key Design Rules

- The `crossterm` event loop must never block — all network I/O goes through async tasks communicating over channels.
- `ui.rs` must remain pure: it takes a reference to app state and renders it, with no `async`, no network calls, and no mutation.
- Lyrion communicates via JSON-RPC over HTTP. The base URL is `http://localhost:9000/jsonrpc.js`. Requests use the `["<player_id>", "<command>", ...]` param structure.
- Connection errors must be caught and surfaced as a connection state enum variant, not propagated as panics.
- after each change update the readme.md and your memory files
- split big tasks and create todo list that you'll follow untill completing the task
