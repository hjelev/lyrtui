mod api;
mod app;
mod background;
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
use ratatui_image::{
    Resize, ResizeEncodeRender,
    picker::{Picker, ProtocolType},
    protocol::StatefulProtocol,
};
use std::{
    collections::{HashMap, HashSet},
    io,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

async fn print_info() -> Result<()> {
    let cfg = config::Config::load()?;
    let config_path = config::config_path();

    println!("lyrtui v{} — Info", env!("CARGO_PKG_VERSION"));

    // ── Configuration ──────────────────────────────────────────────────────────
    println!("\nCONFIGURATION  ({})", config_path.display());
    println!("  server:          {}:{}", cfg.host, cfg.port);
    println!("  url:             {}", cfg.base_url());
    match (&cfg.username, &cfg.password) {
        (Some(u), Some(_)) => println!("  auth:            username={}, password=set", u),
        (Some(u), None) => println!("  auth:            username={}, password=not set", u),
        _ => println!("  auth:            none"),
    }
    match &cfg.default_player {
        Some(id) => println!("  default player:  {}", id),
        None => println!("  default player:  none"),
    }
    println!(
        "  auto-discover:   {} (mask: {})",
        yn(cfg.auto_discover),
        cfg.broadcast_mask
    );
    println!("  nerd icons:      {}", yn(cfg.use_nerd_icons));
    println!("  image protocol:  {}", cfg.image_protocol);
    println!("  full art mode:   {}", yn(cfg.full_art_mode));
    println!("  auto colors:     {}", yn(!cfg.disable_auto_colors));
    println!("  global volume:   {}", yn(cfg.global_volume_control));

    let client = LmsClient::new(cfg.base_url(), cfg.credentials());

    // ── Server ─────────────────────────────────────────────────────────────────
    println!("\nSERVER  (live)");
    match client.get_server_info().await {
        Err(e) => println!("  [unreachable: {}]", e),
        Ok(info) => {
            if let Some(v) = &info.version {
                println!("  version:         {}", v);
            }
            if let Some(n) = &info.name {
                println!("  name:            {}", n);
            }
            if let Some(ip) = &info.ip {
                println!("  ip:              {}", ip);
            }
            if let Some(m) = &info.mac {
                println!("  mac:             {}", m);
            }
            if let Some(id) = &info.uuid {
                println!("  uuid:            {}", id);
            }
            if let Some(c) = info.player_count {
                println!("  players:         {}", c);
            }
        }
    }

    // ── Players ────────────────────────────────────────────────────────────────
    let players = match client.get_players_detailed().await {
        Err(e) => {
            println!("\nPLAYERS\n  [unreachable: {}]", e);
            return Ok(());
        }
        Ok(p) => p,
    };

    println!("\nPLAYERS  ({})", players.len());

    for (i, p) in players.iter().enumerate() {
        let status = if p.power == 0 {
            "OFF".to_string()
        } else if p.is_playing == 1 {
            "PLAYING".to_string()
        } else {
            "STOPPED".to_string()
        };
        let indicator = if p.power == 0 {
            "○"
        } else if p.is_playing == 1 {
            "▶"
        } else {
            "■"
        };
        println!("\n  [{}] {:<38} {} {}", i + 1, p.name, indicator, status);
        println!("      id:          {}", p.playerid);
        if let Some(ip) = &p.ip {
            println!("      ip:          {}", ip);
        }
        match (&p.model, &p.modelname) {
            (Some(m), Some(mn)) => println!("      model:       {} ({})", m, mn),
            (Some(m), None) => println!("      model:       {}", m),
            _ => {}
        }
        if let Some(fw) = &p.firmware {
            println!("      firmware:    {}", fw);
        }
        if let Some(uid) = &p.uuid
            && !uid.is_empty()
        {
            println!("      uuid:        {}", uid);
        }
        println!(
            "      power:       {}    connected: {}",
            yn(p.power == 1),
            yn(p.connected == 1)
        );

        if p.power == 1 {
            match client.get_now_playing(&p.playerid).await {
                Ok(np) => {
                    println!("      volume:      {}", np.volume);
                    if !np.title.is_empty() {
                        println!("      now:         \"{}\" — {}", np.title, np.artist);
                        let mut meta = np.album.clone();
                        if let Some(y) = np.year {
                            meta = format!("{} ({})", meta, y);
                        }
                        if let (Some(idx), Some(total)) =
                            (np.playlist_cur_index, np.playlist_tracks)
                        {
                            meta = format!("{}  [track {} / {}]", meta, idx + 1, total);
                        }
                        if !meta.is_empty() {
                            println!("                   {}", meta);
                        }
                    } else {
                        println!("      now:         (nothing playing)");
                    }
                }
                Err(e) => println!("      status:      [error: {}]", e),
            }
        }
    }

    Ok(())
}

fn yn(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

async fn resolve_player(client: &LmsClient, cfg: &config::Config) -> Result<String> {
    if let Some(id) = &cfg.default_player {
        return Ok(id.clone());
    }
    let players = client.get_players().await?;
    players
        .into_iter()
        .next()
        .map(|p| p.playerid)
        .ok_or_else(|| anyhow::anyhow!("no players found on server"))
}

fn format_track(title: &str, artist: &str) -> String {
    match (title.is_empty(), artist.is_empty()) {
        (true, _) => "(unknown)".to_string(),
        (false, true) => format!("\"{}\"", title),
        (false, false) => format!("\"{}\" — {}", title, artist),
    }
}

/// Load config and resolve the default player for one-shot CLI commands.
async fn cli_player() -> Result<(LmsClient, String)> {
    let cfg = config::Config::load()?;
    let client = LmsClient::new(cfg.base_url(), cfg.credentials());
    let pid = resolve_player(&client, &cfg).await?;
    Ok((client, pid))
}

async fn cmd_play_pause() -> Result<()> {
    let (client, pid) = cli_player().await?;
    let np = client.get_now_playing(&pid).await?;
    let track = format_track(&np.title, &np.artist);
    if np.is_playing {
        client.pause(&pid).await?;
        println!("paused  {}", track);
    } else {
        client.play(&pid).await?;
        println!("playing {}", track);
    }
    Ok(())
}

async fn cmd_next() -> Result<()> {
    let (client, pid) = cli_player().await?;
    client.next(&pid).await?;
    if let Ok(np) = client.get_now_playing(&pid).await {
        println!("next  {}", format_track(&np.title, &np.artist));
    }
    Ok(())
}

async fn cmd_prev() -> Result<()> {
    let (client, pid) = cli_player().await?;
    client.prev(&pid).await?;
    if let Ok(np) = client.get_now_playing(&pid).await {
        println!("prev  {}", format_track(&np.title, &np.artist));
    }
    Ok(())
}

const TICK_RATE: Duration = Duration::from_millis(250);
const ART_RADIUS_NORMAL: u32 = 6;
const ART_RADIUS_FULL: u32 = 2;

#[tokio::main]
async fn main() -> Result<()> {
    let args: std::collections::HashSet<String> = std::env::args().collect();
    let has = |flags: &[&str]| flags.iter().any(|f| args.contains(*f));

    if has(&["-v", "--version"]) {
        println!("lyrtui v{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    if has(&["-i", "--info"]) {
        return print_info().await;
    }

    if has(&["-p", "--play-pause"]) {
        return cmd_play_pause().await;
    }

    if has(&["--next"]) {
        return cmd_next().await;
    }

    if has(&["--prev"]) {
        return cmd_prev().await;
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
        && let Some(discovered_ip) =
            discovery::discover_lms(&cfg.broadcast_mask, Duration::from_secs(2))
    {
        cfg.host = discovered_ip;
    }

    let client = Arc::new(LmsClient::new(cfg.base_url(), cfg.credentials()));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    // Picker must be created after EnterAlternateScreen, before reading events.
    let mut picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
    apply_image_protocol(&mut picker, &cfg.image_protocol);
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, client, cfg, picker).await;

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
        update_status_height(&mut app, sz.height, base_status_height);
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
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    let ok = async {
                        let bytes = c.fetch_image_bytes(&url).await.ok()?;
                        let img = image::load_from_memory(&bytes).ok()?;
                        let rgb = img.to_rgb8();
                        let accent = color_thief::get_palette(
                            rgb.as_raw(),
                            color_thief::ColorFormat::Rgb,
                            10,
                            5,
                        )
                        .ok()
                        .and_then(|colors| {
                            colors
                                .iter()
                                .find(|c| {
                                    let luma = (c.r as u32 * 299
                                        + c.g as u32 * 587
                                        + c.b as u32 * 114)
                                        / 1000;
                                    (70..=210).contains(&luma)
                                })
                                .or_else(|| colors.first())
                                .map(|c| [c.r, c.g, c.b])
                        });
                        let dimensions = (img.width(), img.height());
                        let art_normal = with_rounded_corners(img.clone(), ART_RADIUS_NORMAL);
                        let art_full = with_rounded_corners(img.clone(), ART_RADIUS_FULL);
                        let _ = t
                            .send(AppMsg::ArtworkDecoded {
                                img,
                                art_normal,
                                art_full,
                                accent,
                                dimensions,
                            })
                            .await;
                        Some(())
                    }
                    .await;
                    if ok.is_none() {
                        let _ = t.send(AppMsg::ArtworkFetchFailed(url)).await;
                    }
                });
            }
        }

        let term_h = terminal.size().map(|s| s.height).unwrap_or(24);

        // Resolve representative cover art for visible artists (artists carry no art of their
        // own; we look up the coverid of an album they appear on). Resolved URLs land in
        // `app.artist_artwork` and are then fetched by the thumbnail prefetch block below.
        {
            for idx in thumb_range(term_h, &main_state, &app) {
                if let Some(artist_id) = utils::artist_id_at(&app, idx)
                    && !app.artist_artwork.contains_key(&artist_id)
                    && !pending_artist_art.contains(&artist_id)
                {
                    pending_artist_art.insert(artist_id.clone());
                    let c = client.clone();
                    let t = tx.clone();
                    tokio::spawn(async move {
                        let url = c.get_artist_artwork(&artist_id).await;
                        let _ = t.send(AppMsg::ArtistArtworkResolved(artist_id, url)).await;
                    });
                }
                if let Some(folder_id) = utils::folder_id_at(&app, idx)
                    && !app.folder_artwork.contains_key(&folder_id)
                    && !pending_folder_art.contains(&folder_id)
                {
                    pending_folder_art.insert(folder_id);
                    let c = client.clone();
                    let t = tx.clone();
                    tokio::spawn(async move {
                        let url = c.get_folder_artwork(folder_id).await;
                        let _ = t.send(AppMsg::FolderArtworkResolved(folder_id, url)).await;
                    });
                }
            }
        }

        // Request thumbnails for currently visible items
        {
            let base = client.server_base_url();
            for idx in thumb_range(term_h, &main_state, &app) {
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
                                    let proto = encode_thumb_protocol(&pk, small.clone());
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

        let had_overlay = has_overlay(&app);

        match poll_event(TICK_RATE)? {
            InputEvent::Key(key) => {
                if app.config_modal.is_some() {
                    let prev_protocol = cfg.image_protocol.clone();
                    handlers::handle_config_key(&mut app, key, &mut cfg, &client, &tx);
                    if cfg.image_protocol != prev_protocol {
                        apply_image_protocol(&mut picker, &cfg.image_protocol);
                        refresh_album_art(
                            &last_artwork_image,
                            &mut picker,
                            &mut album_art,
                            &mut album_art_full,
                        );
                        thumbnails = thumbnail_images
                            .iter()
                            .map(|(url, img)| {
                                (url.clone(), picker.new_resize_protocol(img.clone()))
                            })
                            .collect();
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
                            refresh_album_art(
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
                    update_status_height(&mut app, sz.height, base_status_height);
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
        }

        // When an overlay closes its Clear widget may overwrite image cells, causing terminals
        // to discard stored graphic-protocol data. Recreate affected protocols on overlay close.
        if had_overlay && !has_overlay(&app) {
            let base = client.server_base_url();
            for idx in thumb_range(term_h, &main_state, &app) {
                if let Some(url) = utils::thumbnail_url_for(&app, idx, &base)
                    && let Some(img) = thumbnail_images.get(&url)
                {
                    thumbnails.insert(url, picker.new_resize_protocol(img.clone()));
                }
            }
            refresh_album_art(
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
        AppMsg::PlaylistsLoaded(p) => {
            app.playlists = p;
            app.is_loading = false;
        }
        AppMsg::AlbumsLoaded(a) => {
            app.albums = a;
            app.is_loading = false;
        }
        AppMsg::TracksLoaded(t) => {
            app.tracks = t;
            app.is_loading = false;
        }
        AppMsg::RecentArtistsLoaded(a) => {
            app.recent_artists = a;
            app.is_loading = false;
        }
        AppMsg::PopularAlbumsLoaded(a) => {
            app.popular_albums = a;
            app.is_loading = false;
        }
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
        AppMsg::FavItemsLoaded(items) => {
            app.fav_items = items;
            app.main_selected = 0;
            app.is_loading = false;
        }
        AppMsg::FolderItemsLoaded(items) => {
            app.folder_items = items;
            app.main_selected = 0;
            app.is_loading = false;
        }
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
        AppMsg::StatusMsg(msg) => {
            set_timed_status(app, msg, tx);
        }
        AppMsg::ClearStatusMsg(seq) => {
            if app.status_message_gen == seq {
                app.status_message = None;
            }
        }
        AppMsg::SearchResultsLoaded(results) => {
            app.search_results = results;
            app.main_selected = 0;
            app.is_loading = false;
        }
        AppMsg::AppSearchResultsLoaded(items) => {
            app.app_search_results = items;
            app.main_selected = 0;
            app.is_loading = false;
        }
        AppMsg::Error(e) => {
            set_timed_status(app, e, tx);
        }
        AppMsg::DiscoveredServers(servers) => {
            if let Some(modal) = app.config_modal.as_mut() {
                modal.is_scanning = false;
                modal.discovered_servers = servers;
                // Move focus to first discovered server (or back to scan button if none found).
                modal.selected_field = 7;
            }
        }
        AppMsg::ArtistArtworkResolved(artist_id, url) => {
            // `None` is cached too, so we don't re-query an artist that has no resolvable art.
            app.artist_artwork.insert(artist_id, url);
        }
        AppMsg::FolderArtworkResolved(folder_id, url) => {
            app.folder_artwork.insert(folder_id, url);
        }
        AppMsg::ArtworkDecoded { .. }
        | AppMsg::ThumbnailLoaded(..)
        | AppMsg::ThumbnailFailed(_)
        | AppMsg::ArtworkFetchFailed(_) => {
            // handled inline in the event loop
        }
    }
}

fn thumb_range(
    term_h: u16,
    state: &ratatui::widgets::ListState,
    app: &App,
) -> std::ops::Range<usize> {
    let inner_h = term_h.saturating_sub(13);
    let visible = ((inner_h / 2) as usize).max(1);
    let offset = state.offset();
    let end = (offset + visible + 5).min(utils::main_list_len(app));
    offset..end
}

fn has_overlay(app: &App) -> bool {
    app.confirm_delete_queue_item.is_some()
        || app.confirm_clear_queue
        || app.confirm_quit
        || app.config_modal.is_some()
        || app.context_menu.is_some()
        || app.sync_modal.is_some()
}

fn apply_image_protocol(picker: &mut Picker, protocol: &str) {
    match protocol {
        "halfblocks" => picker.set_protocol_type(ProtocolType::Halfblocks),
        "sixel" => picker.set_protocol_type(ProtocolType::Sixel),
        "kitty" => picker.set_protocol_type(ProtocolType::Kitty),
        "iterm2" => picker.set_protocol_type(ProtocolType::Iterm2),
        _ => {
            // "auto" or unknown: on Windows, terminal graphics protocols aren't supported
            if cfg!(target_os = "windows") {
                picker.set_protocol_type(ProtocolType::Halfblocks);
            }
        }
    }
}

/// Build a thumbnail protocol and fully resize+encode it for the fixed `THUMB_W × 2` cell
/// thumbnail rect. Doing the encode here (off the UI thread when called from a spawned task)
/// means draw-time `needs_resize` returns `None`, so rendering never blocks on encoding.
fn encode_thumb_protocol(picker: &Picker, img: image::DynamicImage) -> StatefulProtocol {
    let mut proto = picker.new_resize_protocol(img);
    let area = ratatui::layout::Size {
        width: crate::ui::THUMB_W,
        height: 2,
    };
    if let Some(sz) = proto.needs_resize(&Resize::Fit(None), area) {
        proto.resize_encode(&Resize::Fit(None), sz);
    }
    proto
}

fn create_album_art_protocols(
    img: &image::DynamicImage,
    picker: &mut Picker,
) -> (Option<StatefulProtocol>, Option<StatefulProtocol>) {
    (
        Some(picker.new_resize_protocol(with_rounded_corners(img.clone(), ART_RADIUS_NORMAL))),
        Some(picker.new_resize_protocol(with_rounded_corners(img.clone(), ART_RADIUS_FULL))),
    )
}

/// Recreate the normal/full album-art protocols from the cached image (if any), forcing the
/// terminal to retransmit at current dimensions. No-op when no artwork is cached.
fn refresh_album_art(
    last_artwork_image: &Option<image::DynamicImage>,
    picker: &mut Picker,
    album_art: &mut Option<StatefulProtocol>,
    album_art_full: &mut Option<StatefulProtocol>,
) {
    if let Some(img) = last_artwork_image {
        (*album_art, *album_art_full) = create_album_art_protocols(img, picker);
    }
}

fn update_status_height(app: &mut App, term_height: u16, base_height: u16) {
    let fw = app.font_size.0.max(1) as u32;
    let fh = app.font_size.1.max(1) as u32;
    let dyn_sh = (term_height / 3).max(base_height);
    app.status_height = dyn_sh;
    let inner_h = dyn_sh.saturating_sub(2);
    app.art_col_w = ((inner_h as u32 * fh) / fw).max(4) as u16;
}

fn set_timed_status(app: &mut App, msg: String, tx: &mpsc::Sender<AppMsg>) {
    app.status_message_gen += 1;
    let seq = app.status_message_gen;
    app.status_message = Some(msg);
    let t = tx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(4)).await;
        let _ = t.send(AppMsg::ClearStatusMsg(seq)).await;
    });
}

/// Round the corners of an image by making corner pixels transparent.
/// `radius_pct` is the radius as a percentage of the shorter dimension (clamped to ≥4 px).
fn with_rounded_corners(img: image::DynamicImage, radius_pct: u32) -> image::DynamicImage {
    let mut rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let r = ((w.min(h) * radius_pct / 100) as f64).max(4.0);
    for y in 0..h {
        for x in 0..w {
            let corner = match (
                x < r as u32,
                x >= w.saturating_sub(r as u32),
                y < r as u32,
                y >= h.saturating_sub(r as u32),
            ) {
                (true, _, true, _) => Some((r as u32, r as u32)),
                (_, true, true, _) => Some((w - r as u32, r as u32)),
                (true, _, _, true) => Some((r as u32, h - r as u32)),
                (_, true, _, true) => Some((w - r as u32, h - r as u32)),
                _ => None,
            };
            if let Some((cx, cy)) = corner {
                let dx = x as f64 - cx as f64;
                let dy = y as f64 - cy as f64;
                if dx * dx + dy * dy > r * r {
                    rgba.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
                }
            }
        }
    }
    image::DynamicImage::ImageRgba8(rgba)
}
