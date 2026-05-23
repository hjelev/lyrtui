mod api;
mod app;
mod config;
mod events;
mod ui;

use anyhow::Result;
use api::LmsClient;
use app::{App, AppMsg, ConfigModal, ContextMenu, LibraryView, MainView, RadioNav, SidebarItem};
use crossterm::{
    event::{EnableMouseCapture, DisableMouseCapture, KeyCode, KeyEvent, MouseButton, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use events::{key_to_action, poll_event, Action, InputEvent};
use ratatui::{backend::CrosstermBackend, layout::Rect, widgets::ListState, Terminal};
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
    let cfg = config::Config::load()?;
    let client = Arc::new(LmsClient::new(cfg.base_url()));

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
    let mut app = App::new(cfg.default_player.clone());
    // Compute Now Playing panel height: art column is 18 cols; height = ceil(18 * fw / fh) + 2 borders.
    {
        let fs = picker.font_size();
        let art_rows = (18u16 * fs.width).div_ceil(fs.height);
        app.status_height = art_rows;
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
                            let pids: Vec<String> = players.iter().map(|p| p.playerid.clone()).collect();
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

    loop {
        terminal.draw(|f| ui::draw(f, &app, album_art.as_mut(), &mut sidebar_state, &mut main_state, &mut thumbnails, &cfg.host, cfg.port))?;

        // Drain all pending messages without blocking
        while let Ok(msg) = rx.try_recv() {
            match msg {
                AppMsg::ArtworkLoaded(bytes) => {
                    if let Ok(img) = image::load_from_memory(&bytes) {
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
            let end = (offset + visible + 5).min(main_list_len(&app));
            let base = client.server_base_url();
            for idx in offset..end {
                if let Some(url) = thumbnail_url_for(&app, idx, &base)
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
                    handle_config_key(&mut app, key, &mut cfg, &client);
                } else if app.confirm_clear_queue {
                    handle_confirm_clear_queue_key(&mut app, key, &client, &tx).await;
                } else if app.context_menu.is_some() {
                    handle_context_menu_key(&mut app, key, &client, &tx).await;
                } else {
                    let action = key_to_action(key);
                    if matches!(action, Action::OpenConfig) {
                        app.config_modal = Some(ConfigModal::new(&cfg.host, cfg.port));
                    } else if handle_action(&mut app, action, &client, &tx).await {
                        break;
                    }
                }
            }
            InputEvent::Mouse(mouse) => {
                if app.config_modal.is_none() {
                    let area = terminal.size()?.into();
                    handle_mouse_event(&mut app, mouse, &client, &tx, area, &sidebar_state, &main_state, &mut last_main_click).await;
                }
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
        AppMsg::Connected => app.connection = ConnectionState::Connected,
        AppMsg::Disconnected => app.connection = ConnectionState::Disconnected,
        AppMsg::PlayersLoaded(players) => {
            if app.active_player.is_none() && let Some(p) = players.first() {
                start_now_playing_loop(p.playerid.clone(), client.clone(), tx.clone());
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
            app.app_items = items;
            app.main_selected = 0;
        }
        AppMsg::FavItemsLoaded(items) => {
            app.fav_items = items;
            app.main_selected = 0;
        }
        AppMsg::PlayerVolumesLoaded(volumes) => {
            app.player_volumes = volumes;
        }
        AppMsg::StatusMsg(msg) => {
            app.status_message = Some(msg);
        }
        AppMsg::Error(e) => app.status_message = Some(e),
        AppMsg::ArtworkLoaded(_) | AppMsg::ThumbnailLoaded(..) | AppMsg::ThumbnailFailed(_) => {
            // handled inline in the event loop
        }
    }
}

fn start_now_playing_loop(pid: String, client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    tokio::spawn(async move {
        loop {
            if let Ok(np) = client.get_now_playing(&pid).await {
                let _ = tx.send(AppMsg::NowPlayingUpdated(pid.clone(), np)).await;
            }
            if let Ok(q) = client.get_queue(&pid).await {
                let _ = tx.send(AppMsg::QueueLoaded(pid.clone(), q)).await;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });
}

fn point_in(col: u16, row: u16, area: Rect) -> bool {
    col >= area.x && col < area.x + area.width && row >= area.y && row < area.y + area.height
}

#[allow(clippy::too_many_arguments)]
async fn handle_mouse_event(
    app: &mut App,
    mouse: crossterm::event::MouseEvent,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
    terminal_area: Rect,
    sidebar_state: &ListState,
    main_state: &ListState,
    last_main_click: &mut Option<(Instant, usize)>,
) {
    let (sidebar_area, main_area) = ui::compute_areas(terminal_area, app.status_height);
    let col = mouse.column;
    let row = mouse.row;

    // Context menu intercepts all left clicks
    if app.context_menu.is_some() {
        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
            let option_count = app.context_menu.as_ref().unwrap().option_count();
            let menu_area = ui::compute_context_menu_rect(terminal_area, option_count);
            if point_in(col, row, menu_area) {
                let opt_top = menu_area.y + 1;
                let opt_bot = menu_area.y + menu_area.height.saturating_sub(2);
                if row >= opt_top && row < opt_bot {
                    let opt_idx = (row - opt_top) as usize;
                    let count = app.context_menu.as_ref().unwrap().option_count();
                    if opt_idx < count {
                        app.context_menu.as_mut().unwrap().selected = opt_idx;
                        execute_context_menu_action(app, client, tx).await;
                    }
                }
            } else {
                app.context_menu = None;
            }
        }
        return;
    }

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Right) => {
            handle_action(app, Action::Back, client, tx).await;
        }
        MouseEventKind::Down(MouseButton::Left) => {
            // Check Now Playing control buttons first
            let ctrl_rects = ui::compute_statusbar_control_rects(terminal_area, app.status_height);
            let ctrl_hit = ctrl_rects.iter().enumerate().find(|(_, r)| point_in(col, row, **r));
            if let Some((btn_idx, _)) = ctrl_hit {
                let action = match btn_idx {
                    0 => Action::Prev,
                    1 => Action::PlayPause,
                    2 => Action::Stop,
                    _ => Action::Next,
                };
                handle_action(app, action, client, tx).await;
            } else if point_in(col, row, sidebar_area) {
                app.focus_sidebar = true;
                let inner_top = sidebar_area.y + 1;
                let inner_bot = sidebar_area.y + sidebar_area.height.saturating_sub(1);
                if row >= inner_top && row < inner_bot {
                    let rel = (row - inner_top) as usize;
                    let idx = sidebar_state.offset() + rel;
                    if idx < app.sidebar_items.len() {
                        app.sidebar_selected = idx;
                        handle_action(app, Action::Select, client, tx).await;
                    }
                }
            } else if point_in(col, row, main_area) {
                app.focus_sidebar = false;
                let inner_top = main_area.y + 1;
                let inner_bot = main_area.y + main_area.height.saturating_sub(1);
                if row >= inner_top && row < inner_bot {
                    let rel = (row - inner_top) as usize;
                    let row_h = if uses_two_row_layout(&app.main_view) { 2 } else { 1 };
                    let idx = main_state.offset() + rel / row_h;
                    if idx < main_list_len(app) {
                        let is_double = last_main_click
                            .as_ref()
                            .map(|(t, i)| *i == idx && t.elapsed().as_millis() < 500)
                            .unwrap_or(false);
                        *last_main_click = Some((Instant::now(), idx));

                        app.main_selected = idx;

                        if is_double || !is_main_item_playable(app) {
                            handle_action(app, Action::Select, client, tx).await;
                        } else {
                            app.context_menu = Some(ContextMenu::new(compute_parent_label(app)));
                        }
                    }
                }
            }
        }
        MouseEventKind::ScrollUp => {
            if point_in(col, row, sidebar_area) {
                app.focus_sidebar = true;
            } else if point_in(col, row, main_area) {
                app.focus_sidebar = false;
            }
            handle_action(app, Action::NavUp, client, tx).await;
        }
        MouseEventKind::ScrollDown => {
            if point_in(col, row, sidebar_area) {
                app.focus_sidebar = true;
            } else if point_in(col, row, main_area) {
                app.focus_sidebar = false;
            }
            handle_action(app, Action::NavDown, client, tx).await;
        }
        _ => {}
    }
}

fn thumbnail_url_for(app: &App, idx: usize, base: &str) -> Option<String> {
    match &app.main_view {
        MainView::Library(LibraryView::Artists) => {
            app.artists.get(idx).map(|a| {
                format!("{}/music/{}/artist.jpg", base, json_id_to_string(&a.id))
            })
        }
        MainView::Library(LibraryView::Albums { .. }) => {
            app.albums.get(idx).map(|a| {
                format!("{}/music/{}/cover.jpg", base, json_id_to_string(&a.id))
            })
        }
        MainView::Library(LibraryView::Tracks { .. }) => {
            app.tracks.get(idx).and_then(|t| {
                t.id.as_ref().map(|id| format!("{}/music/{}/cover.jpg", base, json_id_to_string(id)))
            })
        }
        MainView::Queue => {
            app.queue.get(idx).and_then(|t| {
                t.id.as_ref().map(|id| format!("{}/music/{}/cover.jpg", base, json_id_to_string(id)))
            })
        }
        MainView::Radio => app.radio_items.get(idx).and_then(|i| i.artwork_url.clone()),
        MainView::Apps => app.app_items.get(idx).and_then(|i| i.artwork_url.clone()),
        MainView::Favourites => app.fav_items.get(idx).and_then(|i| i.artwork_url.clone()),
        _ => None,
    }
}

fn compute_parent_label(app: &App) -> Option<String> {
    match &app.main_view {
        MainView::Library(LibraryView::Tracks { album_id: Some(id) }) => {
            let name = app.albums.iter()
                .find(|a| json_id_to_string(&a.id) == *id)
                .map(|a| a.album.clone())
                .unwrap_or_else(|| "album".to_string());
            Some(format!("Add \"{}\" to queue", name))
        }
        MainView::Radio if !app.radio_items.is_empty() => {
            Some(format!("Add \"{}\" folder to queue", app.radio_title))
        }
        MainView::Apps if !app.app_items.is_empty() => {
            Some(format!("Add \"{}\" folder to queue", app.app_title))
        }
        MainView::Favourites if !app.fav_items.is_empty() => {
            Some(format!("Add \"{}\" folder to queue", app.fav_title))
        }
        _ => None,
    }
}

fn uses_two_row_layout(view: &MainView) -> bool {
    !matches!(view, MainView::Players | MainView::Help)
}

fn is_main_item_playable(app: &App) -> bool {
    match &app.main_view {
        MainView::Library(LibraryView::Tracks { .. }) => !app.tracks.is_empty(),
        MainView::Radio => app.radio_items.get(app.main_selected).map(|i| i.is_playable() && !i.is_navigable()).unwrap_or(false),
        MainView::Apps => app.app_items.get(app.main_selected).map(|i| i.is_playable() && !i.is_navigable()).unwrap_or(false),
        MainView::Favourites => app.fav_items.get(app.main_selected).map(|i| i.is_playable() && !i.is_navigable()).unwrap_or(false),
        _ => false,
    }
}

async fn handle_context_menu_key(
    app: &mut App,
    key: crossterm::event::KeyEvent,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    let Some(menu) = app.context_menu.as_mut() else { return };
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            if menu.selected > 0 { menu.selected -= 1; }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if menu.selected < menu.option_count() - 1 { menu.selected += 1; }
        }
        KeyCode::Enter => {
            execute_context_menu_action(app, client, tx).await;
        }
        KeyCode::Esc | KeyCode::Char('q') => {
            app.context_menu = None;
        }
        _ => {}
    }
}

async fn handle_confirm_clear_queue_key(
    app: &mut App,
    key: crossterm::event::KeyEvent,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    match key.code {
        KeyCode::Char('y') | KeyCode::Enter => {
            app.confirm_clear_queue = false;
            if let Some(pid) = app.active_player.clone() {
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    if c.clear_queue(&pid).await.is_ok() {
                        let _ = t.send(AppMsg::StatusMsg("Queue cleared".to_string())).await;
                    }
                });
            }
        }
        _ => {
            app.confirm_clear_queue = false;
        }
    }
}

async fn execute_context_menu_action(
    app: &mut App,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    let Some(menu) = app.context_menu.take() else { return };
    match menu.selected {
        0 => handle_main_select(app, client, tx).await,
        1 => handle_insert_next(app, client, tx).await,
        2 => handle_add_to_queue(app, client, tx).await,
        3 => handle_add_to_favorites(app, client, tx).await,
        4 => handle_add_parent_to_queue(app, client, tx).await,
        _ => {}
    }
}

async fn handle_insert_next(app: &mut App, client: &Arc<LmsClient>, tx: &mpsc::Sender<AppMsg>) {
    let Some(pid) = app.active_player.clone() else {
        app.status_message = Some("No active player".to_string());
        return;
    };

    match app.main_view.clone() {
        MainView::Library(LibraryView::Tracks { .. }) => {
            if let Some(track) = app.tracks.get(app.main_selected) {
                let id = json_id_to_string(track.id.as_ref().unwrap_or(&serde_json::Value::Null));
                let name = track.title.clone();
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    if c.insert_track_next(&pid, &id).await.is_ok() {
                        let _ = t.send(AppMsg::StatusMsg(format!("\"{}\" will play next", name))).await;
                    }
                });
            }
        }
        MainView::Queue => {
            if let Some(track) = app.queue.get(app.main_selected) {
                let name = track.title.clone();
                if let Some(id_val) = &track.id {
                    let id = json_id_to_string(id_val);
                    let c = client.clone();
                    let t = tx.clone();
                    tokio::spawn(async move {
                        if c.insert_track_next(&pid, &id).await.is_ok() {
                            let _ = t.send(AppMsg::StatusMsg(format!("\"{}\" will play next", name))).await;
                        }
                    });
                }
            }
        }
        MainView::Radio => {
            if let Some(item) = app.radio_items.get(app.main_selected).cloned()
                && let Some(url) = item.url
            {
                let name = item.name.clone();
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    if c.insert_url_next(&pid, &url).await.is_ok() {
                        let _ = t.send(AppMsg::StatusMsg(format!("\"{}\" will play next", name))).await;
                    }
                });
            }
        }
        MainView::Apps => {
            if let Some(item) = app.app_items.get(app.main_selected).cloned()
                && let Some(url) = item.url
            {
                let name = item.name.clone();
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    if c.insert_url_next(&pid, &url).await.is_ok() {
                        let _ = t.send(AppMsg::StatusMsg(format!("\"{}\" will play next", name))).await;
                    }
                });
            }
        }
        MainView::Favourites => {
            if let Some(item) = app.fav_items.get(app.main_selected).cloned()
                && let Some(url) = item.url
            {
                let name = item.name.clone();
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    if c.insert_url_next(&pid, &url).await.is_ok() {
                        let _ = t.send(AppMsg::StatusMsg(format!("\"{}\" will play next", name))).await;
                    }
                });
            }
        }
        _ => {}
    }
}

