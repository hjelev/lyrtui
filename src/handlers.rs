use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::widgets::ListState;

use crate::api::{FolderItemType, LmsClient};
use crate::app::{
    App, AppMsg, ConnectionState, ContextMenu, FieldKind, FolderNav, IMAGE_PROTOCOLS, LibraryView,
    MainView, MyMusicEntry, RadioNav, SearchResultItem, SearchScope, SidebarItem, SyncModal,
};
use crate::discovery;
use crate::events::Action;
use crate::{background, config, filter, ui, utils};

fn point_in(col: u16, row: u16, area: Rect) -> bool {
    col >= area.x && col < area.x + area.width && row >= area.y && row < area.y + area.height
}

/// Map a clicked terminal `row` to a list index, given the list's first/last inner rows
/// (`[top, bot)`), the rendered scroll `offset`, and the per-item row height. Returns `None`
/// when the row is outside the list area.
fn list_index_at_row(row: u16, top: u16, bot: u16, offset: usize, row_h: u16) -> Option<usize> {
    (row >= top && row < bot).then(|| offset + (row - top) as usize / row_h as usize)
}

const HELP_CONTENT_LINES: u16 = 18; // tallest column in draw_help (left); keep in sync

const SEARCH_SCOPES: [SearchScope; 4] = [
    SearchScope::MyMusic,
    SearchScope::Radios,
    SearchScope::Apps,
    SearchScope::All,
];

fn map_control_button_to_action(btn_idx: usize) -> Action {
    match btn_idx {
        0 => Action::Prev,
        1 => Action::PlayPause,
        2 => Action::Stop,
        3 => Action::Next,
        4 => Action::ToggleShuffle,
        5 => Action::ToggleRepeat,
        6 => Action::VolumeDown,
        _ => Action::VolumeUp,
    }
}

fn is_double_click(last: &Option<(Instant, usize)>, idx: usize) -> bool {
    last.as_ref()
        .map(|(t, i)| *i == idx && t.elapsed().as_millis() < 500)
        .unwrap_or(false)
}

/// Spawn `op`, and on `Ok` send `ok_msg` as a transient status message. Collapses the
/// repeated "clone client+tx, spawn task, if ok send StatusMsg" pattern used throughout the
/// read-only action handlers.
fn spawn_status<F, Fut>(client: &Arc<LmsClient>, tx: &mpsc::Sender<AppMsg>, ok_msg: String, op: F)
where
    F: FnOnce(Arc<LmsClient>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
{
    let c = client.clone();
    let t = tx.clone();
    tokio::spawn(async move {
        if op(c).await.is_ok() {
            let _ = t.send(AppMsg::StatusMsg(ok_msg)).await;
        }
    });
}

/// Spawn a fire-and-forget `op` on the client, discarding its result. The `spawn_status`
/// sibling for actions that don't surface a status message (e.g. play/sync commands).
fn spawn_fire<F, Fut>(client: &Arc<LmsClient>, op: F)
where
    F: FnOnce(Arc<LmsClient>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
{
    let c = client.clone();
    tokio::spawn(async move {
        let _ = op(c).await;
    });
}

/// Spawn an add-to-favourites call for `url`, surfacing a success or failure status message.
fn spawn_add_to_favorites(
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
    pid: String,
    url: String,
    name: String,
) {
    let c = client.clone();
    let t = tx.clone();
    tokio::spawn(async move {
        if c.add_to_favorites(&pid, &url, &name).await.is_ok() {
            let _ = t
                .send(AppMsg::StatusMsg(format!("Added \"{}\" to favourites", name)))
                .await;
        } else {
            let _ = t
                .send(AppMsg::StatusMsg("Could not add to favourites".to_string()))
                .await;
        }
    });
}

/// Seeks the current track to the position corresponding to a left-click at
/// column `col` within the progress-bar fill rect `bar`. Updates the elapsed
/// time optimistically so the bar moves immediately, then fires the seek.
fn seek_to_click(app: &mut App, client: &Arc<LmsClient>, col: u16, bar: Rect) {
    let Some(pid) = app.active_pid() else {
        return;
    };
    let Some(np) = app.now_playing.as_ref() else {
        return;
    };
    if np.duration <= 0.0 || bar.width == 0 {
        return;
    }
    let frac = ((col.saturating_sub(bar.x)) as f64 / bar.width as f64).clamp(0.0, 1.0);
    let secs = frac * np.duration;
    if let Some(np) = app.now_playing.as_mut() {
        np.elapsed = secs;
    }
    spawn_fire(client, move |c| async move { c.seek(&pid, secs).await });
}

fn set_volume_from_click(
    app: &mut App,
    vol_sync_tx: &mpsc::Sender<(String, u8)>,
    col: u16,
    bar: Rect,
    pid: &str,
) {
    if bar.width == 0 {
        return;
    }
    let frac = (col.saturating_sub(bar.x) as f64 / bar.width as f64).clamp(0.0, 1.0);
    let nv = (frac * 100.0).round() as u8;
    app.player_volumes.insert(pid.to_string(), nv);
    app.volume_pending.insert(pid.to_string(), std::time::Instant::now());
    if app.active_player.as_deref() == Some(pid)
        && let Some(np) = app.now_playing.as_mut()
    {
        np.volume = nv;
    }
    let _ = vol_sync_tx.try_send((pid.to_string(), nv));
}

fn cycle_search_scope(
    forward: bool,
    app: &mut App,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    let idx = SEARCH_SCOPES
        .iter()
        .position(|s| s == &app.search_scope)
        .unwrap_or(0);
    app.search_scope = SEARCH_SCOPES[if forward {
        (idx + 1) % 4
    } else {
        (idx + 3) % 4
    }]
    .clone();
    if !app.search_query.is_empty() {
        app.search_results = vec![];
        app.main_selected = 0;
        let player_id = app.active_pid().unwrap_or_default();
        background::trigger_search(
            app.search_query.clone(),
            app.search_scope.clone(),
            app.app_services.clone(),
            app.radio_services.clone(),
            player_id,
            client.clone(),
            tx.clone(),
        );
    }
}

fn help_max_scroll(app: &App) -> u16 {
    HELP_CONTENT_LINES.saturating_sub(app.help_visible_lines.get())
}

// --- Overlay mouse handlers --------------------------------------------------
// Each of these is called from handle_mouse_event while its overlay is open. The
// overlay always consumes the event (handle_mouse_event returns afterwards), so
// these only need to act on left-clicks (and, where relevant, scroll).

/// Sync modal: toggle player checkboxes, confirm, or dismiss.
fn handle_sync_modal_mouse(
    app: &mut App,
    mouse: MouseEvent,
    client: &Arc<LmsClient>,
    terminal_area: Rect,
) {
    let (col, row) = (mouse.column, mouse.row);
    if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
        let n = app
            .sync_modal
            .as_ref()
            .map(|m| m.other_players.len())
            .unwrap_or(0);
        let (popup, player_rects, [sync_rect, cancel_rect]) =
            ui::compute_sync_modal_rects(terminal_area, n);
        if point_in(col, row, sync_rect) {
            apply_sync(app, client);
        } else if point_in(col, row, cancel_rect) {
            app.sync_modal = None;
        } else if let Some(modal) = app.sync_modal.as_mut() {
            if let Some(i) = player_rects.iter().position(|r| point_in(col, row, *r)) {
                modal.focus_buttons = false;
                modal.list_selected = i;
                if let Some(c) = modal.checked.get_mut(i) {
                    *c = !*c;
                }
            } else if !point_in(col, row, popup) {
                app.sync_modal = None;
            }
        }
    }
}

/// Clear-queue confirmation: clear on OK, dismiss otherwise.
fn handle_clear_queue_mouse(
    app: &mut App,
    mouse: MouseEvent,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
    terminal_area: Rect,
) {
    let (col, row) = (mouse.column, mouse.row);
    if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
        let (popup, [ok_rect, cancel_rect]) = ui::compute_clear_queue_button_rects(terminal_area);
        if point_in(col, row, ok_rect) {
            app.confirm_clear_queue = false;
            if let Some(pid) = app.active_pid() {
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    if c.clear_queue(&pid).await.is_ok() {
                        let _ = t.send(AppMsg::StatusMsg("Queue cleared".to_string())).await;
                    }
                });
            }
        } else if point_in(col, row, cancel_rect) || !point_in(col, row, popup) {
            app.confirm_clear_queue = false;
        }
    }
}

/// Quit confirmation: quit on Quit, dismiss otherwise.
fn handle_quit_confirm_mouse(app: &mut App, mouse: MouseEvent, terminal_area: Rect) {
    let (col, row) = (mouse.column, mouse.row);
    if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
        let (popup, [quit_rect, cancel_rect]) = ui::compute_quit_button_rects(terminal_area);
        if point_in(col, row, quit_rect) {
            app.confirm_quit = false;
            app.should_quit = true;
        } else if point_in(col, row, cancel_rect) || !point_in(col, row, popup) {
            app.confirm_quit = false;
            app.quit_selected_button = 1;
        }
    }
}

/// Delete-queue-item dialog: run the chosen action, dismiss on outside click.
fn handle_delete_queue_mouse(
    app: &mut App,
    mouse: MouseEvent,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
    terminal_area: Rect,
    idx: usize,
) {
    let (col, row) = (mouse.column, mouse.row);
    if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
        let queue_len = app.queue.len();
        let (popup, opt_rects) = ui::compute_delete_queue_button_rects(terminal_area);
        let clicked = opt_rects.iter().enumerate().find_map(|(i, r)| {
            if point_in(col, row, *r) { Some(i as u8) } else { None }
        });
        if let Some(choice) = clicked {
            let enabled = [true, idx > 0, idx + 1 < queue_len, true, true];
            if enabled[choice as usize] {
                app.confirm_delete_queue_item = None;
                app.delete_queue_selected_button = 0;
                if choice < 4 {
                    execute_delete_queue_choice(app, client, tx, idx, choice, queue_len);
                }
            }
        } else if !point_in(col, row, popup) {
            app.confirm_delete_queue_item = None;
        }
    }
}

/// Config modal: save, dismiss, or focus/activate a field.
fn handle_config_modal_mouse(
    app: &mut App,
    mouse: MouseEvent,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
    cfg: &mut config::Config,
    terminal_area: Rect,
) {
    let (col, row) = (mouse.column, mouse.row);
    if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
        let n_servers = app
            .config_modal
            .as_ref()
            .map(|m| m.discovered_servers.len())
            .unwrap_or(0);
        let (modal_area, field_rects) = ui::compute_config_modal_rects(terminal_area, n_servers);
        let (_, btn_rects) = ui::compute_config_modal_button_rects(terminal_area, n_servers);
        if point_in(col, row, btn_rects[0]) {
            apply_config_save(app, cfg, client);
        } else if point_in(col, row, btn_rects[1]) {
            app.config_modal = None;
        } else if let Some(idx) = field_rects.iter().position(|r| point_in(col, row, *r)) {
            if let Some(modal) = app.config_modal.as_mut() {
                modal.selected_field = idx;
            }
            activate_config_field(app, idx, cfg, client, tx);
        } else if !point_in(col, row, modal_area) {
            app.config_modal = None;
        }
    }
}

