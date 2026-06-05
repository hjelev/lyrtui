# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Run Commands

```sh
cargo run                          # Run the TUI app
cargo build --release              # Build optimized binary
cargo test                         # Run all tests
cargo test <test_name>             # Run a single test
cargo clippy -- -D warnings        # Lint (warnings are errors)
cargo check                        # Fast compile check without linking
```

Do **not** run `cargo fmt` — there is no `rustfmt.toml` and it reformats the entire compact codebase.

The Lyrion Music Server must be running locally on `localhost:9000` for the app to connect. If it's not available, the TUI should show a "Disconnected / Reconnecting..." status rather than crashing.

## Module Overview

| File | Role | Size |
|------|------|------|
| `main.rs` | Entry point, event loop, image/thumbnail management | ~45 KB |
| `app.rs` | All application state (`App` struct + `AppMsg` enum) | ~18 KB |
| `ui.rs` | Pure rendering logic (ratatui) | ~141 KB |
| `handlers.rs` | Keyboard/mouse event handlers, async actions | ~129 KB |
| `api.rs` | Lyrion JSON-RPC client (`LmsClient`) | ~40 KB |
| `background.rs` | Long-running background async tasks | ~10 KB |
| `utils.rs` | Shared helpers (URL builders, list-length queries, etc.) | ~10 KB |
| `config.rs` | TOML config load/save (`~/.config/lyrtui/config.toml`) | ~3 KB |
| `discovery.rs` | UDP broadcast LMS server auto-discovery | ~2 KB |
| `events.rs` | `crossterm` event polling and translation | ~4 KB |
| `filter.rs` | Local panel filter (`/`): in-place list filtering with backup/restore | ~6 KB |

## Architecture

The app is structured around a strict separation between UI and network layers, communicating via `tokio::sync::mpsc` channels:

- **`main.rs`** — initializes the terminal and `ratatui_image::Picker`, spawns background tasks, runs the main event loop. Owns the image state (`StatefulProtocol` handles for album art and thumbnails). Contains helpers: `encode_thumb_protocol()` (pre-encodes thumbnails in a `tokio::spawn` so draw-time `needs_resize` is `None`), `refresh_album_art()`, `create_album_art_protocols()`.
- **`app.rs`** — the `App` struct holds all application state. `AppMsg` is the single enum used to communicate from background tasks back to the event loop. No async, no I/O.
- **`ui.rs`** — pure rendering. Takes `&App` and produces no side effects. Exception: `help_visible_lines: Cell<u16>` is written during draw so handlers can clamp scroll. Key draw functions: `draw()` (top-level), `draw_library()`, `draw_queue()`, `draw_browse_list()`, `draw_search()`, `draw_app_search()`, `draw_full_art_mode()`, `render_two_row_view()`, `draw_two_row_list()`.
- **`handlers.rs`** — all keyboard/mouse logic. `handle_action()` is the main key dispatcher; `handle_mouse_event()` handles clicks. Also contains `handle_config_key()`, `handle_context_menu_key()`, and helpers for volume, sync, queue confirmation dialogs.
- **`api.rs`** — `LmsClient` wraps all JSON-RPC HTTP calls via `reqwest`. Covers players, now-playing, queue, artists, albums, tracks, playlists, radio, apps, favourites, folder browsing, search, artwork URLs, and server sync/unsync. Never touches UI types.
- **`background.rs`** — `start_now_playing_loop()` polls playback state; `trigger_search()`, `trigger_app_specific_search()`, `load_radio_items()`, `load_fav_items()`, `load_app_items()` fire one-shot fetches. All send results back via `AppMsg`.
- **`config.rs`** — `Config` struct (serde/toml). Fields: `host`, `port`, `default_player`, `use_nerd_icons`, `username`, `password`, `auto_discover`, `broadcast_mask`, `global_volume_control`, `full_art_mode`, `disable_auto_colors`, `image_protocol`.
- **`discovery.rs`** — UDP broadcast scan to find LMS servers on the local network.
- **`utils.rs`** — `cover_url()`, `artist_artwork_url()`, `folder_id_at()`, `main_list_len()`, `is_track_view()`, and other shared helpers.
- **`filter.rs`** — the local panel filter (`/`). Filters the current view's list **in place**: the backing Vec is replaced with the matching subset while the full list is stashed in `App::local_filter` (`LocalFilter` + `FilterBackup`) for instant restore. Because the Vec *is* the filtered list, every existing selection/index read stays untouched. `open`/`recompute`/`clear`/`reapply_if_active`/`clear_if_view_changed` centralize all the per-view `match`ing. Keyboard: `/` → `Action::OpenLocalFilter`, live editing routed to `handlers::handle_local_filter_key`; the event loop calls `clear_if_view_changed` each iteration and `reapply_if_active` after `QueueLoaded`.

## Key Enums

### `MainView` (current screen)
`Library(LibraryView)` | `MyMusic` | `Queue` | `Players` | `Radio` | `Apps` | `Favourites` | `Help` | `Search` | `AppSearch { cmd, item_id }`

### `LibraryView`
`Artists` | `AlbumArtists` | `Albums { artist_id }` | `Tracks { album_id }` | `Folder { folder_id }` | `Playlists` | `RecentlyPlayedArtists` | `PopularAlbums`

### `AppMsg` (background → event loop)
Covers: `Connected/Disconnected`, `PlayersLoaded`, `NowPlayingUpdated`, `QueueLoaded`, `ArtistsLoaded`, `AlbumArtistsLoaded`, `AlbumsLoaded`, `TracksLoaded`, `RecentArtistsLoaded`, `PopularAlbumsLoaded`, `RadioItemsLoaded`, `AppItemsLoaded`, `FavItemsLoaded`, `FolderItemsLoaded`, `PlaylistsLoaded`, `ArtworkDecoded`, `ThumbnailLoaded(url, DynamicImage, StatefulProtocol)`, `ThumbnailFailed`, `ArtworkFetchFailed`, `ArtistArtworkResolved`, `FolderArtworkResolved`, `PlayerVolumesLoaded`, `PlayerSyncGroupsLoaded`, `StatusMsg`, `SearchResultsLoaded`, `AppSearchResultsLoaded`, `DiscoveredServers`.

## Key Design Rules

- The event loop must never block — all network I/O goes through async tasks communicating over `AppMsg`.
- `ui.rs` must remain pure: `&App` in, rendered frame out. No `async`, no network, no mutation (except the `Cell<u16>` scroll measurement).
- Lyrion communicates via JSON-RPC over HTTP. Base URL: `http://<host>:<port>/jsonrpc.js`. Params: `["<player_id>", "<command>", ...]`.
- Connection errors are caught and surfaced as `ConnectionState` variants, never panics.
- Thumbnails are pre-encoded off the draw path: `encode_thumb_protocol()` spawns a task that sends back `AppMsg::ThumbnailLoaded` with a ready `StatefulProtocol` so `terminal.draw` never blocks on image encode.
- Windowed list rendering: `draw_two_row_list` and `draw_browse_list` take `total: usize` + `make_row: impl Fn(usize) -> RowItem` instead of a pre-built `Vec`, so only visible rows are constructed.
- Lazy artwork: artist art (`artist_artwork: HashMap<String, Option<String>>`) and folder art (`folder_artwork: HashMap<u32, Option<String>>`) use `None` = "no art found" sentinel to prevent re-fetching; absent key = not yet resolved.
- After each significant change: update `README.md` and memory files.
- Split big tasks into a todo list and follow it until completion.