async fn handle_add_to_favorites(app: &mut App, client: &Arc<LmsClient>, tx: &mpsc::Sender<AppMsg>) {
    let Some(pid) = app.active_player.clone() else {
        app.status_message = Some("No active player".to_string());
        return;
    };

    match app.main_view.clone() {
        MainView::Library(LibraryView::Tracks { .. }) => {
            if let Some(track) = app.tracks.get(app.main_selected) {
                let id = json_id_to_string(track.id.as_ref().unwrap_or(&serde_json::Value::Null));
                let name = track.title.clone();
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    let url = format!("db:track.id={}", id);
                    if c.add_to_favorites(&pid, &url, &name).await.is_ok() {
                        let _ = t.send(AppMsg::StatusMsg(format!("Added \"{}\" to favourites", name))).await;
                    } else {
                        let _ = t.send(AppMsg::StatusMsg("Could not add to favourites".to_string())).await;
                    }
                });
            }
        }
        MainView::Radio => {
            if let Some(item) = app.radio_items.get(app.main_selected).cloned()
                && let Some(url) = item.url
            {
                let name = item.name.clone();
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    if c.add_to_favorites(&pid, &url, &name).await.is_ok() {
                        let _ = t.send(AppMsg::StatusMsg(format!("Added \"{}\" to favourites", name))).await;
                    } else {
                        let _ = t.send(AppMsg::StatusMsg("Could not add to favourites".to_string())).await;
                    }
                });
            }
        }
        MainView::Apps => {
            if let Some(item) = app.app_items.get(app.main_selected).cloned()
                && let Some(url) = item.url
            {
                let name = item.name.clone();
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    if c.add_to_favorites(&pid, &url, &name).await.is_ok() {
                        let _ = t.send(AppMsg::StatusMsg(format!("Added \"{}\" to favourites", name))).await;
                    } else {
                        let _ = t.send(AppMsg::StatusMsg("Could not add to favourites".to_string())).await;
                    }
                });
            }
        }
        _ => {
            app.status_message = Some("Cannot add this item to favourites".to_string());
        }
    }
}

