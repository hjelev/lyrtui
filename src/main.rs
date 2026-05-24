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
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use events::{key_to_action, poll_event, Action, InputEvent};
use ratatui::{backend::CrosstermBackend, widgets::ListState, Terminal};
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
    let mut cfg = config::Config::load()?;

    // Run discovery if this is a first run (no config file) or auto_discover is on.
    let config_file_exists = config::config_path().exists();
    if (!config_file_exists || cfg.auto_discover)
        && let Some(discovered_ip) = discovery::discover_lms(
            &cfg.broadcast_mask,
            Duration::from_secs(2),
        )
    {
        cfg.host = discovered_ip;
    }

    let credentials = cfg.username.as_ref()
        .zip(cfg.password.as_ref())
        .map(|(u, p)| (u.clone(), p.clone()));
    let client = Arc::new(LmsClient::new(cfg.base_url(), credentials));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    // Picker must be created after EnterAlternateScreen, before reading events.
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, client, cfg, picker).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    result
}

async fn run(
    terminal: &mut ratatui::Terminal<CrosstermBackend<io::Stdout>>,
    client: Arc<LmsClient>,
    cfg: config::Config,
    picker: Picker,
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
    {
        let fs = picker.font_size();
        let art_rows = (18u16 * fs.width).div_ceil(fs.height);
        let inner_h = art_rows.saturating_sub(2);
        let art_col_w = (inner_h as u32 * fs.height as u32 / fs.width as u32) as u16;
        app.status_height = art_rows;
        app.art_col_w = art_col_w.max(4);
    }
    let mut album_art: Option<StatefulProtocol> = None;
    let mut last_artwork_url: Option<String> = None;
    let mut sidebar_state = ListState::default();
    let mut main_state = ListState::default();
    let mut last_main_click: Option<(Instant, usize)> = None;
    let mut thumbnails: HashMap<String, StatefulProtocol> = HashMap::new();
    let mut pending_thumbs: HashSet<String> = HashSet::new();
    let mut failed_thumbs: HashSet<String> = HashSet::new();

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
                            let mut volumes = std::collections::HashMap::new();
                            for pid in pids {
                                if let Ok(vol) = c.get_player_volume(&pid).await {
                                    volumes.insert(pid, vol);
                                }
                            }
                            if !volumes.is_empty() {
                                let _ = t.send(AppMsg::PlayerVolumesLoaded(volumes)).await;
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

    loop {
        terminal.draw(|f| {
            ui::draw(
                f,
                &app,
                album_art.as_mut(),
                &mut sidebar_state,
                &mut main_state,
                &mut thumbnails,
                &cfg.host,
                cfg.port,
            )
        })?;

        // Drain all pending messages without blocking
        while let Ok(msg) = rx.try_recv() {
            match msg {
                AppMsg::ArtworkLoaded(bytes) => {
                    if let Ok(img) = image::load_from_memory(&bytes) {
                        let rgb = img.to_rgb8();
                        if let Ok(colors) = color_thief::get_palette(rgb.as_raw(), color_thief::ColorFormat::Rgb, 10, 5) {
                            // Pick the first palette color with usable brightness:
                            // - not too dark (unreadable on dark bg, and black text unreadable on it as bg)
                            // - not too light (washed out / near white)
                            let picked = colors.iter().find(|c| {
                                let luma = (c.r as u32 * 299 + c.g as u32 * 587 + c.b as u32 * 114) / 1000;
                                (70..=210).contains(&luma)
                            }).or_else(|| colors.first());
                            if let Some(c) = picked {
                                app.accent_color = Some([c.r, c.g, c.b]);
                            }
                        }
                        album_art = Some(picker.new_resize_protocol(img));
                    }
                }
                AppMsg::ThumbnailLoaded(url, bytes) => {
                    pending_thumbs.remove(&url);
                    if let Ok(img) = image::load_from_memory(&bytes) {
                        thumbnails.insert(url, picker.new_resize_protocol(img));
                    } else {
                        failed_thumbs.insert(url);
                    }
                }
                AppMsg::ThumbnailFailed(url) => {
                    pending_thumbs.remove(&url);
                    failed_thumbs.insert(url);
                }
                other => handle_msg(&mut app, other, &client, &tx).await,
            }
        }

        // Fetch artwork when the playing track changes
        let current_url = app.now_playing.as_ref().and_then(|np| np.artwork_url.clone());
        if current_url != last_artwork_url {
            last_artwork_url = current_url.clone();
            album_art = None;
            // Keep the previous accent_color until the new image resolves.
            if let Some(url) = current_url {
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    if let Ok(bytes) = c.fetch_image_bytes(&url).await {
                        let _ = t.send(AppMsg::ArtworkLoaded(bytes)).await;
                    }
                });
            }
        }

        // Request thumbnails for currently visible items
        {
            let term_h = terminal.size().map(|s| s.height).unwrap_or(24);
            let inner_h = term_h.saturating_sub(13);
            let visible = ((inner_h / 2) as usize).max(1);
            let offset = main_state.offset();
            let end = (offset + visible + 5).min(utils::main_list_len(&app));
            let base = client.server_base_url();
            for idx in offset..end {
                if let Some(url) = utils::thumbnail_url_for(&app, idx, &base)
                    && !thumbnails.contains_key(&url)
                    && !pending_thumbs.contains(&url)
                    && !failed_thumbs.contains(&url)
                {
                    pending_thumbs.insert(url.clone());
                    let c = client.clone();
                    let t = tx.clone();
                    let u = url.clone();
                    tokio::spawn(async move {
                        if let Ok(bytes) = c.fetch_image_bytes(&u).await {
                            let _ = t.send(AppMsg::ThumbnailLoaded(u, bytes)).await;
                        } else {
                            let _ = t.send(AppMsg::ThumbnailFailed(u)).await;
                        }
                    });
                }
            }
        }

        match poll_event(TICK_RATE)? {
            InputEvent::Key(key) => {
                if app.config_modal.is_some() {
                    handlers::handle_config_key(&mut app, key, &mut cfg, &client);
                } else if app.confirm_clear_queue {
                    handlers::handle_confirm_clear_queue_key(&mut app, key, &client, &tx).await;
                } else if app.context_menu.is_some() {
                    handlers::handle_context_menu_key(&mut app, key, &client, &tx).await;
                } else if matches!(app.main_view, MainView::Search) && app.search_input_active {
                    handlers::handle_search_input_key(&mut app, key, &client, &tx).await;
                } else {
                    let action = key_to_action(key);
                    if matches!(action, Action::OpenConfig) {
                        app.config_modal = Some(ConfigModal::new(
                            &cfg.host, cfg.port,
                            cfg.username.as_deref(), cfg.password.as_deref(),
                            cfg.use_nerd_icons,
                            cfg.auto_discover,
                            &cfg.broadcast_mask,
                            cfg.disable_auto_colors,
                        ));
                    } else {
                        let prev_gvc = app.global_volume_control;
                        let prev_art = app.full_art_mode;
                        if handlers::handle_action(&mut app, action, &client, &tx, &vol_sync_tx).await {
                            break;
                        }
                        if app.global_volume_control != prev_gvc {
                            cfg.global_volume_control = app.global_volume_control;
                            let _ = cfg.save();
                        }
                        if app.full_art_mode != prev_art {
                            cfg.full_art_mode = app.full_art_mode;
                            let _ = cfg.save();
                        }
                    }
                }
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
            }
            InputEvent::Tick => {}
        }
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
            app.connection = ConnectionState::Connected;
            if app.status_message.as_deref() == Some("Reconnecting...") {
                app.status_message = None;
            }
        }
        AppMsg::Disconnected => app.connection = ConnectionState::Disconnected,
        AppMsg::PlayersLoaded(players) => {
            if app.active_player.is_none()
                && let Some(p) = players.first()
            {
                background::start_now_playing_loop(p.playerid.clone(), client.clone(), tx.clone());
                app.active_player = Some(p.playerid.clone());
            }
            app.players = players;
        }
        AppMsg::NowPlayingUpdated(pid, np) => {
            if app.active_player.as_deref() == Some(pid.as_str()) {
                app.now_playing = Some(np);
            }
        }
        AppMsg::QueueLoaded(pid, q) => {
            if app.active_player.as_deref() == Some(pid.as_str()) {
                app.queue = q;
            }
        }
        AppMsg::ArtistsLoaded(a) => app.artists = a,
        AppMsg::AlbumsLoaded(a) => app.albums = a,
        AppMsg::TracksLoaded(t) => app.tracks = t,
        AppMsg::RadioItemsLoaded(items) => {
            app.radio_items = items;
            app.main_selected = 0;
        }
        AppMsg::AppItemsLoaded(items) => {
            if app.app_nav_stack.is_empty() {
                app.app_services = items.clone();
            }
            app.app_items = items;
            app.main_selected = 0;
        }
        AppMsg::FavItemsLoaded(items) => {
            app.fav_items = items;
            app.main_selected = 0;
        }
        AppMsg::FolderItemsLoaded(items) => {
            app.folder_items = items;
            app.main_selected = 0;
        }
        AppMsg::PlayerVolumesLoaded(volumes) => {
            app.player_volumes = volumes;
        }
        AppMsg::StatusMsg(msg) => {
            app.status_message = Some(msg);
        }
        AppMsg::SearchResultsLoaded(results) => {
            app.search_results = results;
            app.main_selected = 0;
        }
        AppMsg::Error(e) => app.status_message = Some(e),
        AppMsg::ArtworkLoaded(_) | AppMsg::ThumbnailLoaded(..) | AppMsg::ThumbnailFailed(_) => {
            // handled inline in the event loop
        }
    }
}