/// Context menu: scroll selection, run the clicked option, or dismiss.
async fn handle_context_menu_mouse(
    app: &mut App,
    mouse: MouseEvent,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
    terminal_area: Rect,
) {
    let (col, row) = (mouse.column, mouse.row);
    match mouse.kind {
        MouseEventKind::ScrollUp => {
            if let Some(m) = app.context_menu.as_mut()
                && m.selected > 0
            {
                m.selected -= 1;
            }
        }
        MouseEventKind::ScrollDown => {
            if let Some(m) = app.context_menu.as_mut()
                && m.selected + 1 < m.option_count()
            {
                m.selected += 1;
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            let option_count = app
                .context_menu
                .as_ref()
                .map(|m| m.option_count())
                .unwrap_or(0);
            let menu_area = ui::compute_context_menu_rect(terminal_area, option_count);
            if point_in(col, row, menu_area) {
                let opt_top = menu_area.y + 1;
                let opt_bot = menu_area.y + menu_area.height.saturating_sub(1);
                if row >= opt_top && row < opt_bot {
                    let opt_idx = (row - opt_top) as usize;
                    if opt_idx < option_count {
                        if let Some(m) = app.context_menu.as_mut() {
                            m.selected = opt_idx;
                        }
                        execute_context_menu_action(app, client, tx).await;
                    }
                }
            } else {
                app.context_menu = None;
            }
        }
        _ => {}
    }
}

/// Full-art mode: clicks on the image/exit/player close it, control buttons map to
/// playback actions, the progress bar seeks, and the queue list selects/plays a track.
#[allow(clippy::too_many_arguments)]
async fn handle_full_art_mouse(
    app: &mut App,
    mouse: MouseEvent,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
    vol_sync_tx: &mpsc::Sender<(String, u8)>,
    cfg: &mut config::Config,
    terminal_area: Rect,
    main_state: &ListState,
) {
    let (col, row) = (mouse.column, mouse.row);
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let image_rect = ui::compute_full_art_image_rect(terminal_area, app);
            if point_in(col, row, image_rect) {
                app.full_art_mode = false;
                cfg.full_art_mode = false;
                let _ = cfg.save();
                return;
            }
            let exit_rect = ui::compute_full_art_footer_exit_rect(terminal_area);
            if point_in(col, row, exit_rect) {
                app.full_art_mode = false;
                return;
            }
            let ctrl_rects = ui::compute_full_art_control_rects(terminal_area, app);
            if let Some((btn_idx, _)) = ctrl_rects
                .iter()
                .enumerate()
                .find(|(_, r)| point_in(col, row, **r))
            {
                handle_action(
                    app,
                    map_control_button_to_action(btn_idx),
                    client,
                    tx,
                    vol_sync_tx,
                )
                .await;
                return;
            }
            let prog_rect = ui::compute_full_art_progress_rect(terminal_area, app);
            if point_in(col, row, prog_rect) {
                seek_to_click(app, client, col, prog_rect);
                return;
            }
            let queue_rect = ui::compute_full_art_queue_rect(terminal_area, app);
            if point_in(col, row, queue_rect) {
                let inner_top = queue_rect.y + 1;
                let inner_bot = queue_rect.y + queue_rect.height.saturating_sub(1);
                if row >= inner_top && row < inner_bot {
                    let rel = (row - inner_top) as usize;
                    let idx = main_state.offset() + rel / 2;
                    if idx < app.queue.len() {
                        app.main_selected = idx;
                        if let Some(pid) = app.active_pid() {
                            spawn_fire(client, move |c| async move {
                                c.play_track_index(&pid, idx).await
                            });
                        }
                    }
                }
            }
            if let Some(vol_rect) = ui::compute_full_art_footer_vol_icon_rect(terminal_area, app)
                && point_in(col, row, vol_rect)
            {
                if let Some(pid) = app.active_pid() {
                    toggle_mute_player(app, vol_sync_tx, &pid);
                }
                return;
            }
            let player_rect = ui::compute_full_art_footer_player_rect(terminal_area, app);
            if point_in(col, row, player_rect) {
                app.full_art_mode = false;
                cfg.full_art_mode = false;
                let _ = cfg.save();
                app.main_view = MainView::Players;
                app.focus_sidebar = false;
                app.players_focus_global = false;
                app.main_selected = 0;
            }
        }
        MouseEventKind::ScrollUp => {
            let queue_rect = ui::compute_full_art_queue_rect(terminal_area, app);
            if point_in(col, row, queue_rect) {
                app.main_selected = app.main_selected.saturating_sub(1);
            }
        }
        MouseEventKind::ScrollDown => {
            let queue_rect = ui::compute_full_art_queue_rect(terminal_area, app);
            if point_in(col, row, queue_rect) {
                let max = app.queue.len().saturating_sub(1);
                if app.main_selected < max {
                    app.main_selected += 1;
                }
            }
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn handle_mouse_event(
    app: &mut App,
    mouse: MouseEvent,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
    vol_sync_tx: &mpsc::Sender<(String, u8)>,
    terminal_area: Rect,
    sidebar_state: &ListState,
    main_state: &ListState,
    last_main_click: &mut Option<(Instant, usize)>,
    cfg: &mut config::Config,
) {
    let (sidebar_area, main_area) = ui::compute_areas(terminal_area, app.status_height);
    let col = mouse.column;
    let row = mouse.row;

    // Modal/menu overlays each intercept all mouse events while open.
    if app.sync_modal.is_some() {
        handle_sync_modal_mouse(app, mouse, client, terminal_area);
        return;
    }
    if app.confirm_clear_queue {
        handle_clear_queue_mouse(app, mouse, client, tx, terminal_area);
        return;
    }
    if app.confirm_quit {
        handle_quit_confirm_mouse(app, mouse, terminal_area);
        return;
    }
    if let Some(idx) = app.confirm_delete_queue_item {
        handle_delete_queue_mouse(app, mouse, client, tx, terminal_area, idx);
        return;
    }
    if app.config_modal.is_some() {
        handle_config_modal_mouse(app, mouse, client, tx, cfg, terminal_area);
        return;
    }
    if app.context_menu.is_some() {
        handle_context_menu_mouse(app, mouse, client, tx, terminal_area).await;
        return;
    }

    // Full art mode intercepts all mouse events
    if app.full_art_mode {
        handle_full_art_mouse(
            app,
            mouse,
            client,
            tx,
            vol_sync_tx,
            cfg,
            terminal_area,
            main_state,
        )
        .await;
        return;
    }

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Right) => {
            handle_action(app, Action::Back, client, tx, vol_sync_tx).await;
        }
        MouseEventKind::Down(MouseButton::Left) => {
            let ctrl_rects = ui::compute_statusbar_control_rects(
                terminal_area,
                app.status_height,
                app.art_col_w,
            );
            let ctrl_hit = ctrl_rects
                .iter()
                .enumerate()
                .find(|(_, r)| point_in(col, row, **r));
            if let Some((btn_idx, _)) = ctrl_hit {
                handle_action(
                    app,
                    map_control_button_to_action(btn_idx),
                    client,
                    tx,
                    vol_sync_tx,
                )
                .await;
            } else if let Some(bar) = Some(ui::compute_statusbar_progress_rect(
                terminal_area,
                app.status_height,
                app.art_col_w,
                app,
            ))
            .filter(|b| point_in(col, row, *b))
            {
                seek_to_click(app, client, col, bar);
            } else {
                let art_rect =
                    ui::compute_statusbar_art_rect(terminal_area, app.status_height, app.art_col_w);
                if point_in(col, row, art_rect) && app.now_playing.is_some() {
                    app.full_art_mode = true;
                    cfg.full_art_mode = true;
                    let _ = cfg.save();
                    app.main_selected = now_playing_queue_index(app);
                }
            }

            if let Some(vol_rect) =
                ui::compute_statusbar_vol_icon_rect(terminal_area, app.status_height, app)
                && point_in(col, row, vol_rect)
            {
                if let Some(pid) = app.active_pid() {
                    toggle_mute_player(app, vol_sync_tx, &pid);
                }
                return;
            }

            let title_rect = ui::compute_statusbar_title_area(terminal_area, app.status_height);
            if point_in(col, row, title_rect) {
                app.main_view = MainView::Players;
                app.focus_sidebar = false;
                app.players_focus_global = false;
                app.main_selected = 0;
                return;
            }

            let np_title_rect = ui::compute_statusbar_np_title_rect(
                terminal_area,
                app.status_height,
                app.art_col_w,
            );
            if point_in(col, row, np_title_rect) && app.now_playing.is_some() {
                let idx = now_playing_queue_index(app);
                if matches!(app.main_view, MainView::Queue) {
                    app.main_selected = idx;
                } else {
                    app.main_view = MainView::Queue;
                    app.focus_sidebar = false;
                    app.main_selected = idx;
                }
                return;
            }

            let nav_title_rect =
                ui::compute_sidebar_nav_title_rect(terminal_area, app.status_height);
            if !app.full_art_mode && point_in(col, row, nav_title_rect) {
                app.confirm_quit = true;
                app.quit_selected_button = 1;
                return;
            }

            if !app.full_art_mode && point_in(col, row, sidebar_area) {
                app.focus_sidebar = true;
                let inner_top = sidebar_area.y + 1;
                let inner_bot = sidebar_area.y + sidebar_area.height.saturating_sub(1);
                if let Some(idx) =
                    list_index_at_row(row, inner_top, inner_bot, sidebar_state.offset(), 1)
                    && idx < app.sidebar_items.len()
                {
                    app.sidebar_selected = idx;
                    handle_action(app, Action::Select, client, tx, vol_sync_tx).await;
                }
            } else if point_in(col, row, main_area) {
                app.focus_sidebar = false;
                // The local-filter box (when active) steals 3 rows off the top of the panel,
                // pushing the list down. Only filterable views can have it active, so this
                // offset is always 0 for Players/Search.
                let filter_off = if app.local_filter.is_some() { 3 } else { 0 };
                let inner_top = main_area.y + 1 + filter_off;
                let inner_bot = main_area.y + main_area.height.saturating_sub(1);

                // Players view has a special layout: power buttons + global row on top,
                // then per-player rows. Handle it independently.
                if matches!(app.main_view, MainView::Players) {
                    let inner_x = main_area.x + 1;
                    // +1 for the pill endcap / leading space on each row
                    let pwr_start_x = inner_x + 1;
                    let pwr_end_x = pwr_start_x + ui::PLAYERS_PWR_BTN_W;

                    // Use the same shared layout helper draw_players renders with, so click
                    // targets always match what's drawn.
                    let ui::PlayersRowLayout {
                        vol_str_w,
                        bar_w: player_bar_w,
                        name_col_w: player_name_col_w,
                    } = ui::players_row_layout(main_area, app.use_nerd_icons);
                    // label = " {name}  " → 1 + name_col_w + 2 = name_col_w + 3
                    let label_w = player_name_col_w + 3;
                    let sync_btn_x = pwr_end_x + label_w as u16;
                    let sync_btn_end = sync_btn_x + ui::PLAYERS_SYNC_BTN_W;
                    // Vol str starts at same column for global and per-player rows (bars are aligned)
                    let vol_str_start_x = sync_btn_end + player_bar_w as u16;

                    if row == inner_top {
                        // Global row
                        if col >= pwr_start_x && col < pwr_end_x {
                            // Global power button: toggle all players on/off
                            toggle_all_players_power(app, client);
                        } else if col > pwr_end_x && col < pwr_end_x + 4 {
                            // Checkbox "[x]"/"[ ]": toggle global volume control
                            app.players_focus_global = true;
                            app.global_volume_control = !app.global_volume_control;
                        } else if col >= sync_btn_end && col < sync_btn_end + player_bar_w as u16 {
                            // Volume bar click on global row: set all players to clicked volume
                            let bar = Rect::new(sync_btn_end, row, player_bar_w as u16, 1);
                            for pid in app.player_ids() {
                                set_volume_from_click(app, vol_sync_tx, col, bar, &pid);
                            }
                            app.players_focus_global = true;
                        } else if col >= vol_str_start_x && col < vol_str_start_x + vol_str_w as u16
                        {
                            // Vol icon in global row: mute/unmute all players
                            toggle_mute_all(app, vol_sync_tx);
                        } else {
                            app.players_focus_global = true;
                        }
                    } else if row > inner_top && row < inner_bot {
                        let vis_i = (row - inner_top - 1) as usize;
                        let player_i = main_state.offset() + vis_i;
                        if col >= pwr_start_x && col < pwr_end_x {
                            // Individual player power button
                            if let Some(p) = app.players.get(player_i) {
                                let pid = p.playerid.clone();
                                let turn_on = p.power == 0;
                                spawn_fire(client, move |c| async move {
                                    c.set_power(&pid, turn_on).await
                                });
                            }
                        } else if col >= sync_btn_x && col < sync_btn_end {
                            open_sync_modal(app, player_i);
                        } else if col >= sync_btn_end && col < sync_btn_end + player_bar_w as u16 {
                            // Volume bar click: set this player's volume
                            if let Some(p) = app.players.get(player_i) {
                                let bar = Rect::new(sync_btn_end, row, player_bar_w as u16, 1);
                                let pid = p.playerid.clone();
                                set_volume_from_click(app, vol_sync_tx, col, bar, &pid);
                            }
                            app.players_focus_global = false;
                            app.main_selected = player_i;
                        } else if col >= vol_str_start_x && col < vol_str_start_x + vol_str_w as u16
                        {
                            // Vol icon: mute/unmute this specific player
                            if let Some(p) = app.players.get(player_i) {
                                let pid = p.playerid.clone();
                                toggle_mute_player(app, vol_sync_tx, &pid);
                            }
                        } else if player_i < app.players.len() {
                            app.players_focus_global = false;
                            app.main_selected = player_i;
                            handle_action(app, Action::Select, client, tx, vol_sync_tx).await;
                        }
                    }
                    return;
                }

                // Search view: input box, tab bar, and results have non-standard layout
                if matches!(app.main_view, MainView::Search) {
                    let (input_rect, tab_rect) = ui::compute_search_panel_rects(main_area);
                    if point_in(col, row, input_rect) {
                        app.search_input_active = true;
                        return;
                    }
                    if point_in(col, row, tab_rect) {
                        if let Some(scope) =
                            ui::search_scope_at_col(col, tab_rect, app.use_nerd_icons)
                        {
                            app.search_scope = scope;
                        }
                        return;
                    }
                    // Results start after border(1) + input(3) + tab(1) = 5 rows from main_area.y
                    let results_top = main_area.y + 5;
                    if let Some(idx) =
                        list_index_at_row(row, results_top, inner_bot, main_state.offset(), 2)
                        && idx < app.search_results.len()
                    {
                        let is_double = is_double_click(last_main_click, idx);
                        *last_main_click = Some((Instant::now(), idx));
                        app.main_selected = idx;
                        if is_double || !utils::is_main_item_playable(app) {
                            handle_action(app, Action::Select, client, tx, vol_sync_tx).await;
                        } else {
                            let (add, replace) = utils::compute_parent_labels(app);
                            app.context_menu = Some(ContextMenu::new(add, replace));
                        }
                    }
                    return;
                }

                let row_h = if utils::uses_two_row_layout(&app.main_view) {
                    2
                } else {
                    1
                };
                if let Some(idx) =
                    list_index_at_row(row, inner_top, inner_bot, main_state.offset(), row_h)
                    && idx < utils::main_list_len(app)
                {
                    let is_double = is_double_click(last_main_click, idx);
                    *last_main_click = Some((Instant::now(), idx));

                    app.main_selected = idx;

                    if is_double || !utils::is_main_item_playable(app) {
                        handle_action(app, Action::Select, client, tx, vol_sync_tx).await;
                    } else {
                        let (add, replace) = utils::compute_parent_labels(app);
                        app.context_menu = Some(ContextMenu::new(add, replace));
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
            handle_action(app, Action::NavUp, client, tx, vol_sync_tx).await;
        }
        MouseEventKind::ScrollDown => {
            if point_in(col, row, sidebar_area) {
                app.focus_sidebar = true;
            } else if point_in(col, row, main_area) {
                app.focus_sidebar = false;
            }
            handle_action(app, Action::NavDown, client, tx, vol_sync_tx).await;
        }
        _ => {}
    }
}

pub async fn handle_context_menu_key(
    app: &mut App,
    key: KeyEvent,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    let Some(menu) = app.context_menu.as_mut() else {
        return;
    };
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            if menu.selected > 0 {
                menu.selected -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if menu.selected < menu.option_count() - 1 {
                menu.selected += 1;
            }
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

pub fn open_sync_modal(app: &mut App, player_index: usize) {
    let Some(player) = app.players.get(player_index) else {
        return;
    };
    let pid = player.playerid.clone();
    let player_name = player.name.clone();
    let synced_ids = app
        .player_sync_groups
        .get(&pid)
        .cloned()
        .unwrap_or_default();
    let other_players: Vec<_> = app
        .players
        .iter()
        .filter(|p| p.playerid != pid)
        .cloned()
        .collect();
    let checked = other_players
        .iter()
        .map(|p| {
            synced_ids
                .iter()
                .any(|id| id.eq_ignore_ascii_case(&p.playerid))
        })
        .collect();
    app.sync_modal = Some(SyncModal {
        player_id: pid,
        player_name,
        initial_synced_ids: synced_ids,
        other_players,
        checked,
        list_selected: 0,
        focus_buttons: false,
        selected_button: 0,
    });
}

fn apply_sync(app: &mut App, client: &Arc<LmsClient>) {
    let Some(modal) = app.sync_modal.take() else {
        return;
    };
    let now_checked: Vec<String> = modal
        .other_players
        .iter()
        .zip(modal.checked.iter())
        .filter(|(_, c)| **c)
        .map(|(p, _)| p.playerid.clone())
        .collect();

    // Unsync players that were synced but are now unchecked
    for pid in &modal.initial_synced_ids {
        if !now_checked.contains(pid) {
            let pid = pid.clone();
            spawn_fire(client, move |c| async move { c.unsync(&pid).await });
        }
    }
    // Sync newly checked players (each joins player_id's group)
    for pid in &now_checked {
        let pid = pid.clone();
        let master = modal.player_id.clone();
        spawn_fire(
            client,
            move |c| async move { c.sync_with(&pid, &master).await },
        );
    }
    // If all are unchecked and self was in a group, unsync self too
    if now_checked.is_empty() && !modal.initial_synced_ids.is_empty() {
        let pid = modal.player_id.clone();
        spawn_fire(client, move |c| async move { c.unsync(&pid).await });
    }
}

pub async fn handle_sync_modal_key(app: &mut App, key: KeyEvent, client: &Arc<LmsClient>) {
    let Some(modal) = app.sync_modal.as_mut() else {
        return;
    };
    let n = modal.other_players.len();
    match key.code {
        KeyCode::Esc => {
            app.sync_modal = None;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if !modal.focus_buttons {
                if modal.list_selected > 0 {
                    modal.list_selected -= 1;
                }
            } else {
                modal.focus_buttons = false;
                if n > 0 {
                    modal.list_selected = n - 1;
                }
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if !modal.focus_buttons {
                if modal.list_selected + 1 < n {
                    modal.list_selected += 1;
                } else {
                    modal.focus_buttons = true;
                }
            }
        }
        KeyCode::Tab => {
            modal.focus_buttons = !modal.focus_buttons;
        }
        KeyCode::Char(' ') => {
            if !modal.focus_buttons
                && let Some(c) = modal.checked.get_mut(modal.list_selected)
            {
                *c = !*c;
            }
        }
        KeyCode::Left | KeyCode::Char('h') => {
            if modal.focus_buttons {
                modal.selected_button = 0;
            }
        }
        KeyCode::Right | KeyCode::Char('l') => {
            if modal.focus_buttons {
                modal.selected_button = 1;
            }
        }
        KeyCode::Enter => {
            if modal.focus_buttons {
                if modal.selected_button == 0 {
                    apply_sync(app, client);
                } else {
                    app.sync_modal = None;
                }
            } else {
                // Toggle checkbox on Enter when list is focused
                if let Some(c) = modal.checked.get_mut(modal.list_selected) {
                    *c = !*c;
                }
            }
        }
        _ => {}
    }
}

pub async fn handle_confirm_clear_queue_key(
    app: &mut App,
    key: KeyEvent,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    match key.code {
        KeyCode::Tab | KeyCode::Right | KeyCode::Left => {
            app.clear_queue_selected_button = 1 - app.clear_queue_selected_button;
        }
        KeyCode::Char('y') => {
            app.confirm_clear_queue = false;
            app.clear_queue_selected_button = 0;
            if let Some(pid) = app.active_pid() {
                spawn_status(client, tx, "Queue cleared".to_string(), move |c| async move {
                    c.clear_queue(&pid).await
                });
            }
        }
        KeyCode::Enter => {
            let confirmed = app.clear_queue_selected_button == 0;
            app.confirm_clear_queue = false;
            app.clear_queue_selected_button = 0;
            if confirmed && let Some(pid) = app.active_pid() {
                spawn_status(client, tx, "Queue cleared".to_string(), move |c| async move {
                    c.clear_queue(&pid).await
                });
            }
        }
        KeyCode::Esc => {
            app.confirm_clear_queue = false;
            app.clear_queue_selected_button = 0;
        }
        _ => {}
    }
}

pub async fn handle_confirm_quit_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Tab | KeyCode::Right | KeyCode::Left => {
            app.quit_selected_button = 1 - app.quit_selected_button;
        }
        KeyCode::Char('y') => {
            app.confirm_quit = false;
            app.should_quit = true;
        }
        KeyCode::Enter => {
            let confirmed = app.quit_selected_button == 0;
            app.confirm_quit = false;
            app.quit_selected_button = 1;
            if confirmed {
                app.should_quit = true;
            }
        }
        KeyCode::Esc | KeyCode::Char('n') => {
            app.confirm_quit = false;
            app.quit_selected_button = 1;
        }
        _ => {}
    }
}

fn next_enabled_delete_option(sel: u8, dir: i8, enabled: &[bool; 5]) -> u8 {
    let mut s = sel as i8;
    for _ in 0..5 {
        s = (s + dir).rem_euclid(5);
        if enabled[s as usize] { return s as u8; }
    }
    sel
}

/// Execute one of the four queue-removal choices (0=this, 1=before, 2=after, 3=all).
fn execute_delete_queue_choice(
    app: &mut App,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
    idx: usize,
    choice: u8,
    queue_len: usize,
) {
    let Some(pid) = app.active_pid() else { return };
    match choice {
        0 => {
            if idx < app.queue.len() {
                let name = app.queue[idx].title.clone();
                spawn_status(client, tx, format!("Removed \"{}\" from queue", name),
                    move |c| async move { c.delete_queue_item(&pid, idx).await });
                app.queue.remove(idx);
                if !app.queue.is_empty() && app.main_selected >= app.queue.len() {
                    app.main_selected = app.queue.len() - 1;
                }
            }
        }
        1 => {
            if idx > 0 {
                spawn_status(client, tx, format!("Removed {} songs before this", idx),
                    move |c| async move { c.delete_queue_items_before(&pid, idx).await });
                app.queue.drain(0..idx);
                app.main_selected = 0;
            }
        }
        2 => {
            let after = queue_len.saturating_sub(idx + 1);
            if after > 0 {
                spawn_status(client, tx, format!("Removed {} songs after this", after),
                    move |c| async move { c.delete_queue_items_after(&pid, idx, queue_len).await });
                app.queue.truncate(idx + 1);
                if app.main_selected > idx { app.main_selected = idx; }
            }
        }
        3 => {
            spawn_status(client, tx, "Queue cleared".to_string(),
                move |c| async move { c.clear_queue(&pid).await });
            app.queue.clear();
            app.main_selected = 0;
        }
        _ => {}
    }
}

pub async fn handle_confirm_delete_queue_item_key(
    app: &mut App,
    key: KeyEvent,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    let Some(idx) = app.confirm_delete_queue_item else { return };
    let queue_len = app.queue.len();
    let enabled = [true, idx > 0, idx + 1 < queue_len, true, true];
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            app.delete_queue_selected_button =
                next_enabled_delete_option(app.delete_queue_selected_button, -1, &enabled);
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
            app.delete_queue_selected_button =
                next_enabled_delete_option(app.delete_queue_selected_button, 1, &enabled);
        }
        KeyCode::Char('y') | KeyCode::Char('d') => {
            app.confirm_delete_queue_item = None;
            app.delete_queue_selected_button = 0;
            execute_delete_queue_choice(app, client, tx, idx, 0, queue_len);
        }
        KeyCode::Enter => {
            let choice = app.delete_queue_selected_button;
            app.confirm_delete_queue_item = None;
            app.delete_queue_selected_button = 0;
            if choice < 4 {
                execute_delete_queue_choice(app, client, tx, idx, choice, queue_len);
            }
        }
        KeyCode::Esc | KeyCode::Char('q') => {
            app.confirm_delete_queue_item = None;
            app.delete_queue_selected_button = 0;
        }
        _ => {}
    }
}

async fn execute_context_menu_action(
    app: &mut App,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    let Some(menu) = app.context_menu.take() else {
        return;
    };
    match menu.selected {
        0 => handle_main_select(app, client, tx).await,
        1 => handle_insert_next(app, client, tx).await,
        2 => handle_add_to_queue(app, client, tx).await,
        3 => handle_replace_queue(app, client, tx).await,
        4 => handle_add_to_favorites(app, client, tx).await,
        5 => handle_add_parent_to_queue(app, client, tx).await,
        6 => handle_replace_queue_with_parent(app, client, tx).await,
        _ => {}
    }
}

async fn handle_insert_next(app: &mut App, client: &Arc<LmsClient>, tx: &mpsc::Sender<AppMsg>) {
    let Some(pid) = app.active_pid() else {
        app.status_message = Some("No active player".to_string());
        return;
    };

    let next_msg = |name: &str| format!("\"{}\" will play next", name);
    match app.main_view.clone() {
        MainView::Library(LibraryView::Playlists) => {
            if let Some(playlist) = app.playlists.get(app.main_selected) {
                let id = utils::json_id_to_string(&playlist.id);
                let name = playlist.name.clone();
                spawn_status(client, tx, next_msg(&name), move |c| async move {
                    c.insert_playlist_next(&pid, &id).await
                });
            }
        }
        MainView::Library(LibraryView::Tracks { .. }) => {
            if let Some(track) = app.tracks.get(app.main_selected) {
                let id = utils::extract_id(track.id.as_ref());
                let name = track.title.clone();
                spawn_status(client, tx, next_msg(&name), move |c| async move {
                    c.insert_track_next(&pid, &id).await
                });
            }
        }
        MainView::Queue => {
            if let Some(track) = app.queue.get(app.main_selected) {
                let name = track.title.clone();
                if let Some(id_val) = &track.id {
                    let id = utils::json_id_to_string(id_val);
                    spawn_status(client, tx, next_msg(&name), move |c| async move {
                        c.insert_track_next(&pid, &id).await
                    });
                }
            }
        }
        MainView::Library(LibraryView::Folder { .. }) => {
            if let Some(item) = app.folder_items.get(app.main_selected).cloned()
                && item.item_type == FolderItemType::Track
            {
                let id = item.id.to_string();
                let name = item.filename.clone();
                spawn_status(client, tx, next_msg(&name), move |c| async move {
                    c.insert_track_next(&pid, &id).await
                });
            }
        }
        MainView::Radio => {
            if let Some(item) = app.radio_items.get(app.main_selected).cloned()
                && let Some(url) = item.url
            {
                let name = item.name.clone();
                spawn_status(client, tx, next_msg(&name), move |c| async move {
                    c.insert_url_next_with_title(&pid, &url, &name).await
                });
            }
        }
        MainView::Apps => {
            if let Some(item) = app.app_items.get(app.main_selected).cloned()
                && let Some(url) = item.url
            {
                let name = item.name.clone();
                spawn_status(client, tx, next_msg(&name), move |c| async move {
                    c.insert_url_next_with_title(&pid, &url, &name).await
                });
            }
        }
        MainView::AppSearch { .. } => {
            if let Some(item) = app.app_search_results.get(app.main_selected).cloned()
                && let Some(url) = item.url
            {
                let name = item.name.clone();
                spawn_status(client, tx, next_msg(&name), move |c| async move {
                    c.insert_url_next_with_title(&pid, &url, &name).await
                });
            }
        }
        MainView::Favourites => {
            if let Some(item) = app.fav_items.get(app.main_selected).cloned()
                && let Some(url) = item.url
            {
                let name = item.name.clone();
                spawn_status(client, tx, next_msg(&name), move |c| async move {
                    c.insert_url_next_with_title(&pid, &url, &name).await
                });
            }
        }
        MainView::Search => match app.search_results.get(app.main_selected).cloned() {
            Some(SearchResultItem::Track(track)) => {
                let id = utils::extract_id(track.id.as_ref());
                let name = track.title.clone();
                spawn_status(client, tx, next_msg(&name), move |c| async move {
                    c.insert_track_next(&pid, &id).await
                });
            }
            Some(SearchResultItem::Playlist(pl)) => {
                let id = utils::json_id_to_string(&pl.id);
                let name = pl.name.clone();
                spawn_status(client, tx, next_msg(&name), move |c| async move {
                    c.insert_playlist_next(&pid, &id).await
                });
            }
            _ => {}
        },
        _ => {}
    }
}

async fn handle_add_to_favorites(
    app: &mut App,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    let Some(pid) = app.active_pid() else {
        app.status_message = Some("No active player".to_string());
        return;
    };

    match app.main_view.clone() {
        MainView::Library(LibraryView::Tracks { .. }) => {
            if let Some(track) = app.tracks.get(app.main_selected) {
                let id = utils::extract_id(track.id.as_ref());
                let name = track.title.clone();
                spawn_add_to_favorites(client, tx, pid, format!("db:track.id={}", id), name);
            }
        }
        MainView::Radio => {
            if let Some(item) = app.radio_items.get(app.main_selected).cloned()
                && let Some(url) = item.url
            {
                spawn_add_to_favorites(client, tx, pid, url, item.name);
            }
        }
        MainView::Apps | MainView::AppSearch { .. } => {
            let item = if matches!(app.main_view, MainView::Apps) {
                app.app_items.get(app.main_selected).cloned()
            } else {
                app.app_search_results.get(app.main_selected).cloned()
            };
            if let Some(item) = item
                && let Some(url) = item.url
            {
                spawn_add_to_favorites(client, tx, pid, url, item.name);
            }
        }
        MainView::Search => {
            if let Some(SearchResultItem::Track(track)) =
                app.search_results.get(app.main_selected).cloned()
            {
                let id = utils::extract_id(track.id.as_ref());
                spawn_add_to_favorites(client, tx, pid, format!("db:track.id={}", id), track.title);
            }
        }
        _ => {
            app.status_message = Some("Cannot add this item to favourites".to_string());
        }
    }
}

fn toggle_mute_player(app: &mut App, vol_sync_tx: &mpsc::Sender<(String, u8)>, pid: &str) {
    let current = app.player_volumes.get(pid).copied().unwrap_or(0);
    let new_vol = if current > 0 {
        app.muted_volumes.insert(pid.to_string(), current);
        0u8
    } else {
        app.muted_volumes.remove(pid).unwrap_or(50)
    };
    app.player_volumes.insert(pid.to_string(), new_vol);
    app.volume_pending
        .insert(pid.to_string(), std::time::Instant::now());
    let _ = vol_sync_tx.try_send((pid.to_string(), new_vol));
    if app.active_player.as_deref() == Some(pid)
        && let Some(np) = app.now_playing.as_mut()
    {
        np.volume = new_vol;
    }
}

/// Mute/unmute every known player.
fn toggle_mute_all(app: &mut App, vol_sync_tx: &mpsc::Sender<(String, u8)>) {
    for pid in app.player_ids() {
        toggle_mute_player(app, vol_sync_tx, &pid);
    }
}

/// Power every player on if any is currently off, otherwise power all off.
fn toggle_all_players_power(app: &App, client: &Arc<LmsClient>) {
    let all_on = !app.players.is_empty() && app.players.iter().all(|p| p.power > 0);
    let turn_on = !all_on;
    let pids = app.player_ids();
    let c = client.clone();
    tokio::spawn(async move {
        for pid in pids {
            let _ = c.set_power(&pid, turn_on).await;
        }
    });
}

/// Current volume for `pid`, defaulting to 50 when the player hasn't reported one yet.
fn player_volume_or_default(app: &App, pid: &str) -> u8 {
    app.player_volumes.get(pid).copied().unwrap_or(50)
}

/// Record a new local volume for `pid` and queue it for debounced sync to the server:
/// update the cached volume, mark it locally-pending so polls don't clobber it, and send it
/// down the volume-sync channel.
fn bump_player_volume(
    app: &mut App,
    pid: &str,
    nv: u8,
    now: std::time::Instant,
    vol_sync_tx: &mpsc::Sender<(String, u8)>,
) {
    app.player_volumes.insert(pid.to_string(), nv);
    app.volume_pending.insert(pid.to_string(), now);
    let _ = vol_sync_tx.try_send((pid.to_string(), nv));
}

fn adjust_volume(app: &mut App, vol_sync_tx: &mpsc::Sender<(String, u8)>, delta: i16) {
    let new_vol = |v: u8| -> u8 { ((v as i16) + delta).clamp(0, 100) as u8 };

    let now = std::time::Instant::now();
    if app.global_volume_control {
        for pid in app.player_ids() {
            let nv = new_vol(player_volume_or_default(app, &pid));
            bump_player_volume(app, &pid, nv, now, vol_sync_tx);
        }
        if let Some(active_pid) = app.active_pid()
            && let Some(nv) = app.player_volumes.get(&active_pid).copied()
            && let Some(np) = app.now_playing.as_mut()
        {
            np.volume = nv;
        }
    } else if let MainView::Players = &app.main_view {
        if app.players_focus_global {
            for pid in app.player_ids() {
                let nv = new_vol(player_volume_or_default(app, &pid));
                bump_player_volume(app, &pid, nv, now, vol_sync_tx);
            }
        } else if let Some(player) = app.players.get(app.main_selected) {
            let pid = player.playerid.clone();
            let nv = new_vol(player_volume_or_default(app, &pid));
            bump_player_volume(app, &pid, nv, now, vol_sync_tx);
            if app.active_player.as_deref() == Some(&pid)
                && let Some(np) = app.now_playing.as_mut()
            {
                np.volume = nv;
            }
        }
    } else if let Some(pid) = app.active_pid() {
        let nv = new_vol(app.now_playing.as_ref().map(|n| n.volume).unwrap_or(50));
        if let Some(np) = app.now_playing.as_mut() {
            np.volume = nv;
        }
        bump_player_volume(app, &pid, nv, now, vol_sync_tx);
    }
}

/// Returns true if the app should quit.
pub async fn handle_action(
    app: &mut App,
    action: Action,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
    vol_sync_tx: &mpsc::Sender<(String, u8)>,
) -> bool {
    match action {
        Action::Quit => return true,

        Action::FocusSidebar => {
            if !app.full_art_mode {
                if matches!(app.main_view, MainView::Players) && !app.focus_sidebar {
                    adjust_volume(app, vol_sync_tx, -5);
                } else {
                    app.focus_sidebar = true;
                    app.players_focus_global = false;
                    app.search_input_active = false;
                    app.app_search_input_active = false;
                }
            }
        }
        Action::FocusMain => {
            if matches!(app.main_view, MainView::Players) && !app.focus_sidebar {
                adjust_volume(app, vol_sync_tx, 5);
            } else {
                app.focus_sidebar = false;
                if matches!(app.main_view, MainView::Search) {
                    app.search_input_active = true;
                }
                if matches!(app.main_view, MainView::AppSearch { .. }) {
                    app.app_search_input_active = true;
                }
            }
        }
        Action::ToggleFocus => {
            if !app.full_art_mode {
                app.focus_sidebar = !app.focus_sidebar;
                app.players_focus_global = false;
                app.search_input_active =
                    !app.focus_sidebar && matches!(app.main_view, MainView::Search);
                app.app_search_input_active =
                    !app.focus_sidebar && matches!(app.main_view, MainView::AppSearch { .. });
            }
        }

        Action::NavUp => {
            if app.full_art_mode {
                app.main_selected = app.main_selected.saturating_sub(1);
            } else if app.focus_sidebar {
                app.sidebar_selected = app.sidebar_selected.saturating_sub(1);
            } else if let MainView::Help = &app.main_view {
                app.help_scroll = app.help_scroll.saturating_sub(1);
            } else if let MainView::Players = &app.main_view {
                if !app.players_focus_global && app.main_selected == 0 {
                    app.players_focus_global = true;
                } else if !app.players_focus_global {
                    app.main_selected -= 1;
                }
            } else if matches!(app.main_view, MainView::Search)
                && !app.search_input_active
                && app.main_selected == 0
            {
                app.search_input_active = true;
            } else if matches!(app.main_view, MainView::AppSearch { .. })
                && !app.app_search_input_active
                && app.main_selected == 0
            {
                app.app_search_input_active = true;
            } else if app.local_filter.as_ref().is_some_and(|f| !f.editing)
                && app.main_selected == 0
            {
                // At the top of a filtered list, ↑/k re-enters the filter input box.
                if let Some(f) = &mut app.local_filter {
                    f.editing = true;
                }
            } else {
                app.main_selected = app.main_selected.saturating_sub(1);
            }
        }

        Action::NavDown => {
            if app.full_art_mode {
                let max = app.queue.len().saturating_sub(1);
                if app.main_selected < max {
                    app.main_selected += 1;
                }
            } else if app.focus_sidebar {
                let max = app.sidebar_items.len().saturating_sub(1);
                if app.sidebar_selected < max {
                    app.sidebar_selected += 1;
                }
            } else if let MainView::Help = &app.main_view {
                app.help_scroll = app.help_scroll.saturating_add(1).min(help_max_scroll(app));
            } else if let MainView::Players = &app.main_view {
                if app.players_focus_global {
                    app.players_focus_global = false;
                    app.main_selected = 0;
                } else {
                    let max = utils::main_list_len(app).saturating_sub(1);
                    if app.main_selected < max {
                        app.main_selected += 1;
                    }
                }
            } else {
                let max = utils::main_list_len(app).saturating_sub(1);
                if app.main_selected < max {
                    app.main_selected += 1;
                }
            }
        }

        Action::PageUp => {
            if !app.focus_sidebar {
                if let MainView::Help = &app.main_view {
                    app.help_scroll = app.help_scroll.saturating_sub(10);
                } else {
                    app.main_selected = app.main_selected.saturating_sub(10);
                }
            }
        }

        Action::PageDown => {
            if !app.focus_sidebar {
                if let MainView::Help = &app.main_view {
                    app.help_scroll = app.help_scroll.saturating_add(10).min(help_max_scroll(app));
                } else {
                    let max = utils::main_list_len(app).saturating_sub(1);
                    app.main_selected = (app.main_selected + 10).min(max);
                }
            }
        }

        Action::Home => {
            if !app.focus_sidebar {
                if let MainView::Help = &app.main_view {
                    app.help_scroll = 0;
                } else {
                    app.main_selected = 0;
                }
            }
        }

        Action::End => {
            if !app.focus_sidebar {
                if let MainView::Help = &app.main_view {
                    app.help_scroll = help_max_scroll(app);
                } else {
                    app.main_selected = utils::main_list_len(app).saturating_sub(1);
                }
            }
        }

        Action::Select => {
            if app.full_art_mode {
                if let Some(pid) = app.active_pid() {
                    let idx = app.main_selected;
                    spawn_fire(client, move |c| async move {
                        c.play_track_index(&pid, idx).await
                    });
                }
            } else if app.focus_sidebar {
                activate_sidebar_item(app, client, tx).await;
            } else if utils::is_main_item_playable(app) {
                let (add, replace) = utils::compute_parent_labels(app);
                app.context_menu = Some(ContextMenu::new(add, replace));
            } else {
                handle_main_select(app, client, tx).await;
            }
        }

        Action::Back => {
            if app.local_filter.is_some() {
                // While a local filter is applied (and not editing — those keys are routed to
                // handle_local_filter_key upstream), Back/Esc/Backspace clears the filter and
                // stays on the same view rather than navigating away.
                filter::clear(app);
            } else if !app.focus_sidebar {
                match &app.main_view.clone() {
                    MainView::Library(LibraryView::Tracks { album_id: Some(_) }) => {
                        if let Some(prev) = app.previous_view.take() {
                            app.main_view = prev;
                        } else {
                            app.main_view =
                                MainView::Library(LibraryView::Albums { artist_id: None });
                            app.main_selected = 0;
                        }
                    }
                    MainView::Library(LibraryView::Albums { artist_id: Some(_) }) => {
                        if let Some(prev) = app.previous_view.take() {
                            app.main_view = prev;
                        } else {
                            app.main_view = MainView::Library(LibraryView::Artists);
                            app.main_selected = 0;
                        }
                    }
                    MainView::Library(LibraryView::Folder { .. }) => {
                        if let Some(prev) = app.folder_nav_stack.pop() {
                            app.folder_items = prev.items;
                            app.main_selected = prev.selected;
                            app.folder_title = prev.title;
                            app.main_view = MainView::Library(LibraryView::Folder {
                                folder_id: prev.folder_id,
                            });
                        } else {
                            app.main_view = MainView::MyMusic;
                            app.main_selected = 0;
                        }
                    }
                    MainView::Library(LibraryView::Artists)
                    | MainView::Library(LibraryView::AlbumArtists)
                    | MainView::Library(LibraryView::Albums { artist_id: None })
                    | MainView::Library(LibraryView::Tracks { album_id: None })
                    | MainView::Library(LibraryView::Playlists)
                    | MainView::Library(LibraryView::RecentlyPlayedArtists)
                    | MainView::Library(LibraryView::PopularAlbums) => {
                        if let Some(prev) = app.previous_view.take() {
                            app.main_view = prev;
                        } else {
                            app.main_view = MainView::MyMusic;
                            app.main_selected = 0;
                        }
                    }
                    MainView::MyMusic => {
                        app.focus_sidebar = true;
                        app.players_focus_global = false;
                    }
                    MainView::Radio => {
                        if let Some(prev) = app.radio_nav_stack.pop() {
                            app.radio_items = prev.items;
                            app.main_selected = prev.selected;
                            app.radio_title = prev.title;
                        } else if let Some(prev) = app.previous_view.take() {
                            app.main_view = prev;
                        } else {
                            app.focus_sidebar = true;
                        }
                    }
                    MainView::Apps => {
                        if let Some(prev) = app.app_nav_stack.pop() {
                            app.app_items = prev.items;
                            app.main_selected = prev.selected;
                            app.app_title = prev.title;
                        } else if let Some(prev) = app.previous_view.take() {
                            app.main_view = prev;
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
                    MainView::Search => {
                        app.focus_sidebar = true;
                        app.search_input_active = false;
                    }
                    MainView::AppSearch { .. } => {
                        if app.app_search_input_active {
                            app.app_search_input_active = false;
                        } else {
                            app.main_view = MainView::Apps;
                            app.app_search_query = String::new();
                            app.app_search_cursor_pos = 0;
                            app.app_search_results = vec![];
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
            if let Some(pid) = app.active_pid() {
                let c = client.clone();
                let playing = app.is_playing();
                tokio::spawn(async move {
                    let _ = if playing {
                        c.pause(&pid).await
                    } else {
                        c.play(&pid).await
                    };
                });
            }
        }

        Action::Next => {
            if let Some(pid) = app.active_pid() {
                spawn_fire(client, move |c| async move { c.next(&pid).await });
            }
        }

        Action::Stop => {
            if let Some(pid) = app.active_pid() {
                spawn_fire(client, move |c| async move { c.stop(&pid).await });
            }
        }

        Action::Prev => {
            if let Some(pid) = app.active_pid() {
                spawn_fire(client, move |c| async move { c.prev(&pid).await });
            }
        }

        Action::VolumeUp => adjust_volume(app, vol_sync_tx, 5),

        Action::VolumeDown => adjust_volume(app, vol_sync_tx, -5),

        Action::ToggleMute => {
            if app.global_volume_control {
                toggle_mute_all(app, vol_sync_tx);
            } else if let MainView::Players = &app.main_view {
                if app.players_focus_global {
                    toggle_mute_all(app, vol_sync_tx);
                } else if let Some(player) = app.players.get(app.main_selected) {
                    let pid = player.playerid.clone();
                    toggle_mute_player(app, vol_sync_tx, &pid);
                }
            } else if let Some(pid) = app.active_pid() {
                toggle_mute_player(app, vol_sync_tx, &pid);
            }
        }

        Action::TogglePower => {
            if let MainView::Players = &app.main_view {
                if app.players_focus_global {
                    toggle_all_players_power(app, client);
                } else if let Some(player) = app.players.get(app.main_selected) {
                    let pid = player.playerid.clone();
                    let turn_on = player.power == 0;
                    spawn_fire(
                        client,
                        move |c| async move { c.set_power(&pid, turn_on).await },
                    );
                }
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
                app.clear_queue_selected_button = 0;
            }
        }

        Action::ToggleShuffle => {
            if let (Some(pid), Some(np)) = (app.active_pid(), app.now_playing.as_ref()) {
                let new_val = if np.shuffle > 0 { 0u8 } else { 1 };
                spawn_fire(client, move |c| async move {
                    c.set_shuffle(&pid, new_val).await
                });
            }
        }

        Action::ToggleRepeat => {
            if let (Some(pid), Some(np)) = (app.active_pid(), app.now_playing.as_ref()) {
                let new_val = match np.repeat {
                    0 => 1u8, // off → repeat single track
                    1 => 2u8, // repeat single → repeat queue
                    2 => 3u8, // repeat queue → don't stop the music
                    _ => 0u8, // don't stop → off
                };
                spawn_fire(
                    client,
                    move |c| async move { c.set_repeat(&pid, new_val).await },
                );
            }
        }

        Action::ToggleFullArtMode => {
            if !app.full_art_mode {
                app.saved_main_selected = Some(app.main_selected);
                app.focus_sidebar = false;
                app.main_selected = now_playing_queue_index(app);
            } else if let Some(saved) = app.saved_main_selected.take() {
                app.main_selected = saved;
            }
            app.full_art_mode = !app.full_art_mode;
        }

        Action::DeleteQueueItem => {
            let in_queue = matches!(app.main_view, MainView::Queue) || app.full_art_mode;
            if in_queue && !app.focus_sidebar {
                let idx = app.main_selected;
                if app.active_player.is_some() && idx < app.queue.len() {
                    app.confirm_delete_queue_item = Some(idx);
                    app.delete_queue_selected_button = 0;
                }
            }
        }

        Action::ScopePrev => {
            if matches!(app.main_view, MainView::Search) && !app.focus_sidebar {
                cycle_search_scope(false, app, client, tx);
            }
        }

        Action::ScopeNext => {
            if matches!(app.main_view, MainView::Search) && !app.focus_sidebar {
                cycle_search_scope(true, app, client, tx);
            }
        }

        Action::NavToSidebar(idx) => {
            if idx < app.sidebar_items.len() {
                app.sidebar_selected = idx;
                activate_sidebar_item(app, client, tx).await;
            }
        }

        Action::OpenLocalFilter => {
            if !utils::has_overlay(app) {
                if let Some(f) = &mut app.local_filter {
                    // Re-open editing on the existing query.
                    f.editing = true;
                    app.focus_sidebar = false;
                } else if filter::is_filterable(&app.main_view) {
                    filter::open(app);
                    app.focus_sidebar = false;
                }
            }
        }

        Action::OpenConfig | Action::None => {}
    }
    false
}

/// Reset and focus a top-level browse view (Radio/Apps/Favourites) before kicking off its
/// background load: clear the view's item list, nav stack and title, then set the shared
/// focus/loading flags. The caller still spawns the appropriate `background::load_*` task.
fn enter_browse_view(app: &mut App, view: MainView) {
    match view {
        MainView::Radio => {
            app.radio_items.clear();
            app.radio_nav_stack.clear();
            app.radio_title = "Radio".to_string();
        }
        MainView::Apps => {
            app.app_items.clear();
            app.app_nav_stack.clear();
            app.app_title = "Apps".to_string();
        }
        MainView::Favourites => {
            app.fav_items.clear();
            app.fav_nav_stack.clear();
            app.fav_title = "Favourites".to_string();
        }
        _ => {}
    }
    app.main_view = view;
    app.focus_sidebar = false;
    app.is_loading = true;
}

/// Push the current browse view onto a nav stack so we can return to it after descending into
/// a child item.
fn push_nav(
    stack: &mut Vec<RadioNav>,
    title: String,
    items: Vec<crate::api::RadioItem>,
    selected: usize,
) {
    stack.push(RadioNav {
        title,
        items,
        selected,
    });
}

async fn activate_sidebar_item(app: &mut App, client: &Arc<LmsClient>, tx: &mpsc::Sender<AppMsg>) {
    app.main_selected = 0;
    app.players_focus_global = false;
    match app.sidebar_items.get(app.sidebar_selected).cloned() {
        Some(SidebarItem::MyMusic) => {
            app.main_view = MainView::MyMusic;
            app.main_selected = 0;
            app.focus_sidebar = false;
        }
        Some(SidebarItem::Radio) => {
            enter_browse_view(app, MainView::Radio);
            background::load_radio_services(client.clone(), tx.clone());
        }
        Some(SidebarItem::Apps) => {
            enter_browse_view(app, MainView::Apps);
            background::load_app_services(client.clone(), tx.clone());
        }
        Some(SidebarItem::Favourites) => {
            enter_browse_view(app, MainView::Favourites);
            background::load_fav_items(
                app.active_pid().unwrap_or_default(),
                None,
                client.clone(),
                tx.clone(),
            );
        }
        Some(SidebarItem::Queue) => {
            app.main_view = MainView::Queue;
            app.focus_sidebar = false;
            app.main_selected = now_playing_queue_index(app);
        }
        Some(SidebarItem::Players) => {
            app.main_view = MainView::Players;
            app.focus_sidebar = false;
        }
        Some(SidebarItem::Search) => {
            app.main_view = MainView::Search;
            app.search_input_active = true;
            app.focus_sidebar = false;
            app.main_selected = 0;
        }
        Some(SidebarItem::Help) => {
            app.main_view = MainView::Help;
            app.focus_sidebar = false;
            app.help_scroll = 0;
        }
        None => {}
    }
}

/// Returns the queue index of the currently playing track.
/// Uses `playlist_cur_index` from NowPlaying when available, otherwise falls
/// back to matching by title in the queue vec.
fn now_playing_queue_index(app: &App) -> usize {
    let np = match app.now_playing.as_ref() {
        Some(np) => np,
        None => return 0,
    };
    if let Some(idx) = np.playlist_cur_index
        && idx < app.queue.len()
    {
        return idx;
    }
    // Fallback: find by title
    let title = &np.title;
    app.queue
        .iter()
        .position(|t| &t.title == title)
        .unwrap_or(0)
}

fn set_modal_error(app: &mut App, msg: &str) {
    if let Some(m) = app.config_modal.as_mut() {
        m.error = Some(msg.to_string());
    }
}

fn apply_config_save(app: &mut App, cfg: &mut config::Config, client: &Arc<LmsClient>) {
    let Some(modal) = app.config_modal.as_ref() else {
        return;
    };
    let host = modal.host.trim().to_string();
    let port_str = modal.port.trim().to_string();
    let username = modal.username.trim().to_string();
    let password = modal.password.clone();
    let use_nerd_icons = modal.use_nerd_icons;
    let auto_discover = modal.auto_discover;
    let broadcast_mask = modal.broadcast_mask.trim().to_string();
    let disable_auto_colors = modal.disable_auto_colors;
    let accent_lightness = modal.accent_lightness;
    let image_protocol = IMAGE_PROTOCOLS[modal.image_protocol_idx].to_string();
    // immutable borrow of modal ends here; mutable borrows below are now allowed

    if host.is_empty() {
        set_modal_error(app, "Host cannot be empty");
        return;
    }
    if broadcast_mask.is_empty() {
        set_modal_error(app, "Broadcast mask cannot be empty");
        return;
    }
    match port_str.parse::<u16>() {
        Ok(port) if port > 0 => {
            cfg.host = host;
            cfg.port = port;
            cfg.use_nerd_icons = use_nerd_icons;
            cfg.auto_discover = auto_discover;
            cfg.broadcast_mask = broadcast_mask;
            cfg.disable_auto_colors = disable_auto_colors;
            cfg.accent_lightness = accent_lightness;
            cfg.image_protocol = image_protocol;
            cfg.username = if username.is_empty() {
                None
            } else {
                Some(username.clone())
            };
            cfg.password = if password.is_empty() {
                None
            } else {
                Some(password.clone())
            };
            match cfg.save() {
                Ok(()) => {
                    client.update_base_url(cfg.base_url());
                    client.update_credentials(cfg.credentials());
                    app.use_nerd_icons = use_nerd_icons;
                    app.disable_auto_colors = disable_auto_colors;
                    app.accent_lightness = accent_lightness;
                    app.config_modal = None;
                    app.connection = ConnectionState::Reconnecting;
                    app.players = vec![];
                    app.active_player = None;
                    app.now_playing = None;
                    app.status_message = Some("Reconnecting...".to_string());
                }
                Err(e) => set_modal_error(app, &format!("Save error: {e}")),
            }
        }
        _ => set_modal_error(app, "Invalid port (1–65535)"),
    }
}

/// Activate config-modal field `idx` (the action triggered by pressing Enter on it, or
/// clicking it): toggles flip, the scan button starts discovery, a discovered-server row
/// fills the host field, OK saves, Cancel closes, and text fields enter edit mode. Shared by
/// the keyboard and mouse handlers so the two can't drift apart.
fn activate_config_field(
    app: &mut App,
    idx: usize,
    cfg: &mut config::Config,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    let Some(kind) = app.config_modal.as_ref().map(|m| m.field_kind(idx)) else {
        return;
    };
    match kind {
        FieldKind::ToggleNerd => {
            if let Some(m) = app.config_modal.as_mut() {
                m.use_nerd_icons ^= true;
            }
        }
        FieldKind::ToggleDiscover => {
            if let Some(m) = app.config_modal.as_mut() {
                m.auto_discover ^= true;
            }
        }
        FieldKind::ToggleColors => {
            if let Some(m) = app.config_modal.as_mut() {
                m.disable_auto_colors ^= true;
            }
        }
        FieldKind::SpinnerLightness => {
            if let Some(m) = app.config_modal.as_mut() {
                m.accent_lightness = (m.accent_lightness + 5).min(90);
            }
        }
        FieldKind::SelectorProtocol => {
            if let Some(m) = app.config_modal.as_mut() {
                m.image_protocol_idx = (m.image_protocol_idx + 1) % IMAGE_PROTOCOLS.len();
            }
        }
        FieldKind::ScanButton => {
            if let Some(modal) = app.config_modal.as_mut()
                && !modal.is_scanning
            {
                modal.is_scanning = true;
                modal.scan_attempted = true;
                modal.discovered_servers.clear();
                let mask = modal.broadcast_mask.clone();
                let username = modal.username.clone();
                let password = modal.password.clone();
                let tx2 = tx.clone();
                tokio::spawn(async move {
                    let servers = tokio::task::spawn_blocking(move || {
                        discovery::discover_lms_all(&mask, Duration::from_secs(3))
                    })
                    .await
                    .unwrap_or_default();
                    let handles: Vec<_> = servers
                        .iter()
                        .map(|(ip, port)| {
                            let url = format!("http://{}:{}/jsonrpc.js", ip, port);
                            let creds = if username.is_empty() {
                                None
                            } else {
                                Some((username.clone(), password.clone()))
                            };
                            tokio::spawn(async move {
                                LmsClient::new(url, creds)
                                    .get_server_info()
                                    .await
                                    .ok()
                                    .and_then(|i| i.version)
                            })
                        })
                        .collect();
                    let mut with_versions = Vec::with_capacity(servers.len());
                    for ((ip, port), handle) in servers.into_iter().zip(handles) {
                        let version = handle.await.unwrap_or(None);
                        with_versions.push((ip, port, version));
                    }
                    let _ = tx2.send(AppMsg::DiscoveredServers(with_versions)).await;
                });
            }
        }
        FieldKind::DiscoveredServer(i) => {
            if let Some(modal) = app.config_modal.as_mut()
                && let Some((ip, port, _)) = modal.discovered_servers.get(i).cloned()
            {
                modal.host = ip;
                modal.port = port.to_string();
                modal.selected_field = 0;
            }
        }
        FieldKind::OkButton => apply_config_save(app, cfg, client),
        FieldKind::CancelButton => app.config_modal = None,
        // Text fields (host/port/username/password/broadcast-mask): enter edit mode.
        _ => {
            if let Some(modal) = app.config_modal.as_mut() {
                modal.editing = true;
                modal.error = None;
                modal.cursor_pos = modal.current_field_char_count();
            }
        }
    }
}

pub fn handle_config_key(
    app: &mut App,
    key: KeyEvent,
    cfg: &mut config::Config,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    let editing = app
        .config_modal
        .as_ref()
        .map(|m| m.editing)
        .unwrap_or(false);
    let selected = app.config_modal.as_ref().map(|m| m.selected_field);
    let Some(selected) = selected else { return };

    if editing {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Tab => {
                if let Some(m) = app.config_modal.as_mut() {
                    m.editing = false;
                }
            }
            KeyCode::Left => {
                if let Some(modal) = app.config_modal.as_mut()
                    && modal.cursor_pos > 0
                {
                    modal.cursor_pos -= 1;
                }
            }
            KeyCode::Right => {
                if let Some(modal) = app.config_modal.as_mut() {
                    let len = modal.current_field_char_count();
                    if modal.cursor_pos < len {
                        modal.cursor_pos += 1;
                    }
                }
            }
            KeyCode::Home => {
                if let Some(modal) = app.config_modal.as_mut() {
                    modal.cursor_pos = 0;
                }
            }
            KeyCode::End => {
                if let Some(modal) = app.config_modal.as_mut() {
                    modal.cursor_pos = modal.current_field_char_count();
                }
            }
            KeyCode::Char(c) => {
                if let Some(modal) = app.config_modal.as_mut() {
                    if modal.field_kind(modal.selected_field) == FieldKind::TextPort
                        && !c.is_ascii_digit()
                    {
                        // port field: digits only
                    } else {
                        let cp = modal.cursor_pos;
                        if let Some(field) = modal.current_field_str_mut() {
                            let mut chars: Vec<char> = field.chars().collect();
                            chars.insert(cp, c);
                            *field = chars.into_iter().collect();
                        }
                        modal.cursor_pos += 1;
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(modal) = app.config_modal.as_mut() {
                    let cp = modal.cursor_pos;
                    if cp > 0 {
                        if let Some(field) = modal.current_field_str_mut() {
                            let mut chars: Vec<char> = field.chars().collect();
                            chars.remove(cp - 1);
                            *field = chars.into_iter().collect();
                        }
                        modal.cursor_pos -= 1;
                    }
                }
            }
            KeyCode::Delete => {
                if let Some(modal) = app.config_modal.as_mut() {
                    let cp = modal.cursor_pos;
                    if let Some(field) = modal.current_field_str_mut() {
                        let len = field.chars().count();
                        if cp < len {
                            let mut chars: Vec<char> = field.chars().collect();
                            chars.remove(cp);
                            *field = chars.into_iter().collect();
                        }
                    }
                }
            }
            _ => {}
        }
    } else {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(modal) = app.config_modal.as_mut()
                    && modal.selected_field > 0
                {
                    modal.selected_field -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(modal) = app.config_modal.as_mut() {
                    let max = modal.field_count() - 1;
                    if modal.selected_field < max {
                        modal.selected_field += 1;
                    }
                }
            }
            KeyCode::Tab => {
                if let Some(modal) = app.config_modal.as_mut() {
                    let count = modal.field_count();
                    modal.selected_field = (modal.selected_field + 1) % count;
                }
            }
            KeyCode::Enter | KeyCode::Char('i') => {
                activate_config_field(app, selected, cfg, client, tx);
            }
            // Left/Right navigate the field list like Up/Down, except on the image-protocol
            // selector and accent-lightness spinner where they cycle/adjust the value.
            KeyCode::Left => {
                let kind = app.config_modal.as_ref().map(|m| m.field_kind(selected));
                if let Some(FieldKind::SelectorProtocol) = kind
                    && let Some(modal) = app.config_modal.as_mut()
                {
                    modal.image_protocol_idx = if modal.image_protocol_idx == 0 {
                        IMAGE_PROTOCOLS.len() - 1
                    } else {
                        modal.image_protocol_idx - 1
                    };
                } else if let Some(FieldKind::SpinnerLightness) = kind
                    && let Some(modal) = app.config_modal.as_mut()
                {
                    modal.accent_lightness = modal.accent_lightness.saturating_sub(5).max(10);
                } else if let Some(modal) = app.config_modal.as_mut()
                    && modal.selected_field > 0
                {
                    modal.selected_field -= 1;
                }
            }
            KeyCode::Right => {
                let kind = app.config_modal.as_ref().map(|m| m.field_kind(selected));
                if let Some(FieldKind::SelectorProtocol) = kind
                    && let Some(modal) = app.config_modal.as_mut()
                {
                    modal.image_protocol_idx =
                        (modal.image_protocol_idx + 1) % IMAGE_PROTOCOLS.len();
                } else if let Some(FieldKind::SpinnerLightness) = kind
                    && let Some(modal) = app.config_modal.as_mut()
                {
                    modal.accent_lightness = (modal.accent_lightness + 5).min(90);
                } else if let Some(modal) = app.config_modal.as_mut() {
                    let max = modal.field_count() - 1;
                    if modal.selected_field < max {
                        modal.selected_field += 1;
                    }
                }
            }
            KeyCode::Char(' ') => {
                let kind = app.config_modal.as_ref().map(|m| m.field_kind(selected));
                if let Some(modal) = app.config_modal.as_mut() {
                    match kind {
                        Some(FieldKind::ToggleNerd) => modal.use_nerd_icons = !modal.use_nerd_icons,
                        Some(FieldKind::ToggleDiscover) => {
                            modal.auto_discover = !modal.auto_discover
                        }
                        Some(FieldKind::ToggleColors) => {
                            modal.disable_auto_colors = !modal.disable_auto_colors
                        }
                        Some(FieldKind::SpinnerLightness) => {
                            modal.accent_lightness = (modal.accent_lightness + 5).min(90);
                        }
                        Some(FieldKind::SelectorProtocol) => {
                            modal.image_protocol_idx =
                                (modal.image_protocol_idx + 1) % IMAGE_PROTOCOLS.len();
                        }
                        _ => {}
                    }
                }
            }
            KeyCode::Char('s') => {
                apply_config_save(app, cfg, client);
            }
            KeyCode::Esc | KeyCode::Char('c') | KeyCode::Char('q') => {
                app.config_modal = None;
            }
            _ => {}
        }
    }
}

pub async fn handle_main_select(app: &mut App, client: &Arc<LmsClient>, tx: &mpsc::Sender<AppMsg>) {
    match app.main_view.clone() {
        MainView::MyMusic => {
            // Map the selected row through the shared MyMusicEntry order (single source of truth
            // with ui::draw_my_music) rather than matching raw indices.
            let Some(&entry) = MyMusicEntry::ALL.get(app.main_selected) else {
                return;
            };
            match entry {
                MyMusicEntry::Artists => {
                    app.main_view = MainView::Library(LibraryView::Artists);
                    app.main_selected = 0;
                }
                MyMusicEntry::AlbumArtists => {
                    app.main_view = MainView::Library(LibraryView::AlbumArtists);
                    app.main_selected = 0;
                }
                MyMusicEntry::RecentlyPlayedArtists => {
                    app.is_loading = true;
                    background::load_recent_artists(50, client.clone(), tx.clone());
                    app.main_view = MainView::Library(LibraryView::RecentlyPlayedArtists);
                    app.main_selected = 0;
                }
                MyMusicEntry::Albums => {
                    app.is_loading = true;
                    background::load_albums(None, client.clone(), tx.clone());
                    app.main_view = MainView::Library(LibraryView::Albums { artist_id: None });
                    app.main_selected = 0;
                }
                MyMusicEntry::PopularAlbums => {
                    app.is_loading = true;
                    background::load_popular_albums(50, client.clone(), tx.clone());
                    app.main_view = MainView::Library(LibraryView::PopularAlbums);
                    app.main_selected = 0;
                }
                MyMusicEntry::Tracks => {
                    app.is_loading = true;
                    background::load_all_tracks(client.clone(), tx.clone());
                    app.main_view = MainView::Library(LibraryView::Tracks { album_id: None });
                    app.main_selected = 0;
                }
                MyMusicEntry::Playlists => {
                    app.is_loading = true;
                    background::load_playlists(client.clone(), tx.clone());
                    app.main_view = MainView::Library(LibraryView::Playlists);
                    app.main_selected = 0;
                }
                MyMusicEntry::Folders => {
                    app.folder_items = vec![];
                    app.folder_nav_stack = vec![];
                    app.folder_title = "Folders".to_string();
                    app.main_view = MainView::Library(LibraryView::Folder { folder_id: None });
                    app.main_selected = 0;
                    app.is_loading = true;
                    background::load_folder_items(None, client.clone(), tx.clone());
                }
            }
        }
        // Both artist lists drill into the same Albums view; they differ only in source vec.
        MainView::Library(view @ (LibraryView::Artists | LibraryView::AlbumArtists)) => {
            let artist = if matches!(view, LibraryView::AlbumArtists) {
                app.album_artists.get(app.main_selected)
            } else {
                app.artists.get(app.main_selected)
            };
            if let Some(artist) = artist {
                let id = utils::json_id_to_string(&artist.id);
                app.is_loading = true;
                background::load_albums(Some(id.clone()), client.clone(), tx.clone());
                app.main_view = MainView::Library(LibraryView::Albums {
                    artist_id: Some(id),
                });
                app.main_selected = 0;
            }
        }
        MainView::Library(LibraryView::Albums { .. }) => {
            if let Some(album) = app.albums.get(app.main_selected) {
                let id = utils::json_id_to_string(&album.id);
                app.is_loading = true;
                background::load_tracks(id.clone(), client.clone(), tx.clone());
                app.main_view = MainView::Library(LibraryView::Tracks { album_id: Some(id) });
                app.main_selected = 0;
            }
        }
        MainView::Library(LibraryView::Folder { folder_id }) => {
            if let Some(item) = app.folder_items.get(app.main_selected).cloned() {
                match item.item_type {
                    FolderItemType::Folder => {
                        let nav = FolderNav {
                            folder_id,
                            title: app.folder_title.clone(),
                            items: std::mem::take(&mut app.folder_items),
                            selected: app.main_selected,
                        };
                        app.folder_nav_stack.push(nav);
                        app.folder_title = item.filename.clone();
                        app.main_view = MainView::Library(LibraryView::Folder {
                            folder_id: Some(item.id),
                        });
                        app.main_selected = 0;
                        app.is_loading = true;
                        background::load_folder_items(Some(item.id), client.clone(), tx.clone());
                    }
                    FolderItemType::Track => {
                        if let Some(pid) = app.active_pid() {
                            let track_id = item.id.to_string();
                            spawn_fire(client, move |c| async move {
                                c.play_track(&pid, &track_id).await
                            });
                        }
                    }
                }
            }
        }
        MainView::Library(LibraryView::Playlists) => {
            if let Some(pid) = app.active_pid()
                && let Some(playlist) = app.playlists.get(app.main_selected)
            {
                let id = utils::json_id_to_string(&playlist.id);
                let name = playlist.name.clone();
                spawn_status(
                    client,
                    tx,
                    format!("Playing \"{}\"", name),
                    move |c| async move { c.play_playlist(&pid, &id).await },
                );
            }
        }
        MainView::Library(LibraryView::RecentlyPlayedArtists) => {
            if let Some(artist) = app.recent_artists.get(app.main_selected) {
                let id = utils::json_id_to_string(&artist.id);
                app.is_loading = true;
                app.previous_view =
                    Some(MainView::Library(LibraryView::RecentlyPlayedArtists));
                background::load_albums(Some(id.clone()), client.clone(), tx.clone());
                app.main_view = MainView::Library(LibraryView::Albums {
                    artist_id: Some(id),
                });
                app.main_selected = 0;
            }
        }
        MainView::Library(LibraryView::PopularAlbums) => {
            if let Some(album) = app.popular_albums.get(app.main_selected) {
                let id = utils::json_id_to_string(&album.id);
                app.is_loading = true;
                app.previous_view = Some(MainView::Library(LibraryView::PopularAlbums));
                background::load_tracks(id.clone(), client.clone(), tx.clone());
                app.main_view = MainView::Library(LibraryView::Tracks { album_id: Some(id) });
                app.main_selected = 0;
            }
        }
        MainView::Library(LibraryView::Tracks { album_id }) => {
            if let Some(pid) = app.active_pid() {
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
                            let track_id = utils::extract_id(track.id.as_ref());
                            tokio::spawn(async move {
                                let _ = c.play_track(&pid, &track_id).await;
                            });
                        }
                    }
                }
            }
        }
        MainView::Queue => {
            if let Some(pid) = app.active_pid() {
                let idx = app.main_selected;
                spawn_fire(client, move |c| async move {
                    c.play_track_index(&pid, idx).await
                });
            }
        }
        MainView::Players => {
            if app.players_focus_global {
                app.global_volume_control = !app.global_volume_control;
            } else if let Some(player) = app.players.get(app.main_selected) {
                let pid = player.playerid.clone();
                app.active_player = Some(pid.clone());
                if let Some(h) = app.now_playing_handle.take() {
                    h.abort();
                }
                app.now_playing_handle = Some(background::start_now_playing_loop(
                    pid,
                    client.clone(),
                    tx.clone(),
                ));
            }
        }
        MainView::Radio => {
            if let Some(item) = app.radio_items.get(app.main_selected).cloned() {
                if item.is_navigable()
                    && let Some(cmd) = item.cmd
                {
                    let item_id = item.item_id;
                    let pid = app.active_pid().unwrap_or_default();
                    push_nav(
                        &mut app.radio_nav_stack,
                        app.radio_title.clone(),
                        std::mem::take(&mut app.radio_items),
                        app.main_selected,
                    );
                    app.radio_title = item.name;
                    app.main_selected = 0;
                    app.is_loading = true;
                    background::load_radio_items(pid, cmd, item_id, client.clone(), tx.clone());
                } else if item.is_playable()
                    && let (Some(pid), Some(url)) = (app.active_pid(), item.url)
                {
                    let name = item.name.clone();
                    spawn_fire(client, move |c| async move {
                        c.play_url_with_title(&pid, &url, &name).await
                    });
                }
            }
        }
        MainView::Apps => {
            if let Some(item) = app.app_items.get(app.main_selected).cloned() {
                if item.item_type == "search"
                    && let Some(cmd) = item.cmd.clone()
                {
                    let item_id = item.item_id.clone();
                    app.main_view = MainView::AppSearch { cmd, item_id };
                    app.app_search_query = String::new();
                    app.app_search_cursor_pos = 0;
                    app.app_search_results = vec![];
                    app.app_search_input_active = true;
                    app.main_selected = 0;
                    app.focus_sidebar = false;
                } else if item.is_navigable()
                    && let Some(cmd) = item.cmd
                {
                    let item_id = item.item_id;
                    let pid = app.active_pid().unwrap_or_default();
                    push_nav(
                        &mut app.app_nav_stack,
                        app.app_title.clone(),
                        std::mem::take(&mut app.app_items),
                        app.main_selected,
                    );
                    app.app_title = item.name;
                    app.main_selected = 0;
                    app.is_loading = true;
                    background::load_app_items(pid, cmd, item_id, client.clone(), tx.clone());
                } else if item.is_playable()
                    && let (Some(pid), Some(url)) = (app.active_pid(), item.url)
                {
                    let name = item.name.clone();
                    spawn_fire(client, move |c| async move {
                        c.play_url_with_title(&pid, &url, &name).await
                    });
                }
            }
        }
        MainView::AppSearch { .. } => {
            if let Some(item) = app.app_search_results.get(app.main_selected).cloned() {
                if item.is_navigable()
                    && let Some(item_cmd) = item.cmd.clone()
                    && item.item_id.is_some()
                {
                    let pid = app.active_pid().unwrap_or_default();
                    push_nav(
                        &mut app.app_nav_stack,
                        app.app_title.clone(),
                        std::mem::take(&mut app.app_items),
                        0,
                    );
                    app.app_title = item.name.clone();
                    app.main_selected = 0;
                    app.is_loading = true;
                    app.main_view = MainView::Apps;
                    background::load_app_items(
                        pid,
                        item_cmd,
                        item.item_id,
                        client.clone(),
                        tx.clone(),
                    );
                } else if item.is_playable()
                    && let (Some(pid), Some(url)) = (app.active_pid(), item.url)
                {
                    let name = item.name.clone();
                    spawn_fire(client, move |c| async move {
                        c.play_url_with_title(&pid, &url, &name).await
                    });
                }
            } else {
                // No item selected, activate input box
                app.app_search_input_active = true;
            }
        }
        MainView::Favourites => {
            if let Some(item) = app.fav_items.get(app.main_selected).cloned() {
                if item.is_navigable()
                    && let Some(item_id) = item.item_id.clone()
                {
                    let pid = app.active_pid().unwrap_or_default();
                    push_nav(
                        &mut app.fav_nav_stack,
                        app.fav_title.clone(),
                        std::mem::take(&mut app.fav_items),
                        app.main_selected,
                    );
                    app.fav_title = item.name;
                    app.main_selected = 0;
                    app.is_loading = true;
                    background::load_fav_items(pid, Some(item_id), client.clone(), tx.clone());
                } else if item.is_playable()
                    && let (Some(pid), Some(url)) = (app.active_pid(), item.url)
                {
                    let name = item.name.clone();
                    spawn_fire(client, move |c| async move {
                        c.play_url_with_title(&pid, &url, &name).await
                    });
                }
            }
        }
        MainView::Help => {}
        MainView::Search => {
            let Some(item) = app.search_results.get(app.main_selected).cloned() else {
                return;
            };
            match item {
                SearchResultItem::Artist(a) => {
                    let id = utils::json_id_to_string(&a.id);
                    background::load_albums(Some(id.clone()), client.clone(), tx.clone());
                    app.previous_view = Some(MainView::Search);
                    app.main_view = MainView::Library(LibraryView::Albums {
                        artist_id: Some(id),
                    });
                    app.main_selected = 0;
                }
                SearchResultItem::Album(alb) => {
                    let id = utils::json_id_to_string(&alb.id);
                    background::load_tracks(id.clone(), client.clone(), tx.clone());
                    app.previous_view = Some(MainView::Search);
                    app.main_view = MainView::Library(LibraryView::Tracks { album_id: Some(id) });
                    app.main_selected = 0;
                }
                SearchResultItem::Track(t) => {
                    if let Some(pid) = app.active_pid() {
                        let track_id = utils::extract_id(t.id.as_ref());
                        spawn_fire(client, move |c| async move {
                            c.play_track(&pid, &track_id).await
                        });
                    }
                }
                SearchResultItem::Playlist(pl) => {
                    if let Some(pid) = app.active_pid() {
                        let playlist_id = utils::json_id_to_string(&pl.id);
                        spawn_fire(client, move |c| async move {
                            c.play_playlist(&pid, &playlist_id).await
                        });
                    }
                }
                SearchResultItem::AppItem(item) => {
                    if let Some(cmd) = item.cmd {
                        let item_id = item.item_id.clone();
                        let pid = app.active_pid().unwrap_or_default();
                        app.app_items = vec![];
                        app.app_nav_stack = vec![];
                        app.app_title = item.name.clone();
                        app.previous_view = Some(MainView::Search);
                        app.main_view = MainView::Apps;
                        app.main_selected = 0;
                        background::load_app_items(pid, cmd, item_id, client.clone(), tx.clone());
                    }
                }
                SearchResultItem::RadioItem(item) => {
                    if item.is_navigable()
                        && let Some(cmd) = item.cmd
                    {
                        let item_id = item.item_id.clone();
                        let pid = app.active_pid().unwrap_or_default();
                        app.radio_items = vec![];
                        app.radio_nav_stack = vec![];
                        app.radio_title = item.name.clone();
                        app.previous_view = Some(MainView::Search);
                        app.main_view = MainView::Radio;
                        app.main_selected = 0;
                        app.is_loading = true;
                        background::load_radio_items(pid, cmd, item_id, client.clone(), tx.clone());
                    } else if item.is_playable()
                        && let (Some(pid), Some(url)) = (app.active_pid(), item.url)
                    {
                        let name = item.name.clone();
                        spawn_fire(client, move |c| async move {
                            c.play_url_with_title(&pid, &url, &name).await
                        });
                    }
                }
            }
        }
    }
}

fn collect_url_items(items: &[crate::api::RadioItem]) -> Vec<(String, String)> {
    items
        .iter()
        .filter_map(|i| i.url.clone().map(|u| (i.name.clone(), u)))
        .collect()
}

fn spawn_add_url_folder(
    pid: String,
    items: Vec<(String, String)>,
    title: String,
    queue_was_empty: bool,
    client: Arc<LmsClient>,
    tx: mpsc::Sender<AppMsg>,
) {
    tokio::spawn(async move {
        let mut added = 0usize;
        for (name, url) in &items {
            if client
                .add_url_with_title_to_queue(&pid, url, name)
                .await
                .is_ok()
            {
                added += 1;
            }
        }
        if added > 0 {
            if queue_was_empty {
                let _ = client.play(&pid).await;
            }
            let _ = tx
                .send(AppMsg::StatusMsg(format!(
                    "Added {} items from \"{}\" to queue",
                    added, title
                )))
                .await;
        }
    });
}

fn spawn_replace_with_url_folder(
    pid: String,
    items: Vec<(String, String)>,
    title: String,
    client: Arc<LmsClient>,
    tx: mpsc::Sender<AppMsg>,
) {
    tokio::spawn(async move {
        if client.clear_queue(&pid).await.is_ok() {
            let mut added = 0usize;
            for (name, url) in &items {
                if client
                    .add_url_with_title_to_queue(&pid, url, name)
                    .await
                    .is_ok()
                {
                    added += 1;
                }
            }
            if added > 0 {
                let _ = client.play(&pid).await;
                let _ = tx
                    .send(AppMsg::StatusMsg(format!("Playing \"{}\" folder", title)))
                    .await;
            }
        }
    });
}

async fn handle_add_parent_to_queue(
    app: &mut App,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    let Some(pid) = app.active_pid() else {
        app.status_message = Some("No active player".to_string());
        return;
    };
    let queue_was_empty = app.queue.is_empty();
    let Some(target) = current_parent_target(app) else {
        return;
    };

    match target {
        ParentTarget::Album { id, name } => spawn_add_to_queue(
            client,
            tx,
            pid,
            queue_was_empty,
            format!("Added \"{}\" to queue", name),
            move |c, p| async move { c.add_album_to_queue(&p, &id).await },
        ),
        ParentTarget::Folder { id, name } => spawn_add_to_queue(
            client,
            tx,
            pid,
            queue_was_empty,
            format!("Added \"{}\" to queue", name),
            move |c, p| async move { c.add_folder_to_queue(&p, id).await },
        ),
        ParentTarget::UrlFolder { items, title } => {
            spawn_add_url_folder(pid, items, title, queue_was_empty, client.clone(), tx.clone())
        }
    }
}

fn spawn_add_url_to_queue(
    pid: String,
    url: String,
    name: String,
    queue_was_empty: bool,
    client: Arc<LmsClient>,
    tx: mpsc::Sender<AppMsg>,
) {
    tokio::spawn(async move {
        if client
            .add_url_with_title_to_queue(&pid, &url, &name)
            .await
            .is_ok()
        {
            if queue_was_empty {
                let _ = client.play(&pid).await;
            }
            let _ = tx
                .send(AppMsg::StatusMsg(format!("Added \"{}\" to queue", name)))
                .await;
        }
    });
}

fn spawn_replace_with_url(
    pid: String,
    url: String,
    name: String,
    client: Arc<LmsClient>,
    tx: mpsc::Sender<AppMsg>,
) {
    tokio::spawn(async move {
        if client.clear_queue(&pid).await.is_ok()
            && client
                .add_url_with_title_to_queue(&pid, &url, &name)
                .await
                .is_ok()
        {
            let _ = client.play(&pid).await;
            let _ = tx
                .send(AppMsg::StatusMsg(format!("Playing \"{}\"", name)))
                .await;
        }
    });
}

/// The currently-selected item resolved to a queueable target, independent of whether
/// the caller wants to append it (`handle_add_to_queue`) or play it now
/// (`handle_replace_queue`). Centralises the per-view id/name extraction those two share.
enum QueueTarget {
    Artist { id: String, name: String },
    Album { id: String, name: String },
    Track { id: String, name: String },
    Playlist { id: String, name: String },
    FolderTrack { id: String, name: String },
    FolderFolder { id: u32, name: String },
    Url { url: String, name: String },
}

/// The "parent" container of the current view (the album/folder/url-list being browsed),
/// shared by `handle_add_parent_to_queue` and `handle_replace_queue_with_parent`.
enum ParentTarget {
    Album { id: String, name: String },
    Folder { id: u32, name: String },
    UrlFolder { items: Vec<(String, String)>, title: String },
}

fn current_queue_target(app: &App) -> Option<QueueTarget> {
    let url_target = |item: Option<&crate::api::RadioItem>| {
        let item = item?;
        Some(QueueTarget::Url {
            url: item.url.clone()?,
            name: item.name.clone(),
        })
    };
    match app.main_view.clone() {
        MainView::Library(LibraryView::Artists | LibraryView::AlbumArtists) => {
            let list = if matches!(app.main_view, MainView::Library(LibraryView::Artists)) {
                &app.artists
            } else {
                &app.album_artists
            };
            let artist = list.get(app.main_selected)?;
            Some(QueueTarget::Artist {
                id: utils::json_id_to_string(&artist.id),
                name: artist.artist.clone(),
            })
        }
        MainView::Library(LibraryView::Albums { .. }) => {
            let album = app.albums.get(app.main_selected)?;
            Some(QueueTarget::Album {
                id: utils::json_id_to_string(&album.id),
                name: album.album.clone(),
            })
        }
        MainView::Library(LibraryView::Tracks { .. }) => {
            let track = app.tracks.get(app.main_selected)?;
            Some(QueueTarget::Track {
                id: utils::extract_id(track.id.as_ref()),
                name: track.title.clone(),
            })
        }
        MainView::Library(LibraryView::Playlists) => {
            let playlist = app.playlists.get(app.main_selected)?;
            Some(QueueTarget::Playlist {
                id: utils::json_id_to_string(&playlist.id),
                name: playlist.name.clone(),
            })
        }
        MainView::Library(LibraryView::Folder { .. }) => {
            let item = app.folder_items.get(app.main_selected)?;
            let name = item.filename.clone();
            match item.item_type {
                FolderItemType::Track => Some(QueueTarget::FolderTrack {
                    id: item.id.to_string(),
                    name,
                }),
                FolderItemType::Folder => Some(QueueTarget::FolderFolder { id: item.id, name }),
            }
        }
        MainView::Radio => url_target(app.radio_items.get(app.main_selected)),
        MainView::Apps => url_target(app.app_items.get(app.main_selected)),
        MainView::AppSearch { .. } => url_target(app.app_search_results.get(app.main_selected)),
        MainView::Favourites => url_target(app.fav_items.get(app.main_selected)),
        MainView::Search => match app.search_results.get(app.main_selected)? {
            SearchResultItem::Track(track) => Some(QueueTarget::Track {
                id: utils::extract_id(track.id.as_ref()),
                name: track.title.clone(),
            }),
            SearchResultItem::Album(alb) => Some(QueueTarget::Album {
                id: utils::json_id_to_string(&alb.id),
                name: alb.album.clone(),
            }),
            SearchResultItem::Artist(artist) => Some(QueueTarget::Artist {
                id: utils::json_id_to_string(&artist.id),
                name: artist.artist.clone(),
            }),
            SearchResultItem::Playlist(pl) => Some(QueueTarget::Playlist {
                id: utils::json_id_to_string(&pl.id),
                name: pl.name.clone(),
            }),
            _ => None,
        },
        _ => None,
    }
}

fn current_parent_target(app: &App) -> Option<ParentTarget> {
    match app.main_view.clone() {
        MainView::Library(LibraryView::Tracks { album_id: Some(id) }) => {
            let name = app
                .albums
                .iter()
                .find(|a| utils::json_id_to_string(&a.id) == id)
                .map(|a| a.album.clone())
                .unwrap_or_else(|| "album".to_string());
            Some(ParentTarget::Album { id, name })
        }
        MainView::Radio => Some(ParentTarget::UrlFolder {
            items: collect_url_items(&app.radio_items),
            title: app.radio_title.clone(),
        }),
        MainView::Apps => Some(ParentTarget::UrlFolder {
            items: collect_url_items(&app.app_items),
            title: app.app_title.clone(),
        }),
        MainView::Favourites => Some(ParentTarget::UrlFolder {
            items: collect_url_items(&app.fav_items),
            title: app.fav_title.clone(),
        }),
        MainView::Library(LibraryView::Folder {
            folder_id: Some(folder_id),
        }) => Some(ParentTarget::Folder {
            id: folder_id,
            name: app.folder_title.clone(),
        }),
        _ => None,
    }
}

/// Append-to-queue variant of `spawn_status`: runs `op` (which receives the client and
/// player id), and if it succeeds while the queue was previously empty, starts playback.
fn spawn_add_to_queue<F, Fut>(
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
    pid: String,
    queue_was_empty: bool,
    ok_msg: String,
    op: F,
) where
    F: FnOnce(Arc<LmsClient>, String) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
{
    spawn_status(client, tx, ok_msg, move |c| async move {
        let res = op(c.clone(), pid.clone()).await;
        if res.is_ok() && queue_was_empty {
            let _ = c.play(&pid).await;
        }
        res
    });
}

async fn handle_add_to_queue(app: &mut App, client: &Arc<LmsClient>, tx: &mpsc::Sender<AppMsg>) {
    let Some(pid) = app.active_pid() else {
        app.status_message = Some("No active player".to_string());
        return;
    };
    let queue_was_empty = app.queue.is_empty();
    let Some(target) = current_queue_target(app) else {
        return;
    };

    match target {
        QueueTarget::Artist { id, name } => spawn_add_to_queue(
            client,
            tx,
            pid,
            queue_was_empty,
            format!("Added \"{}\" to queue", name),
            move |c, p| async move { c.add_artist_to_queue(&p, &id).await },
        ),
        QueueTarget::Album { id, name } => spawn_add_to_queue(
            client,
            tx,
            pid,
            queue_was_empty,
            format!("Added \"{}\" to queue", name),
            move |c, p| async move { c.add_album_to_queue(&p, &id).await },
        ),
        QueueTarget::Track { id, name } | QueueTarget::FolderTrack { id, name } => spawn_add_to_queue(
            client,
            tx,
            pid,
            queue_was_empty,
            format!("Added \"{}\" to queue", name),
            move |c, p| async move { c.add_track_to_queue(&p, &id).await },
        ),
        QueueTarget::Playlist { id, name } => spawn_add_to_queue(
            client,
            tx,
            pid,
            queue_was_empty,
            format!("Added \"{}\" to queue", name),
            move |c, p| async move { c.add_playlist_to_queue(&p, &id).await },
        ),
        QueueTarget::FolderFolder { id, name } => spawn_add_to_queue(
            client,
            tx,
            pid,
            queue_was_empty,
            format!("Added folder \"{}\" to queue", name),
            move |c, p| async move { c.add_folder_to_queue(&p, id).await },
        ),
        QueueTarget::Url { url, name } => {
            spawn_add_url_to_queue(pid, url, name, queue_was_empty, client.clone(), tx.clone())
        }
    }
}

async fn handle_replace_queue(app: &mut App, client: &Arc<LmsClient>, tx: &mpsc::Sender<AppMsg>) {
    let Some(pid) = app.active_pid() else {
        app.status_message = Some("No active player".to_string());
        return;
    };
    let Some(target) = current_queue_target(app) else {
        return;
    };

    match target {
        QueueTarget::Artist { id, name } => spawn_status(
            client,
            tx,
            format!("Playing \"{}\"", name),
            move |c| async move { c.play_artist(&pid, &id).await },
        ),
        QueueTarget::Album { id, name } => spawn_status(
            client,
            tx,
            format!("Playing \"{}\"", name),
            move |c| async move { c.play_album(&pid, &id).await },
        ),
        QueueTarget::Track { id, name } | QueueTarget::FolderTrack { id, name } => spawn_status(
            client,
            tx,
            format!("Playing \"{}\"", name),
            move |c| async move { c.play_track(&pid, &id).await },
        ),
        QueueTarget::Playlist { id, name } => spawn_status(
            client,
            tx,
            format!("Playing \"{}\"", name),
            move |c| async move { c.play_playlist(&pid, &id).await },
        ),
        QueueTarget::FolderFolder { id, name } => spawn_status(
            client,
            tx,
            format!("Playing folder \"{}\"", name),
            move |c| async move { c.play_folder(&pid, id).await },
        ),
        QueueTarget::Url { url, name } => {
            spawn_replace_with_url(pid, url, name, client.clone(), tx.clone())
        }
    }
}

async fn handle_replace_queue_with_parent(
    app: &mut App,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    let Some(pid) = app.active_pid() else {
        app.status_message = Some("No active player".to_string());
        return;
    };

    let Some(target) = current_parent_target(app) else {
        return;
    };

    match target {
        ParentTarget::Album { id, name } => spawn_status(
            client,
            tx,
            format!("Playing \"{}\"", name),
            move |c| async move { c.play_album(&pid, &id).await },
        ),
        ParentTarget::Folder { id, name } => spawn_status(
            client,
            tx,
            format!("Playing \"{}\"", name),
            move |c| async move { c.play_folder(&pid, id).await },
        ),
        ParentTarget::UrlFolder { items, title } => {
            spawn_replace_with_url_folder(pid, items, title, client.clone(), tx.clone())
        }
    }
}

fn char_byte_offset(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

fn input_insert(s: &mut String, cursor: &mut usize, c: char) {
    let byte = char_byte_offset(s, *cursor);
    s.insert(byte, c);
    *cursor += 1;
}

fn input_backspace(s: &mut String, cursor: &mut usize) {
    if *cursor > 0 {
        *cursor -= 1;
        let byte = char_byte_offset(s, *cursor);
        s.remove(byte);
    }
}

fn input_delete(s: &mut String, cursor: usize) {
    let char_len = s.chars().count();
    if cursor < char_len {
        let byte = char_byte_offset(s, cursor);
        s.remove(byte);
    }
}

/// Handle a caret/edit key (Char/Backspace/Delete/Left/Right/Home/End) against a text field.
/// Returns `true` if the key was consumed, `false` if the caller should handle it.
fn handle_text_edit_key(key: KeyCode, query: &mut String, cursor: &mut usize) -> bool {
    match key {
        KeyCode::Char(c) => input_insert(query, cursor, c),
        KeyCode::Backspace => input_backspace(query, cursor),
        KeyCode::Delete => input_delete(query, *cursor),
        KeyCode::Left => {
            if *cursor > 0 {
                *cursor -= 1;
            }
        }
        KeyCode::Right => {
            if *cursor < query.chars().count() {
                *cursor += 1;
            }
        }
        KeyCode::Home => *cursor = 0,
        KeyCode::End => *cursor = query.chars().count(),
        _ => return false,
    }
    true
}

/// Handle a key while the local panel filter (`/`) box is being edited. Filters live: any text
/// edit re-derives the filtered list immediately. Routed here from the event loop only when
/// `app.local_filter` is `Some` and `editing` is true.
pub fn handle_local_filter_key(app: &mut App, key: KeyEvent) {
    match key.code {
        // Enter or ↓ commits: keep the filter applied, move focus down to the (filtered) list.
        KeyCode::Enter | KeyCode::Down => {
            if let Some(f) = &mut app.local_filter {
                f.editing = false;
            }
            app.main_selected = 0;
        }
        KeyCode::Esc => filter::clear(app),
        // Backspace on an empty query closes the filter (mirrors typical TUI behavior).
        KeyCode::Backspace if app.local_filter.as_ref().is_some_and(|f| f.query.is_empty()) => {
            filter::clear(app);
        }
        _ => {
            let edited = if let Some(f) = &mut app.local_filter {
                handle_text_edit_key(key.code, &mut f.query, &mut f.cursor)
            } else {
                false
            };
            if edited {
                filter::recompute(app);
            }
        }
    }
}

pub async fn handle_search_input_key(
    app: &mut App,
    key: KeyEvent,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    if handle_text_edit_key(key.code, &mut app.search_query, &mut app.search_cursor_pos) {
        return;
    }
    match key.code {
        KeyCode::Enter => {
            if !app.search_query.is_empty() {
                app.search_results = vec![];
                app.main_selected = 0;
                let player_id = app.active_pid().unwrap_or_default();
                background::trigger_search(
                    app.search_query.clone(),
                    app.search_scope.clone(),
                    app.app_services.clone(),
                    app.radio_services.clone(),
                    player_id,
                    client.clone(),
                    tx.clone(),
                );
                app.search_input_active = false;
                app.search_cursor_pos = 0;
            }
        }
        KeyCode::Tab => {
            cycle_search_scope(true, app, client, tx);
        }
        KeyCode::BackTab => {
            cycle_search_scope(false, app, client, tx);
        }
        KeyCode::Esc | KeyCode::Down => {
            app.search_input_active = false;
        }
        _ => {}
    }
}

pub async fn handle_app_search_input_key(
    app: &mut App,
    key: KeyEvent,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    let MainView::AppSearch { cmd, item_id } = &app.main_view else {
        return;
    };
    let cmd = cmd.clone();
    let item_id = item_id.clone();
    if handle_text_edit_key(
        key.code,
        &mut app.app_search_query,
        &mut app.app_search_cursor_pos,
    ) {
        return;
    }
    match key.code {
        KeyCode::Enter => {
            if !app.app_search_query.is_empty() {
                app.app_search_results = vec![];
                app.main_selected = 0;
                app.is_loading = true;
                let player_id = app.active_pid().unwrap_or_default();
                background::trigger_app_specific_search(
                    app.app_search_query.clone(),
                    cmd,
                    item_id,
                    player_id,
                    client.clone(),
                    tx.clone(),
                );
                app.app_search_input_active = false;
                app.app_search_cursor_pos = 0;
            }
        }
        KeyCode::Esc | KeyCode::Down => {
            app.app_search_input_active = false;
        }
        _ => {}
    }
}