/// Returns true if the app should quit.
async fn handle_action(
    app: &mut App,
    action: Action,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) -> bool {
    match action {
        Action::Quit => return true,

        Action::FocusSidebar => {
            app.focus_sidebar = true;
            app.players_focus_global = false;
        }
        Action::FocusMain => app.focus_sidebar = false,

        Action::NavUp => {
            if app.focus_sidebar {
                app.sidebar_selected = app.sidebar_selected.saturating_sub(1);
            } else if let MainView::Players = &app.main_view {
                if !app.players_focus_global && app.main_selected == 0 {
                    app.players_focus_global = true;
                } else if !app.players_focus_global {
                    app.main_selected -= 1;
                }
            } else {
                app.main_selected = app.main_selected.saturating_sub(1);
            }
        }

        Action::NavDown => {
            if app.focus_sidebar {
                let max = app.sidebar_items.len().saturating_sub(1);
                if app.sidebar_selected < max {
                    app.sidebar_selected += 1;
                }
            } else if let MainView::Players = &app.main_view {
                if app.players_focus_global {
                    app.players_focus_global = false;
                    app.main_selected = 0;
                } else {
                    let max = main_list_len(app).saturating_sub(1);
                    if app.main_selected < max {
                        app.main_selected += 1;
                    }
                }
            } else {
                let max = main_list_len(app).saturating_sub(1);
                if app.main_selected < max {
                    app.main_selected += 1;
                }
            }
        }

        Action::Select => {
            if app.focus_sidebar {
                app.main_selected = 0;
                app.players_focus_global = false;
                match app.sidebar_items.get(app.sidebar_selected).cloned() {
                    Some(SidebarItem::Artists) => {
                        app.main_view = MainView::Library(LibraryView::Artists);
                        app.focus_sidebar = false;
                    }
                    Some(SidebarItem::Albums) => {
                        load_albums(None, client.clone(), tx.clone());
                        app.main_view = MainView::Library(LibraryView::Albums { artist_id: None });
                        app.focus_sidebar = false;
                    }
                    Some(SidebarItem::Tracks) => {
                        load_all_tracks(client.clone(), tx.clone());
                        app.main_view = MainView::Library(LibraryView::Tracks { album_id: None });
                        app.focus_sidebar = false;
                    }
                    Some(SidebarItem::Radio) => {
                        app.radio_items = vec![];
                        app.radio_nav_stack = vec![];
                        app.radio_title = "Radio".to_string();
                        app.main_view = MainView::Radio;
                        app.focus_sidebar = false;
                        load_radio_services(client.clone(), tx.clone());
                    }
                    Some(SidebarItem::Apps) => {
                        app.app_items = vec![];
                        app.app_nav_stack = vec![];
                        app.app_title = "Apps".to_string();
                        app.main_view = MainView::Apps;
                        app.focus_sidebar = false;
                        load_app_services(client.clone(), tx.clone());
                    }
                    Some(SidebarItem::Favourites) => {
                        app.fav_items = vec![];
                        app.fav_nav_stack = vec![];
                        app.fav_title = "Favourites".to_string();
                        app.main_view = MainView::Favourites;
                        app.focus_sidebar = false;
                        load_fav_items(app.active_player.clone().unwrap_or_default(), None, client.clone(), tx.clone());
                    }
                    Some(SidebarItem::Queue) => {
                        app.main_view = MainView::Queue;
                        app.focus_sidebar = false;
                    }
                    Some(SidebarItem::Players) => {
                        app.main_view = MainView::Players;
                        app.focus_sidebar = false;
                    }
                    Some(SidebarItem::Help) => {
                        app.main_view = MainView::Help;
                        app.focus_sidebar = false;
                    }
                    None => {}
                }
            } else if is_main_item_playable(app) {
                app.context_menu = Some(ContextMenu::new(compute_parent_label(app)));
            } else {
                handle_main_select(app, client, tx).await;
            }
        }

        Action::Back => {
            if !app.focus_sidebar {
                match &app.main_view.clone() {
                    MainView::Library(LibraryView::Tracks { album_id: Some(_) }) => {
                        app.main_view = MainView::Library(LibraryView::Albums { artist_id: None });
                        app.main_selected = 0;
                    }
                    MainView::Library(LibraryView::Albums { artist_id: Some(_) }) => {
                        app.main_view = MainView::Library(LibraryView::Artists);
                        app.main_selected = 0;
                    }
                    MainView::Radio => {
                        if let Some(prev) = app.radio_nav_stack.pop() {
                            app.radio_items = prev.items;
                            app.main_selected = prev.selected;
                            app.radio_title = prev.title;
                        } else {
                            app.focus_sidebar = true;
                        }
                    }
                    MainView::Apps => {
                        if let Some(prev) = app.app_nav_stack.pop() {
                            app.app_items = prev.items;
                            app.main_selected = prev.selected;
                            app.app_title = prev.title;
                        } else {
                            app.focus_sidebar = true;
                        }
                    }
                    MainView::Favourites => {
                        if let Some(prev) = app.fav_nav_stack.pop() {
                            app.fav_items = prev.items;
                            app.main_selected = prev.selected;
                            app.fav_title = prev.title;
                        } else {
                            app.focus_sidebar = true;
                        }
                    }
                    _ => {
                        app.focus_sidebar = true;
                        app.players_focus_global = false;
                    }
                }
            }
        }

        Action::PlayPause => {
            if let Some(pid) = app.active_player.clone() {
                let c = client.clone();
                let playing = app.is_playing();
                tokio::spawn(async move {
                    let _ = if playing { c.pause(&pid).await } else { c.play(&pid).await };
                });
            }
        }

        Action::Next => {
            if let Some(pid) = app.active_player.clone() {
                let c = client.clone();
                tokio::spawn(async move { let _ = c.next(&pid).await; });
            }
        }

        Action::Stop => {
            if let Some(pid) = app.active_player.clone() {
                let c = client.clone();
                tokio::spawn(async move { let _ = c.stop(&pid).await; });
            }
        }

        Action::Prev => {
            if let Some(pid) = app.active_player.clone() {
                let c = client.clone();
                tokio::spawn(async move { let _ = c.prev(&pid).await; });
            }
        }

        Action::VolumeUp => {
            if let MainView::Players = &app.main_view {
                if app.players_focus_global {
                    let targets: Vec<(String, u8)> = app.players.iter()
                        .map(|p| (p.playerid.clone(), app.player_volumes.get(&p.playerid).copied().unwrap_or(50)))
                        .collect();
                    let c = client.clone();
                    tokio::spawn(async move {
                        for (pid, vol) in targets {
                            let _ = c.set_volume(&pid, (vol + 5).min(100)).await;
                        }
                    });
                } else if let Some(player) = app.players.get(app.main_selected) {
                    let pid = player.playerid.clone();
                    let vol = app.player_volumes.get(&pid).copied().unwrap_or(50);
                    let c = client.clone();
                    tokio::spawn(async move { let _ = c.set_volume(&pid, (vol + 5).min(100)).await; });
                }
            } else if let Some(pid) = app.active_player.clone() {
                let vol = app.now_playing.as_ref().map(|n| n.volume).unwrap_or(50);
                let c = client.clone();
                tokio::spawn(async move { let _ = c.set_volume(&pid, (vol + 5).min(100)).await; });
            }
        }

        Action::VolumeDown => {
            if let MainView::Players = &app.main_view {
                if app.players_focus_global {
                    let targets: Vec<(String, u8)> = app.players.iter()
                        .map(|p| (p.playerid.clone(), app.player_volumes.get(&p.playerid).copied().unwrap_or(50)))
                        .collect();
                    let c = client.clone();
                    tokio::spawn(async move {
                        for (pid, vol) in targets {
                            let _ = c.set_volume(&pid, vol.saturating_sub(5)).await;
                        }
                    });
                } else if let Some(player) = app.players.get(app.main_selected) {
                    let pid = player.playerid.clone();
                    let vol = app.player_volumes.get(&pid).copied().unwrap_or(50);
                    let c = client.clone();
                    tokio::spawn(async move { let _ = c.set_volume(&pid, vol.saturating_sub(5)).await; });
                }
            } else if let Some(pid) = app.active_player.clone() {
                let vol = app.now_playing.as_ref().map(|n| n.volume).unwrap_or(50);
                let c = client.clone();
                tokio::spawn(async move { let _ = c.set_volume(&pid, vol.saturating_sub(5)).await; });
            }
        }

        Action::TogglePower => {
            if let MainView::Players = &app.main_view
                && !app.players_focus_global
                && let Some(player) = app.players.get(app.main_selected)
            {
                let pid = player.playerid.clone();
                let turn_on = player.power == 0;
                let c = client.clone();
                tokio::spawn(async move { let _ = c.set_power(&pid, turn_on).await; });
            }
        }

        Action::AddToQueue => {
            if !app.focus_sidebar {
                handle_add_to_queue(app, client, tx).await;
            }
        }

        Action::ClearQueue => {
            if app.active_player.is_some() && !app.queue.is_empty() {
                app.confirm_clear_queue = true;
            }
        }

        Action::OpenConfig | Action::None => {}
    }
    false
}

