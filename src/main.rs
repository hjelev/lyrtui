mod api;
mod app;
mod artwork;
mod background;
mod cli;
mod config;
mod discovery;
mod events;
mod handlers;
mod ui;
mod utils;

use anyhow::Result;
use api::LmsClient;
use app::{App, AppMsg, ConfigModal, MainView};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use events::{Action, InputEvent, key_to_action, poll_event};
use ratatui::{Terminal, backend::CrosstermBackend, widgets::ListState};
use ratatui_image::{picker::Picker, protocol::StatefulProtocol};
use std::{
    collections::{HashMap, HashSet},
    io,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

const TICK_RATE: Duration = Duration::from_millis(250);

#[tokio::main]
async fn main() -> Result<()> {
    let args: std::collections::HashSet<String> = std::env::args().collect();
    let has = |flags: &[&str]| flags.iter().any(|f| args.contains(*f));

    if has(&["-v", "--version"]) {
        println!("lyrtui v{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    if has(&["-i", "--info"]) {
        return cli::print_info().await;
    }

    if has(&["-p", "--play-pause"]) {
        return cli::cmd_play_pause().await;
    }

    if has(&["--next"]) {
        return cli::cmd_next().await;
    }

    if has(&["--prev"]) {
        return cli::cmd_prev().await;
    }

    if has(&["-h", "--help"]) {
        print!(
            "\
lyrtui v{version} — TUI for Lyrion Music Server

USAGE:
  lyrtui [OPTIONS]

OPTIONS:
  -h, --help       Print this help message and exit
  -v, --version    Print version and exit
  -i, --info       Print saved config and live server/player info, then exit
  -p, --play-pause Toggle play/pause on the default player
      --next       Skip to the next track
      --prev       Go to the previous track

NAVIGATION:
  ↑/↓ / j/k       Move up/down in lists
  Tab              Switch between panels
  Enter            Select / confirm
  Esc              Go back / close overlay
  1-9              Jump to menu item
  q                Quit

PLAYBACK:
  Space            Play / Pause
  n                Next track
  p                Previous track
  m                Mute / unmute active player
  +/-              Volume up / down
  < / >            Seek backward / forward

CONFIG:
  c                Open config modal

The app connects to a Lyrion Music Server (default: localhost:9000).
Config file: ~/.config/lyrtui/config.toml
",
            version = env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }

    let mut cfg = config::Config::load()?;

    // Run discovery if this is a first run (no config file) or auto_discover is on.
    let config_file_exists = config::config_path().exists();
    if (!config_file_exists || cfg.auto_discover)
        && let Some((discovered_ip, discovered_port)) =
            discovery::discover_lms(&cfg.broadcast_mask, Duration::from_secs(2))
    {
        cfg.host = discovered_ip;
        cfg.port = discovered_port;
    }

    let client = Arc::new(LmsClient::new(cfg.base_url(), cfg.credentials()));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    // Picker must be created after EnterAlternateScreen, before reading events.
    let mut picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
    // Konsole supports kitty graphics but its DA responses don't trigger from_query_stdio's
    // detection reliably — override halfblocks to kitty when running inside Konsole.
    if picker.protocol_type() == ratatui_image::picker::ProtocolType::Halfblocks
        && (std::env::var_os("KONSOLE_VERSION").is_some()
            || std::env::var_os("KONSOLE_DBUS_SERVICE").is_some())
    {
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    }
    // Remember the auto-detected protocol so switching back to "auto" can restore it at runtime.
    let auto_protocol = picker.protocol_type();
    artwork::apply_image_protocol(&mut picker, &cfg.image_protocol, auto_protocol);
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, client, cfg, picker, auto_protocol).await;

    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    disable_raw_mode()?;
    terminal.show_cursor()?;

    result
}

async fn run(
    terminal: &mut ratatui::Terminal<CrosstermBackend<io::Stdout>>,
    client: Arc<LmsClient>,
    cfg: config::Config,
    mut picker: Picker,
    auto_protocol: ratatui_image::picker::ProtocolType,
) -> Result<()> {
    let mut cfg = cfg;
    let (tx, mut rx) = mpsc::channel::<AppMsg>(64);
    let (vol_sync_tx, mut vol_sync_rx) = mpsc::channel::<(String, u8)>(32);
    let mut app = App::new(cfg.default_player.clone());
    app.use_nerd_icons = cfg.use_nerd_icons;
    app.global_volume_control = cfg.global_volume_control;
    app.full_art_mode = cfg.full_art_mode;
    if app.full_art_mode {
        app.focus_sidebar = false;
    }
    app.disable_auto_colors = cfg.disable_auto_colors;
    app.accent_lightness = cfg.accent_lightness;
    // Compute Now Playing panel height: art column is 18 cols; height = ceil(18 * fw / fh) + 2 borders.
    // art_col_w is the actual cell width the square image fills (inner_h * fh / fw).
    let base_status_height;
    {
        let fs = picker.font_size();
        let art_rows = (18u16 * fs.width).div_ceil(fs.height);
        let inner_h = art_rows.saturating_sub(2);
        let art_col_w = (inner_h as u32 * fs.height as u32 / fs.width as u32) as u16;
        app.status_height = art_rows;
        app.art_col_w = art_col_w.max(4);
        app.font_size = (fs.width, fs.height);
        base_status_height = art_rows;
    }
    // Set initial dynamic status height from actual terminal size (avoids a per-frame poll).
    // After this, dimensions are updated only on resize events so the image area stays stable.
    if let Ok(sz) = terminal.size() {
        utils::update_status_height(&mut app, sz.height, base_status_height);
    }
    let mut album_art: Option<StatefulProtocol> = None;
    let mut album_art_full: Option<StatefulProtocol> = None;
    let mut last_artwork_image: Option<image::DynamicImage> = None;
    let mut last_artwork_url: Option<String> = None;
    let mut sidebar_state = ListState::default();
    let mut main_state = ListState::default();
    let mut last_main_click: Option<(Instant, usize)> = None;
    let mut thumbnails: HashMap<String, StatefulProtocol> = HashMap::new();
    let mut thumbnail_images: HashMap<String, image::DynamicImage> = HashMap::new();
    let mut pending_thumbs: HashSet<String> = HashSet::new();
    let mut failed_thumbs: HashSet<String> = HashSet::new();
    // Artist ids whose representative cover art is currently being resolved (see resolution loop).
    let mut pending_artist_art: HashSet<String> = HashSet::new();
    // Folder ids whose representative cover art is currently being resolved.
    let mut pending_folder_art: HashSet<u32> = HashSet::new();

    // Background: server health + player list polling
    {
        let c = client.clone();
        let t = tx.clone();
        tokio::spawn(async move {
            loop {
                match c.server_status().await {
                    Ok(_) => {
                        let _ = t.send(AppMsg::Connected).await;
                        if let Ok(players) = c.get_players().await {
                            let pids: Vec<String> =
                                players.iter().map(|p| p.playerid.clone()).collect();
                            let _ = t.send(AppMsg::PlayersLoaded(players)).await;
                            // Fan the per-player status calls out concurrently instead of
                            // awaiting them serially (N round-trips → ~1 round-trip latency).
                            let mut set = tokio::task::JoinSet::new();
                            for pid in pids {
                                let c2 = c.clone();
                                set.spawn(async move {
                                    c2.get_player_status_info(&pid)
                                        .await
                                        .ok()
                                        .map(|(vol, synced)| (pid, vol, synced))
                                });
                            }
                            let mut volumes = std::collections::HashMap::new();
                            let mut sync_groups = std::collections::HashMap::new();
                            while let Some(res) = set.join_next().await {
                                if let Ok(Some((pid, vol, synced))) = res {
                                    volumes.insert(pid.clone(), vol);
                                    sync_groups.insert(pid, synced);
                                }
                            }
                            if !volumes.is_empty() {
                                let _ = t.send(AppMsg::PlayerVolumesLoaded(volumes)).await;
                            }
                            if !sync_groups.is_empty() {
                                let _ = t.send(AppMsg::PlayerSyncGroupsLoaded(sync_groups)).await;
                            }
                        }
                    }
                    Err(_) => {
                        let _ = t.send(AppMsg::Disconnected).await;
                    }
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    // Background: library initial load
    {
        let c = client.clone();
        let t = tx.clone();
        tokio::spawn(async move {
            if let Ok(artists) = c.get_artists().await {
                let _ = t.send(AppMsg::ArtistsLoaded(artists)).await;
            }
            if let Ok(album_artists) = c.get_album_artists().await {
                let _ = t.send(AppMsg::AlbumArtistsLoaded(album_artists)).await;
            }
            if let Ok(playlists) = c.get_playlists().await {
                let _ = t.send(AppMsg::PlaylistsLoaded(playlists)).await;
            }
        });
    }

    // Background: debounced volume sync — coalesces rapid +/- keypresses into one API call
    {
        let c = client.clone();
        tokio::spawn(async move {
            use std::collections::HashMap;
            let debounce = Duration::from_millis(200);
            let far = || tokio::time::Instant::now() + Duration::from_secs(3600);
            let mut pending: HashMap<String, u8> = HashMap::new();
            let mut deadline = far();
            loop {
                tokio::select! {
                    msg = vol_sync_rx.recv() => match msg {
                        Some((pid, vol)) => {
                            pending.insert(pid, vol);
                            deadline = tokio::time::Instant::now() + debounce;
                        }
                        None => break,
                    },
                    _ = tokio::time::sleep_until(deadline) => {
                        for (pid, vol) in pending.drain() {
                            let c2 = c.clone();
                            tokio::spawn(async move { let _ = c2.set_volume(&pid, vol).await; });
                        }
                        deadline = far();
                    }
                }
            }
        });
    }

    let mut needs_redraw = true;
    loop {
        if needs_redraw {
            terminal.draw(|f| {
                ui::draw(
                    f,
                    &app,
                    album_art.as_mut(),
                    album_art_full.as_mut(),
                    &mut sidebar_state,
                    &mut main_state,
                    &mut thumbnails,
                    &cfg.host,
                    cfg.port,
                )
            })?;
        } // end if needs_redraw
        needs_redraw = false;

        // Drain all pending messages without blocking
        let mut had_msgs = false;
        while let Ok(msg) = rx.try_recv() {
            had_msgs = true;
            match msg {
                AppMsg::ArtworkDecoded { img, art_normal, art_full, accent, dimensions } => {
                    if let Some(c) = accent {
                        app.accent_color = Some(c);
                    }
                    app.art_image_size = Some(dimensions);
                    album_art = Some(picker.new_resize_protocol(art_normal));
                    album_art_full = Some(picker.new_resize_protocol(art_full));
                    last_artwork_image = Some(img);
                }
                AppMsg::ThumbnailLoaded(url, img, proto) => {
                    pending_thumbs.remove(&url);
                    thumbnail_images.insert(url.clone(), img);
                    thumbnails.insert(url, proto);
                }
                AppMsg::ThumbnailFailed(url) => {
                    pending_thumbs.remove(&url);
                    failed_thumbs.insert(url);
                }
                AppMsg::ArtworkFetchFailed(url) => {
                    if last_artwork_url.as_deref() == Some(url.as_str()) {
                        last_artwork_url = None;
                    }
                }
                AppMsg::ArtistArtworkResolved(artist_id, url) => {
                    pending_artist_art.remove(&artist_id);
                    // Cache `None` too, so an artist with no resolvable art is not re-queried.
                    app.artist_artwork.insert(artist_id, url);
                }
                AppMsg::FolderArtworkResolved(folder_id, url) => {
                    pending_folder_art.remove(&folder_id);
                    app.folder_artwork.insert(folder_id, url);
                }
                other => handle_msg(&mut app, other, &client, &tx).await,
            }
        }

        // Fetch artwork when the playing track changes
        let current_url = app
            .now_playing
            .as_ref()
            .and_then(|np| np.artwork_url.clone());
        if current_url != last_artwork_url {
            last_artwork_url = current_url.clone();
            album_art = None;
            last_artwork_image = None;
            // Only clear the image size when the new track has no artwork — if artwork is
            // coming, keep the old dimensions so the layout doesn't jump during the fetch.
            if current_url.is_none() {
                app.art_image_size = None;
            }
            // Keep the previous accent_color until the new image resolves.
            if let Some(url) = current_url {
                background::spawn_artwork_fetch(url, client.clone(), tx.clone());
            }
        }

        let term_h = terminal.size().map(|s| s.height).unwrap_or(24);

        // Resolve representative cover art for visible artists/folders. Resolved URLs land in
        // `app.artist_artwork` / `app.folder_artwork` and are then fetched by the thumbnail
        // prefetch block below.
        for idx in utils::thumb_range(term_h, &main_state, &app) {
            background::resolve_artist_art(&app, idx, &mut pending_artist_art, &client, &tx);
            background::resolve_folder_art(&app, idx, &mut pending_folder_art, &client, &tx);
        }

        // Request thumbnails for currently visible items
        {
            let base = client.server_base_url();
            for idx in utils::thumb_range(term_h, &main_state, &app) {
                if let Some(url) = utils::thumbnail_url_for(&app, idx, &base)
                    && !thumbnails.contains_key(&url)
                    && !pending_thumbs.contains(&url)
                    && !failed_thumbs.contains(&url)
                {
                    pending_thumbs.insert(url.clone());
                    let c = client.clone();
                    let t = tx.clone();
                    let u = url.clone();
                    let pk = picker.clone();
                    let (fw, fh) = app.font_size;
                    let target_w = (crate::ui::THUMB_W as u32 * fw as u32).max(1);
                    let target_h = (2u32 * fh as u32).max(1);
                    tokio::spawn(async move {
                        match c.fetch_image_bytes(&u).await {
                            Ok(bytes) => match image::load_from_memory(&bytes) {
                                Ok(img) => {
                                    let small = img.resize(
                                        target_w,
                                        target_h,
                                        image::imageops::FilterType::Triangle,
                                    );
                                    // Fully resize+encode the protocol off the UI thread so the
                                    // draw call never blocks: at render time needs_resize is None.
                                    let proto = artwork::encode_thumb_protocol(&pk, small.clone());
                                    let _ = t.send(AppMsg::ThumbnailLoaded(u, small, proto)).await;
                                }
                                Err(_) => {
                                    let _ = t.send(AppMsg::ThumbnailFailed(u)).await;
                                }
                            },
                            Err(_) => {
                                let _ = t.send(AppMsg::ThumbnailFailed(u)).await;
                            }
                        }
                    });
                }
            }
        }

        let had_overlay = utils::has_overlay(&app);

        match poll_event(TICK_RATE)? {
            InputEvent::Key(key) => {
                if app.config_modal.is_some() {
                    let prev_protocol = cfg.image_protocol.clone();
                    handlers::handle_config_key(&mut app, key, &mut cfg, &client, &tx);
                    if cfg.image_protocol != prev_protocol {
                        artwork::apply_image_protocol(&mut picker, &cfg.image_protocol, auto_protocol);
                        artwork::refresh_album_art(
                            &last_artwork_image,
                            &mut picker,
                            &mut album_art,
                            &mut album_art_full,
                        );
                        thumbnails = artwork::rebuild_all_thumbnails(&thumbnail_images, &mut picker);
                    }
                } else if app.sync_modal.is_some() {
                    handlers::handle_sync_modal_key(&mut app, key, &client).await;
                } else if app.confirm_quit {
                    handlers::handle_confirm_quit_key(&mut app, key).await;
                    if app.should_quit {
                        break;
                    }
                } else if app.confirm_clear_queue {
                    handlers::handle_confirm_clear_queue_key(&mut app, key, &client, &tx).await;
                } else if app.confirm_delete_queue_item.is_some() {
                    handlers::handle_confirm_delete_queue_item_key(&mut app, key, &client, &tx)
                        .await;
                } else if app.context_menu.is_some() {
                    handlers::handle_context_menu_key(&mut app, key, &client, &tx).await;
                } else if matches!(app.main_view, MainView::Search) && app.search_input_active {
                    handlers::handle_search_input_key(&mut app, key, &client, &tx).await;
                } else if matches!(app.main_view, MainView::AppSearch { .. })
                    && app.app_search_input_active
                {
                    handlers::handle_app_search_input_key(&mut app, key, &client, &tx).await;
                } else if matches!(app.main_view, MainView::Players)
                    && !app.focus_sidebar
                    && matches!(key.code, crossterm::event::KeyCode::Char('s'))
                {
                    let idx = app.main_selected;
                    handlers::open_sync_modal(&mut app, idx);
                } else {
                    if key.code == crossterm::event::KeyCode::Esc {
                        let double = app
                            .esc_last_pressed
                            .take()
                            .map(|t| t.elapsed() < Duration::from_millis(500))
                            .unwrap_or(false);
                        if double {
                            app.confirm_quit = true;
                            app.quit_selected_button = 1;
                            needs_redraw = true;
                            continue;
                        }
                        app.esc_last_pressed = Some(Instant::now());
                    } else {
                        app.esc_last_pressed = None;
                    }
                    let action = key_to_action(key);
                    if matches!(action, Action::OpenConfig) {
                        app.config_modal = Some(ConfigModal::new(
                            &cfg.host,
                            cfg.port,
                            cfg.username.as_deref(),
                            cfg.password.as_deref(),
                            cfg.use_nerd_icons,
                            cfg.auto_discover,
                            &cfg.broadcast_mask,
                            cfg.disable_auto_colors,
                            cfg.accent_lightness,
                            &cfg.image_protocol,
                        ));
                    } else {
                        let prev_gvc = app.global_volume_control;
                        let prev_art = app.full_art_mode;
                        if handlers::handle_action(&mut app, action, &client, &tx, &vol_sync_tx)
                            .await
                        {
                            break;
                        }
                        if app.global_volume_control != prev_gvc {
                            cfg.global_volume_control = app.global_volume_control;
                            let _ = cfg.save();
                        }
                        if app.full_art_mode != prev_art {
                            cfg.full_art_mode = app.full_art_mode;
                            let _ = cfg.save();
                            // Render area size changes between modes; recreate protocol so the
                            // image is retransmitted at the correct dimensions.
                            artwork::refresh_album_art(
                                &last_artwork_image,
                                &mut picker,
                                &mut album_art,
                                &mut album_art_full,
                            );
                        }
                    }
                }
                needs_redraw = true;
            }
            InputEvent::Mouse(mouse) => {
                let area = terminal.size()?.into();
                handlers::handle_mouse_event(
                    &mut app,
                    mouse,
                    &client,
                    &tx,
                    &vol_sync_tx,
                    area,
                    &sidebar_state,
                    &main_state,
                    &mut last_main_click,
                    &mut cfg,
                )
                .await;
                if app.should_quit {
                    break;
                }
                needs_redraw = true;
            }
            InputEvent::Resize => {
                if let Ok(sz) = terminal.size() {
                    utils::update_status_height(&mut app, sz.height, base_status_height);
                }
                needs_redraw = true;
            }
            InputEvent::Tick => {
                // Only redraw on tick if new data arrived; prevents Sixel retransmission
                // flicker and Kitty blank-frame blinks when nothing has actually changed.
                if had_msgs {
                    needs_redraw = true;
                }
            }
            InputEvent::Disconnected => break,
        }

        // When an overlay closes its Clear widget may overwrite image cells, causing terminals
        // to discard stored graphic-protocol data. Recreate affected protocols on overlay close.
        if had_overlay && !utils::has_overlay(&app) {
            let base = client.server_base_url();
            for idx in utils::thumb_range(term_h, &main_state, &app) {
                if let Some(url) = utils::thumbnail_url_for(&app, idx, &base)
                    && let Some(img) = thumbnail_images.get(&url)
                {
                    thumbnails.insert(url, picker.new_resize_protocol(img.clone()));
                }
            }
            artwork::refresh_album_art(
                &last_artwork_image,
                &mut picker,
                &mut album_art,
                &mut album_art_full,
            );
            needs_redraw = true;
        }
    }

    if app.active_player != cfg.default_player {
        cfg.default_player = app.active_player.clone();
        let _ = cfg.save();
    }

    Ok(())
}

async fn handle_msg(
    app: &mut App,
    msg: AppMsg,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    use app::ConnectionState;
    match msg {
        AppMsg::Connected => {
            let was_connected = app.connection == ConnectionState::Connected;
            app.connection = ConnectionState::Connected;
            if app.status_message.as_deref() == Some("Reconnecting...") {
                app.status_message = None;
            }
            if !was_connected {
                background::load_app_services(client.clone(), tx.clone());
                background::load_radio_services(client.clone(), tx.clone());
            }
        }
        AppMsg::Disconnected => app.connection = ConnectionState::Disconnected,
        AppMsg::PlayersLoaded(players) => {
            let new_pid = if let Some(pid) = app.active_player.clone() {
                if players.iter().any(|p| p.playerid == pid) {
                    Some(pid)
                } else {
                    players.first().map(|p| p.playerid.clone())
                }
            } else {
                players.first().map(|p| p.playerid.clone())
            };

            if let Some(pid) = new_pid {
                if app.active_player.as_deref() != Some(&pid) || app.now_playing_handle.is_none() {
                    if let Some(h) = app.now_playing_handle.take() {
                        h.abort();
                    }
                    app.now_playing_handle = Some(background::start_now_playing_loop(
                        pid.clone(),
                        client.clone(),
                        tx.clone(),
                    ));
                }
                app.active_player = Some(pid);
            }
            app.players = players;
        }
        AppMsg::NowPlayingUpdated(pid, mut np) => {
            if app.active_player.as_deref() == Some(pid.as_str()) {
                // Preserve locally-pending volume so the 500ms poll doesn't overwrite
                // an optimistic mute/unmute before the server has processed the command.
                if app.volume_pending.contains_key(&pid)
                    && let Some(cur) = app.now_playing.as_ref()
                {
                    np.volume = cur.volume;
                }
                app.now_playing = Some(np);
            }
        }
        AppMsg::QueueLoaded(pid, q) => {
            if app.active_player.as_deref() == Some(pid.as_str()) {
                app.queue = q;
            }
        }
        AppMsg::ArtistsLoaded(a) => app.artists = a,
        AppMsg::AlbumArtistsLoaded(a) => app.album_artists = a,
        AppMsg::PlaylistsLoaded(p) => { app.playlists = p; app.is_loading = false; }
        AppMsg::AlbumsLoaded(a) => { app.albums = a; app.is_loading = false; }
        AppMsg::TracksLoaded(t) => { app.tracks = t; app.is_loading = false; }
        AppMsg::RecentArtistsLoaded(a) => { app.recent_artists = a; app.is_loading = false; }
        AppMsg::PopularAlbumsLoaded(a) => { app.popular_albums = a; app.is_loading = false; }
        AppMsg::RadioItemsLoaded(items) => {
            if app.radio_nav_stack.is_empty() {
                app.radio_services = items.clone();
            }
            app.radio_items = items;
            if matches!(app.main_view, MainView::Radio) {
                app.main_selected = 0;
            }
            app.is_loading = false;
        }
        AppMsg::AppItemsLoaded(items) => {
            if app.app_nav_stack.is_empty() {
                app.app_services = items.clone();
            }
            app.app_items = items;
            if matches!(app.main_view, MainView::Apps) {
                app.main_selected = 0;
            }
            app.is_loading = false;
        }
        AppMsg::FavItemsLoaded(items) => { app.fav_items = items; app.main_selected = 0; app.is_loading = false; }
        AppMsg::FolderItemsLoaded(items) => { app.folder_items = items; app.main_selected = 0; app.is_loading = false; }
        AppMsg::PlayerVolumesLoaded(volumes) => {
            let now = std::time::Instant::now();
            // Drop stale pending entries, then skip pids still within the guard window
            app.volume_pending
                .retain(|_, t| now.duration_since(*t).as_secs() < 3);
            for (pid, vol) in volumes {
                if !app.volume_pending.contains_key(&pid) {
                    app.player_volumes.insert(pid, vol);
                }
            }
        }
        AppMsg::PlayerSyncGroupsLoaded(groups) => {
            app.player_sync_groups = groups;
        }
        AppMsg::StatusMsg(msg) => background::set_timed_status(app, msg, tx),
        AppMsg::ClearStatusMsg(seq) => {
            if app.status_message_gen == seq {
                app.status_message = None;
            }
        }
        AppMsg::SearchResultsLoaded(results) => { app.search_results = results; app.main_selected = 0; app.is_loading = false; }
        AppMsg::AppSearchResultsLoaded(items) => { app.app_search_results = items; app.main_selected = 0; app.is_loading = false; }
        AppMsg::Error(e) => background::set_timed_status(app, e, tx),
        AppMsg::DiscoveredServers(servers) => {
            if let Some(modal) = app.config_modal.as_mut() {
                modal.is_scanning = false;
                modal.discovered_servers = servers;
                // Move focus to first discovered server (or back to scan button if none found).
                modal.selected_field = 7;
            }
        }
        AppMsg::ArtworkDecoded { .. }
        | AppMsg::ThumbnailLoaded(..)
        | AppMsg::ThumbnailFailed(_)
        | AppMsg::ArtworkFetchFailed(_)
        | AppMsg::ArtistArtworkResolved(..)
        | AppMsg::FolderArtworkResolved(..) => {
            // handled inline in the event loop
        }
    }
}