fn handle_config_key(app: &mut App, key: KeyEvent, cfg: &mut config::Config, client: &Arc<LmsClient>) {
    if app.config_modal.is_none() {
        return;
    }

    let editing = app.config_modal.as_ref().unwrap().editing;

    if editing {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Tab => {
                app.config_modal.as_mut().unwrap().editing = false;
            }
            KeyCode::Char(c) => {
                let modal = app.config_modal.as_mut().unwrap();
                if modal.selected_field == 0 {
                    modal.host.push(c);
                } else if c.is_ascii_digit() {
                    modal.port.push(c);
                }
            }
            KeyCode::Backspace => {
                let modal = app.config_modal.as_mut().unwrap();
                if modal.selected_field == 0 {
                    modal.host.pop();
                } else {
                    modal.port.pop();
                }
            }
            _ => {}
        }
    } else {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                let modal = app.config_modal.as_mut().unwrap();
                if modal.selected_field > 0 {
                    modal.selected_field -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let modal = app.config_modal.as_mut().unwrap();
                if modal.selected_field < 1 {
                    modal.selected_field += 1;
                }
            }
            KeyCode::Enter | KeyCode::Char('i') => {
                let modal = app.config_modal.as_mut().unwrap();
                modal.editing = true;
                modal.error = None;
            }
            KeyCode::Char('s') => {
                let (host, port_str) = {
                    let modal = app.config_modal.as_ref().unwrap();
                    (modal.host.trim().to_string(), modal.port.trim().to_string())
                };
                if host.is_empty() {
                    app.config_modal.as_mut().unwrap().error =
                        Some("Host cannot be empty".to_string());
                } else {
                    match port_str.parse::<u16>() {
                        Ok(port) if port > 0 => {
                            cfg.host = host;
                            cfg.port = port;
                            match cfg.save() {
                                Ok(()) => {
                                    client.update_base_url(cfg.base_url());
                                    app.config_modal = None;
                                    app.connection = app::ConnectionState::Reconnecting;
                                    app.players = vec![];
                                    app.active_player = None;
                                    app.now_playing = None;
                                    app.status_message = Some("Reconnecting...".to_string());
                                }
                                Err(e) => {
                                    app.config_modal.as_mut().unwrap().error =
                                        Some(format!("Save error: {e}"));
                                }
                            }
                        }
                        _ => {
                            app.config_modal.as_mut().unwrap().error =
                                Some("Invalid port (1–65535)".to_string());
                        }
                    }
                }
            }
            KeyCode::Esc | KeyCode::Char('c') | KeyCode::Char('q') => {
                app.config_modal = None;
            }
            _ => {}
        }
    }
}

async fn handle_main_select(app: &mut App, client: &Arc<LmsClient>, tx: &mpsc::Sender<AppMsg>) {
    match app.main_view.clone() {
        MainView::Library(LibraryView::Artists) => {
            if let Some(artist) = app.artists.get(app.main_selected) {
                let id = json_id_to_string(&artist.id);
                load_albums(Some(id.clone()), client.clone(), tx.clone());
                app.main_view = MainView::Library(LibraryView::Albums { artist_id: Some(id) });
                app.main_selected = 0;
            }
        }
        MainView::Library(LibraryView::Albums { .. }) => {
            if let Some(album) = app.albums.get(app.main_selected) {
                let id = json_id_to_string(&album.id);
                load_tracks(id.clone(), client.clone(), tx.clone());
                app.main_view = MainView::Library(LibraryView::Tracks { album_id: Some(id) });
                app.main_selected = 0;
            }
        }
        MainView::Library(LibraryView::Tracks { album_id }) => {
            if let Some(pid) = app.active_player.clone() {
                let idx = app.main_selected;
                let c = client.clone();
                match album_id {
                    Some(aid) => {
                        tokio::spawn(async move {
                            let _ = c.play_album(&pid, &aid).await;
                            tokio::time::sleep(Duration::from_millis(300)).await;
                            let _ = c.play_track_index(&pid, idx).await;
                        });
                    }
                    None => {
                        if let Some(track) = app.tracks.get(idx) {
                            let track_id = json_id_to_string(
                                track.id.as_ref().unwrap_or(&serde_json::Value::Null),
                            );
                            tokio::spawn(async move { let _ = c.play_track(&pid, &track_id).await; });
                        }
                    }
                }
            }
        }
        MainView::Queue => {
            if let Some(pid) = app.active_player.clone() {
                let idx = app.main_selected;
                let c = client.clone();
                tokio::spawn(async move { let _ = c.play_track_index(&pid, idx).await; });
            }
        }
        MainView::Players => {
            if !app.players_focus_global
                && let Some(player) = app.players.get(app.main_selected)
            {
                let pid = player.playerid.clone();
                app.active_player = Some(pid.clone());
                start_now_playing_loop(pid, client.clone(), tx.clone());
            }
        }
        MainView::Radio => {
            if let Some(item) = app.radio_items.get(app.main_selected).cloned() {
                // Navigation takes priority: a container may carry a URL but must still be browsed.
                if item.is_navigable()
                    && let Some(cmd) = item.cmd
                {
                    let item_id = item.item_id;
                    let pid = app.active_player.clone().unwrap_or_default();
                    let nav = RadioNav {
                        title: app.radio_title.clone(),
                        items: std::mem::take(&mut app.radio_items),
                        selected: app.main_selected,
                    };
                    app.radio_nav_stack.push(nav);
                    app.radio_title = item.name;
                    app.main_selected = 0;
                    load_radio_items(pid, cmd, item_id, client.clone(), tx.clone());
                } else if item.is_playable() {
                    if let (Some(pid), Some(url)) =
                        (app.active_player.clone(), item.url)
                    {
                        let c = client.clone();
                        tokio::spawn(async move { let _ = c.play_url(&pid, &url).await; });
                    }
                }
            }
        }
        MainView::Apps => {
            if let Some(item) = app.app_items.get(app.main_selected).cloned() {
                // Navigation takes priority: a container may carry a URL but must still be browsed.
                if item.is_navigable()
                    && let Some(cmd) = item.cmd
                {
                    let item_id = item.item_id;
                    let pid = app.active_player.clone().unwrap_or_default();
                    let nav = RadioNav {
                        title: app.app_title.clone(),
                        items: std::mem::take(&mut app.app_items),
                        selected: app.main_selected,
                    };
                    app.app_nav_stack.push(nav);
                    app.app_title = item.name;
                    app.main_selected = 0;
                    load_app_items(pid, cmd, item_id, client.clone(), tx.clone());
                } else if item.is_playable() {
                    if let (Some(pid), Some(url)) = (app.active_player.clone(), item.url) {
                        let c = client.clone();
                        tokio::spawn(async move { let _ = c.play_url(&pid, &url).await; });
                    }
                }
            }
        }
        MainView::Favourites => {
            if let Some(item) = app.fav_items.get(app.main_selected).cloned() {
                // Navigation takes priority: a container may carry a URL but must still be browsed.
                if item.is_navigable()
                    && let Some(item_id) = item.item_id.clone()
                {
                    let pid = app.active_player.clone().unwrap_or_default();
                    let nav = RadioNav {
                        title: app.fav_title.clone(),
                        items: std::mem::take(&mut app.fav_items),
                        selected: app.main_selected,
                    };
                    app.fav_nav_stack.push(nav);
                    app.fav_title = item.name;
                    app.main_selected = 0;
                    load_fav_items(pid, Some(item_id), client.clone(), tx.clone());
                } else if item.is_playable() {
                    if let (Some(pid), Some(url)) = (app.active_player.clone(), item.url) {
                        let c = client.clone();
                        tokio::spawn(async move { let _ = c.play_url(&pid, &url).await; });
                    }
                }
            }
        }
        MainView::Help => {}
    }
}

async fn handle_add_parent_to_queue(app: &mut App, client: &Arc<LmsClient>, tx: &mpsc::Sender<AppMsg>) {
    let Some(pid) = app.active_player.clone() else {
        app.status_message = Some("No active player".to_string());
        return;
    };

    match app.main_view.clone() {
        MainView::Library(LibraryView::Tracks { album_id: Some(id) }) => {
            let name = app.albums.iter()
                .find(|a| json_id_to_string(&a.id) == id)
                .map(|a| a.album.clone())
                .unwrap_or_else(|| "album".to_string());
            let c = client.clone();
            let t = tx.clone();
            tokio::spawn(async move {
                if c.add_album_to_queue(&pid, &id).await.is_ok() {
                    let _ = t.send(AppMsg::StatusMsg(format!("Added \"{}\" to queue", name))).await;
                }
            });
        }
        MainView::Radio => {
            let urls: Vec<String> = app.radio_items.iter().filter_map(|i| i.url.clone()).collect();
            let title = app.radio_title.clone();
            let c = client.clone();
            let t = tx.clone();
            tokio::spawn(async move {
                let mut added = 0usize;
                for url in &urls {
                    if c.add_url_to_queue(&pid, url).await.is_ok() { added += 1; }
                }
                if added > 0 {
                    let _ = t.send(AppMsg::StatusMsg(format!("Added {} items from \"{}\" to queue", added, title))).await;
                }
            });
        }
        MainView::Apps => {
            let urls: Vec<String> = app.app_items.iter().filter_map(|i| i.url.clone()).collect();
            let title = app.app_title.clone();
            let c = client.clone();
            let t = tx.clone();
            tokio::spawn(async move {
                let mut added = 0usize;
                for url in &urls {
                    if c.add_url_to_queue(&pid, url).await.is_ok() { added += 1; }
                }
                if added > 0 {
                    let _ = t.send(AppMsg::StatusMsg(format!("Added {} items from \"{}\" to queue", added, title))).await;
                }
            });
        }
        MainView::Favourites => {
            let urls: Vec<String> = app.fav_items.iter().filter_map(|i| i.url.clone()).collect();
            let title = app.fav_title.clone();
            let c = client.clone();
            let t = tx.clone();
            tokio::spawn(async move {
                let mut added = 0usize;
                for url in &urls {
                    if c.add_url_to_queue(&pid, url).await.is_ok() { added += 1; }
                }
                if added > 0 {
                    let _ = t.send(AppMsg::StatusMsg(format!("Added {} items from \"{}\" to queue", added, title))).await;
                }
            });
        }
        _ => {}
    }
}

async fn handle_add_to_queue(app: &mut App, client: &Arc<LmsClient>, tx: &mpsc::Sender<AppMsg>) {
    let Some(pid) = app.active_player.clone() else {
        app.status_message = Some("No active player".to_string());
        return;
    };

    match app.main_view.clone() {
        MainView::Library(LibraryView::Artists) => {
            if let Some(artist) = app.artists.get(app.main_selected) {
                let id = json_id_to_string(&artist.id);
                let name = artist.artist.clone();
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    if c.add_artist_to_queue(&pid, &id).await.is_ok() {
                        let _ = t.send(AppMsg::StatusMsg(format!("Added \"{}\" to queue", name))).await;
                    }
                });
            }
        }
        MainView::Library(LibraryView::Albums { .. }) => {
            if let Some(album) = app.albums.get(app.main_selected) {
                let id = json_id_to_string(&album.id);
                let name = album.album.clone();
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    if c.add_album_to_queue(&pid, &id).await.is_ok() {
                        let _ = t.send(AppMsg::StatusMsg(format!("Added \"{}\" to queue", name))).await;
                    }
                });
            }
        }
        MainView::Library(LibraryView::Tracks { .. }) => {
            if let Some(track) = app.tracks.get(app.main_selected) {
                let id = json_id_to_string(track.id.as_ref().unwrap_or(&serde_json::Value::Null));
                let name = track.title.clone();
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    if c.add_track_to_queue(&pid, &id).await.is_ok() {
                        let _ = t.send(AppMsg::StatusMsg(format!("Added \"{}\" to queue", name))).await;
                    }
                });
            }
        }
        MainView::Radio => {
            if let Some(item) = app.radio_items.get(app.main_selected).cloned()
                && let Some(url) = item.url
            {
                let name = item.name.clone();
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    if c.add_url_to_queue(&pid, &url).await.is_ok() {
                        let _ = t.send(AppMsg::StatusMsg(format!("Added \"{}\" to queue", name))).await;
                    }
                });
            }
        }
        MainView::Apps => {
            if let Some(item) = app.app_items.get(app.main_selected).cloned()
                && let Some(url) = item.url
            {
                let name = item.name.clone();
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    if c.add_url_to_queue(&pid, &url).await.is_ok() {
                        let _ = t.send(AppMsg::StatusMsg(format!("Added \"{}\" to queue", name))).await;
                    }
                });
            }
        }
        MainView::Favourites => {
            if let Some(item) = app.fav_items.get(app.main_selected).cloned()
                && let Some(url) = item.url
            {
                let name = item.name.clone();
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    if c.add_url_to_queue(&pid, &url).await.is_ok() {
                        let _ = t.send(AppMsg::StatusMsg(format!("Added \"{}\" to queue", name))).await;
                    }
                });
            }
        }
        _ => {}
    }
}

fn load_albums(artist_id: Option<String>, client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    tokio::spawn(async move {
        let id_ref = artist_id.as_deref();
        if let Ok(albums) = client.get_albums(id_ref).await {
            let _ = tx.send(AppMsg::AlbumsLoaded(albums)).await;
        }
    });
}

fn load_radio_services(client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    tokio::spawn(async move {
        if let Ok(items) = client.get_radio_services().await {
            let _ = tx.send(AppMsg::RadioItemsLoaded(items)).await;
        }
    });
}

fn load_radio_items(
    player_id: String,
    cmd: String,
    item_id: Option<String>,
    client: Arc<LmsClient>,
    tx: mpsc::Sender<AppMsg>,
) {
    tokio::spawn(async move {
        if let Ok(items) = client.browse_radio(&player_id, &cmd, item_id.as_deref()).await {
            let _ = tx.send(AppMsg::RadioItemsLoaded(items)).await;
        }
    });
}

fn load_fav_items(player_id: String, item_id: Option<String>, client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    tokio::spawn(async move {
        if let Ok(items) = client.browse_radio(&player_id, "favorites", item_id.as_deref()).await {
            let _ = tx.send(AppMsg::FavItemsLoaded(items)).await;
        }
    });
}

fn load_app_services(client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    tokio::spawn(async move {
        if let Ok(items) = client.get_apps().await {
            let _ = tx.send(AppMsg::AppItemsLoaded(items)).await;
        }
    });
}

fn load_app_items(
    player_id: String,
    cmd: String,
    item_id: Option<String>,
    client: Arc<LmsClient>,
    tx: mpsc::Sender<AppMsg>,
) {
    tokio::spawn(async move {
        if let Ok(items) = client.browse_radio(&player_id, &cmd, item_id.as_deref()).await {
            let _ = tx.send(AppMsg::AppItemsLoaded(items)).await;
        }
    });
}

fn load_all_tracks(client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    tokio::spawn(async move {
        if let Ok(tracks) = client.get_all_tracks().await {
            let _ = tx.send(AppMsg::TracksLoaded(tracks)).await;
        }
    });
}

fn load_tracks(album_id: String, client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    tokio::spawn(async move {
        if let Ok(tracks) = client.get_tracks(&album_id).await {
            let _ = tx.send(AppMsg::TracksLoaded(tracks)).await;
        }
    });
}

fn json_id_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn main_list_len(app: &App) -> usize {
    match &app.main_view {
        MainView::Library(LibraryView::Artists) => app.artists.len(),
        MainView::Library(LibraryView::Albums { .. }) => app.albums.len(),
        MainView::Library(LibraryView::Tracks { .. }) => app.tracks.len(),
        MainView::Queue => app.queue.len(),
        MainView::Players => app.players.len(),
        MainView::Radio => app.radio_items.len(),
        MainView::Apps => app.app_items.len(),
        MainView::Favourites => app.fav_items.len(),
        MainView::Help => 0,
    }
}
