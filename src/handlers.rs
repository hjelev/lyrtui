use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::widgets::ListState;

use crate::api::{FolderItemType, LmsClient};
use crate::app::{
    App, AppMsg, ConnectionState, ContextMenu, FolderNav, LibraryView, MainView,
    RadioNav, SearchResultItem, SidebarItem,
};
use crate::events::Action;
use crate::{background, config, ui, utils};

fn point_in(col: u16, row: u16, area: Rect) -> bool {
    col >= area.x && col < area.x + area.width && row >= area.y && row < area.y + area.height
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

    // Clear queue dialog intercepts all mouse events when open
    if app.confirm_clear_queue {
        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
            let (popup, [ok_rect, cancel_rect]) = ui::compute_clear_queue_button_rects(terminal_area);
            if point_in(col, row, ok_rect) {
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
            } else if point_in(col, row, cancel_rect) || !point_in(col, row, popup) {
                app.confirm_clear_queue = false;
            }
        }
        return;
    }

    // Config modal intercepts all mouse events when open
    if app.config_modal.is_some() {
        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
            let (modal_area, field_rects) = ui::compute_config_modal_rects(terminal_area);
            let (_, btn_rects) = ui::compute_config_modal_button_rects(terminal_area);
            if point_in(col, row, btn_rects[0]) {
                apply_config_save(app, cfg, client);
            } else if point_in(col, row, btn_rects[1]) {
                app.config_modal = None;
            } else if point_in(col, row, modal_area) {
                let modal = app.config_modal.as_mut().unwrap();
                if point_in(col, row, field_rects[0]) {
                    modal.selected_field = 0;
                    modal.editing = true;
                    modal.error = None;
                } else if point_in(col, row, field_rects[1]) {
                    modal.selected_field = 1;
                    modal.editing = true;
                    modal.error = None;
                } else if point_in(col, row, field_rects[2]) {
                    modal.selected_field = 2;
                    modal.editing = true;
                    modal.error = None;
                } else if point_in(col, row, field_rects[3]) {
                    modal.selected_field = 3;
                    modal.editing = true;
                    modal.error = None;
                } else if point_in(col, row, field_rects[4]) {
                    modal.selected_field = 4;
                    modal.use_nerd_icons = !modal.use_nerd_icons;
                } else if point_in(col, row, field_rects[5]) {
                    modal.selected_field = 5;
                    modal.auto_discover = !modal.auto_discover;
                } else if point_in(col, row, field_rects[6]) {
                    modal.selected_field = 6;
                    modal.editing = true;
                    modal.error = None;
                }
            } else {
                app.config_modal = None;
            }
        }
        return;
    }

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

    // Full art mode intercepts all mouse events
    if app.full_art_mode {
        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
            let exit_rect = ui::compute_full_art_footer_exit_rect(terminal_area);
            if point_in(col, row, exit_rect) {
                app.full_art_mode = false;
                return;
            }
            let ctrl_rects = ui::compute_full_art_control_rects(terminal_area);
            let ctrl_hit = ctrl_rects.iter().enumerate().find(|(_, r)| point_in(col, row, **r));
            if let Some((btn_idx, _)) = ctrl_hit {
                let action = match btn_idx {
                    0 => Action::Prev,
                    1 => Action::PlayPause,
                    2 => Action::Stop,
                    3 => Action::Next,
                    4 => Action::ToggleShuffle,
                    5 => Action::ToggleRepeat,
                    6 => Action::VolumeDown,
                    _ => Action::VolumeUp,
                };
                handle_action(app, action, client, tx, vol_sync_tx).await;
            }
        }
        return;
    }

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Right) => {
            handle_action(app, Action::Back, client, tx, vol_sync_tx).await;
        }
        MouseEventKind::Down(MouseButton::Left) => {
            let ctrl_rects =
                ui::compute_statusbar_control_rects(terminal_area, app.status_height, app.art_col_w);
            let ctrl_hit = ctrl_rects
                .iter()
                .enumerate()
                .find(|(_, r)| point_in(col, row, **r));
            if let Some((btn_idx, _)) = ctrl_hit {
                let action = match btn_idx {
                    0 => Action::Prev,
                    1 => Action::PlayPause,
                    2 => Action::Stop,
                    3 => Action::Next,
                    4 => Action::ToggleShuffle,
                    5 => Action::ToggleRepeat,
                    6 => Action::VolumeDown,
                    _ => Action::VolumeUp,
                };
                handle_action(app, action, client, tx, vol_sync_tx).await;
            } else if point_in(col, row, sidebar_area) {
                app.focus_sidebar = true;
                let inner_top = sidebar_area.y + 1;
                let inner_bot = sidebar_area.y + sidebar_area.height.saturating_sub(1);
                if row >= inner_top && row < inner_bot {
                    let rel = (row - inner_top) as usize;
                    let idx = sidebar_state.offset() + rel;
                    if idx < app.sidebar_items.len() {
                        app.sidebar_selected = idx;
                        handle_action(app, Action::Select, client, tx, vol_sync_tx).await;
                    }
                }
            } else if point_in(col, row, main_area) {
                app.focus_sidebar = false;
                let inner_top = main_area.y + 1;
                let inner_bot = main_area.y + main_area.height.saturating_sub(1);
                if row >= inner_top && row < inner_bot {
                    let rel = (row - inner_top) as usize;
                    let row_h = if utils::uses_two_row_layout(&app.main_view) {
                        2
                    } else {
                        1
                    };
                    let idx = main_state.offset() + rel / row_h;
                    if idx < utils::main_list_len(app) {
                        let is_double = last_main_click
                            .as_ref()
                            .map(|(t, i)| *i == idx && t.elapsed().as_millis() < 500)
                            .unwrap_or(false);
                        *last_main_click = Some((Instant::now(), idx));

                        app.main_selected = idx;

                        if is_double || !utils::is_main_item_playable(app) {
                            handle_action(app, Action::Select, client, tx, vol_sync_tx).await;
                        } else {
                            app.context_menu =
                                Some(ContextMenu::new(utils::compute_parent_label(app)));
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
        KeyCode::Enter => {
            let confirmed = app.clear_queue_selected_button == 0;
            app.confirm_clear_queue = false;
            app.clear_queue_selected_button = 0;
            if confirmed {
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
        }
        KeyCode::Esc => {
            app.confirm_clear_queue = false;
            app.clear_queue_selected_button = 0;
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
                let id = utils::json_id_to_string(
                    track.id.as_ref().unwrap_or(&serde_json::Value::Null),
                );
                let name = track.title.clone();
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    if c.insert_track_next(&pid, &id).await.is_ok() {
                        let _ = t
                            .send(AppMsg::StatusMsg(format!("\"{}\" will play next", name)))
                            .await;
                    }
                });
            }
        }
        MainView::Queue => {
            if let Some(track) = app.queue.get(app.main_selected) {
                let name = track.title.clone();
                if let Some(id_val) = &track.id {
                    let id = utils::json_id_to_string(id_val);
                    let c = client.clone();
                    let t = tx.clone();
                    tokio::spawn(async move {
                        if c.insert_track_next(&pid, &id).await.is_ok() {
                            let _ = t
                                .send(AppMsg::StatusMsg(format!("\"{}\" will play next", name)))
                                .await;
                        }
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
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    if c.insert_track_next(&pid, &id).await.is_ok() {
                        let _ = t
                            .send(AppMsg::StatusMsg(format!("\"{}\" will play next", name)))
                            .await;
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
                    if c.insert_url_next(&pid, &url).await.is_ok() {
                        let _ = t
                            .send(AppMsg::StatusMsg(format!("\"{}\" will play next", name)))
                            .await;
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
                        let _ = t
                            .send(AppMsg::StatusMsg(format!("\"{}\" will play next", name)))
                            .await;
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
                        let _ = t
                            .send(AppMsg::StatusMsg(format!("\"{}\" will play next", name)))
                            .await;
                    }
                });
            }
        }
        MainView::Search => {
            match app.search_results.get(app.main_selected).cloned() {
                Some(SearchResultItem::Track(track)) => {
                    let id = utils::json_id_to_string(
                        track.id.as_ref().unwrap_or(&serde_json::Value::Null),
                    );
                    let name = track.title.clone();
                    let c = client.clone();
                    let t = tx.clone();
                    tokio::spawn(async move {
                        if c.insert_track_next(&pid, &id).await.is_ok() {
                            let _ = t
                                .send(AppMsg::StatusMsg(format!("\"{}\" will play next", name)))
                                .await;
                        }
                    });
                }
                Some(SearchResultItem::Playlist(pl)) => {
                    let id = utils::json_id_to_string(&pl.id);
                    let name = pl.name.clone();
                    let c = client.clone();
                    let t = tx.clone();
                    tokio::spawn(async move {
                        if c.insert_playlist_next(&pid, &id).await.is_ok() {
                            let _ = t
                                .send(AppMsg::StatusMsg(format!("\"{}\" will play next", name)))
                                .await;
                        }
                    });
                }
                _ => {}
            }
        }
        _ => {}
    }
}

async fn handle_add_to_favorites(
    app: &mut App,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    let Some(pid) = app.active_player.clone() else {
        app.status_message = Some("No active player".to_string());
        return;
    };

    match app.main_view.clone() {
        MainView::Library(LibraryView::Tracks { .. }) => {
            if let Some(track) = app.tracks.get(app.main_selected) {
                let id = utils::json_id_to_string(
                    track.id.as_ref().unwrap_or(&serde_json::Value::Null),
                );
                let name = track.title.clone();
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    let url = format!("db:track.id={}", id);
                    if c.add_to_favorites(&pid, &url, &name).await.is_ok() {
                        let _ = t
                            .send(AppMsg::StatusMsg(format!(
                                "Added \"{}\" to favourites",
                                name
                            )))
                            .await;
                    } else {
                        let _ = t
                            .send(AppMsg::StatusMsg(
                                "Could not add to favourites".to_string(),
                            ))
                            .await;
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
                        let _ = t
                            .send(AppMsg::StatusMsg(format!(
                                "Added \"{}\" to favourites",
                                name
                            )))
                            .await;
                    } else {
                        let _ = t
                            .send(AppMsg::StatusMsg(
                                "Could not add to favourites".to_string(),
                            ))
                            .await;
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
                        let _ = t
                            .send(AppMsg::StatusMsg(format!(
                                "Added \"{}\" to favourites",
                                name
                            )))
                            .await;
                    } else {
                        let _ = t
                            .send(AppMsg::StatusMsg(
                                "Could not add to favourites".to_string(),
                            ))
                            .await;
                    }
                });
            }
        }
        MainView::Search => {
            if let Some(SearchResultItem::Track(track)) =
                app.search_results.get(app.main_selected).cloned()
            {
                let id = utils::json_id_to_string(
                    track.id.as_ref().unwrap_or(&serde_json::Value::Null),
                );
                let name = track.title.clone();
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    let url = format!("db:track.id={}", id);
                    if c.add_to_favorites(&pid, &url, &name).await.is_ok() {
                        let _ = t
                            .send(AppMsg::StatusMsg(format!(
                                "Added \"{}\" to favourites",
                                name
                            )))
                            .await;
                    } else {
                        let _ = t
                            .send(AppMsg::StatusMsg(
                                "Could not add to favourites".to_string(),
                            ))
                            .await;
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
            app.focus_sidebar = true;
            app.players_focus_global = false;
            app.search_input_active = false;
        }
        Action::FocusMain => {
            app.focus_sidebar = false;
            if matches!(app.main_view, MainView::Search) {
                app.search_input_active = true;
            }
        }

        Action::NavUp => {
            if app.focus_sidebar {
                app.sidebar_selected = app.sidebar_selected.saturating_sub(1);
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
                app.main_selected = app.main_selected.saturating_sub(10);
            }
        }

        Action::PageDown => {
            if !app.focus_sidebar {
                let max = utils::main_list_len(app).saturating_sub(1);
                app.main_selected = (app.main_selected + 10).min(max);
            }
        }

        Action::Home => {
            if !app.focus_sidebar {
                app.main_selected = 0;
            }
        }

        Action::End => {
            if !app.focus_sidebar {
                app.main_selected = utils::main_list_len(app).saturating_sub(1);
            }
        }

        Action::Select => {
            if app.focus_sidebar {
                app.main_selected = 0;
                app.players_focus_global = false;
                match app.sidebar_items.get(app.sidebar_selected).cloned() {
                    Some(SidebarItem::MyMusic) => {
                        app.main_view = MainView::MyMusic;
                        app.main_selected = 0;
                        app.focus_sidebar = false;
                    }
                    Some(SidebarItem::Radio) => {
                        app.radio_items = vec![];
                        app.radio_nav_stack = vec![];
                        app.radio_title = "Radio".to_string();
                        app.main_view = MainView::Radio;
                        app.focus_sidebar = false;
                        background::load_radio_services(client.clone(), tx.clone());
                    }
                    Some(SidebarItem::Apps) => {
                        app.app_items = vec![];
                        app.app_nav_stack = vec![];
                        app.app_title = "Apps".to_string();
                        app.main_view = MainView::Apps;
                        app.focus_sidebar = false;
                        background::load_app_services(client.clone(), tx.clone());
                    }
                    Some(SidebarItem::Favourites) => {
                        app.fav_items = vec![];
                        app.fav_nav_stack = vec![];
                        app.fav_title = "Favourites".to_string();
                        app.main_view = MainView::Favourites;
                        app.focus_sidebar = false;
                        background::load_fav_items(
                            app.active_player.clone().unwrap_or_default(),
                            None,
                            client.clone(),
                            tx.clone(),
                        );
                    }
                    Some(SidebarItem::Queue) => {
                        app.main_view = MainView::Queue;
                        app.focus_sidebar = false;
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
                    }
                    None => {}
                }
            } else if utils::is_main_item_playable(app) {
                app.context_menu = Some(ContextMenu::new(utils::compute_parent_label(app)));
            } else {
                handle_main_select(app, client, tx).await;
            }
        }

        Action::Back => {
            if !app.focus_sidebar {
                match &app.main_view.clone() {
                    MainView::Library(LibraryView::Tracks { album_id: Some(_) }) => {
                        app.main_view =
                            MainView::Library(LibraryView::Albums { artist_id: None });
                        app.main_selected = 0;
                    }
                    MainView::Library(LibraryView::Albums { artist_id: Some(_) }) => {
                        app.main_view = MainView::Library(LibraryView::Artists);
                        app.main_selected = 0;
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
                    | MainView::Library(LibraryView::Albums { artist_id: None })
                    | MainView::Library(LibraryView::Tracks { album_id: None }) => {
                        app.main_view = MainView::MyMusic;
                        app.main_selected = 0;
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
                    MainView::Search => {
                        app.focus_sidebar = true;
                        app.search_input_active = false;
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
                tokio::spawn(async move {
                    let _ = c.next(&pid).await;
                });
            }
        }

        Action::Stop => {
            if let Some(pid) = app.active_player.clone() {
                let c = client.clone();
                tokio::spawn(async move {
                    let _ = c.stop(&pid).await;
                });
            }
        }

        Action::Prev => {
            if let Some(pid) = app.active_player.clone() {
                let c = client.clone();
                tokio::spawn(async move {
                    let _ = c.prev(&pid).await;
                });
            }
        }

        Action::VolumeUp => {
            if app.global_volume_control {
                let pids: Vec<String> =
                    app.players.iter().map(|p| p.playerid.clone()).collect();
                for pid in &pids {
                    let new_vol =
                        (app.player_volumes.get(pid).copied().unwrap_or(50) + 5).min(100);
                    app.player_volumes.insert(pid.clone(), new_vol);
                    let _ = vol_sync_tx.try_send((pid.clone(), new_vol));
                }
                if let Some(active_pid) = app.active_player.clone()
                    && let Some(new_vol) = app.player_volumes.get(&active_pid).copied()
                    && let Some(np) = app.now_playing.as_mut()
                {
                    np.volume = new_vol;
                }
            } else if let MainView::Players = &app.main_view {
                if app.players_focus_global {
                    let pids: Vec<String> =
                        app.players.iter().map(|p| p.playerid.clone()).collect();
                    for pid in pids {
                        let new_vol =
                            (app.player_volumes.get(&pid).copied().unwrap_or(50) + 5).min(100);
                        app.player_volumes.insert(pid.clone(), new_vol);
                        let _ = vol_sync_tx.try_send((pid, new_vol));
                    }
                } else if let Some(player) = app.players.get(app.main_selected) {
                    let pid = player.playerid.clone();
                    let new_vol =
                        (app.player_volumes.get(&pid).copied().unwrap_or(50) + 5).min(100);
                    app.player_volumes.insert(pid.clone(), new_vol);
                    if app.active_player.as_deref() == Some(&pid)
                        && let Some(np) = app.now_playing.as_mut()
                    {
                        np.volume = new_vol;
                    }
                    let _ = vol_sync_tx.try_send((pid, new_vol));
                }
            } else if let Some(pid) = app.active_player.clone() {
                let new_vol =
                    (app.now_playing.as_ref().map(|n| n.volume).unwrap_or(50) + 5).min(100);
                if let Some(np) = app.now_playing.as_mut() {
                    np.volume = new_vol;
                }
                app.player_volumes.insert(pid.clone(), new_vol);
                let _ = vol_sync_tx.try_send((pid, new_vol));
            }
        }

        Action::VolumeDown => {
            if app.global_volume_control {
                let pids: Vec<String> =
                    app.players.iter().map(|p| p.playerid.clone()).collect();
                for pid in &pids {
                    let new_vol = app
                        .player_volumes
                        .get(pid)
                        .copied()
                        .unwrap_or(50)
                        .saturating_sub(5);
                    app.player_volumes.insert(pid.clone(), new_vol);
                    let _ = vol_sync_tx.try_send((pid.clone(), new_vol));
                }
                if let Some(active_pid) = app.active_player.clone()
                    && let Some(new_vol) = app.player_volumes.get(&active_pid).copied()
                    && let Some(np) = app.now_playing.as_mut()
                {
                    np.volume = new_vol;
                }
            } else if let MainView::Players = &app.main_view {
                if app.players_focus_global {
                    let pids: Vec<String> =
                        app.players.iter().map(|p| p.playerid.clone()).collect();
                    for pid in pids {
                        let new_vol = app
                            .player_volumes
                            .get(&pid)
                            .copied()
                            .unwrap_or(50)
                            .saturating_sub(5);
                        app.player_volumes.insert(pid.clone(), new_vol);
                        let _ = vol_sync_tx.try_send((pid, new_vol));
                    }
                } else if let Some(player) = app.players.get(app.main_selected) {
                    let pid = player.playerid.clone();
                    let new_vol = app
                        .player_volumes
                        .get(&pid)
                        .copied()
                        .unwrap_or(50)
                        .saturating_sub(5);
                    app.player_volumes.insert(pid.clone(), new_vol);
                    if app.active_player.as_deref() == Some(&pid)
                        && let Some(np) = app.now_playing.as_mut()
                    {
                        np.volume = new_vol;
                    }
                    let _ = vol_sync_tx.try_send((pid, new_vol));
                }
            } else if let Some(pid) = app.active_player.clone() {
                let new_vol = app
                    .now_playing
                    .as_ref()
                    .map(|n| n.volume)
                    .unwrap_or(50)
                    .saturating_sub(5);
                if let Some(np) = app.now_playing.as_mut() {
                    np.volume = new_vol;
                }
                app.player_volumes.insert(pid.clone(), new_vol);
                let _ = vol_sync_tx.try_send((pid, new_vol));
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
                tokio::spawn(async move {
                    let _ = c.set_power(&pid, turn_on).await;
                });
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
            if let (Some(pid), Some(np)) = (app.active_player.clone(), app.now_playing.as_ref()) {
                let new_val = if np.shuffle > 0 { 0u8 } else { 1 };
                let c = client.clone();
                tokio::spawn(async move {
                    let _ = c.set_shuffle(&pid, new_val).await;
                });
            }
        }

        Action::ToggleRepeat => {
            if let (Some(pid), Some(np)) = (app.active_player.clone(), app.now_playing.as_ref()) {
                let new_val = match np.repeat {
                    0 => 1u8, // off → repeat single track
                    1 => 2u8, // repeat single → repeat queue
                    2 => 3u8, // repeat queue → don't stop the music
                    _ => 0u8, // don't stop → off
                };
                let c = client.clone();
                tokio::spawn(async move {
                    let _ = c.set_repeat(&pid, new_val).await;
                });
            }
        }

        Action::ToggleFullArtMode => {
            app.full_art_mode = !app.full_art_mode;
        }

        Action::OpenConfig | Action::None => {}
    }
    false
}

fn apply_config_save(app: &mut App, cfg: &mut config::Config, client: &Arc<LmsClient>) {
    let (host, port_str, username, password, use_nerd_icons, auto_discover, broadcast_mask) = {
        let modal = app.config_modal.as_ref().unwrap();
        (
            modal.host.trim().to_string(),
            modal.port.trim().to_string(),
            modal.username.trim().to_string(),
            modal.password.clone(),
            modal.use_nerd_icons,
            modal.auto_discover,
            modal.broadcast_mask.trim().to_string(),
        )
    };
    if host.is_empty() {
        app.config_modal.as_mut().unwrap().error = Some("Host cannot be empty".to_string());
    } else if broadcast_mask.is_empty() {
        app.config_modal.as_mut().unwrap().error = Some("Broadcast mask cannot be empty".to_string());
    } else {
        match port_str.parse::<u16>() {
            Ok(port) if port > 0 => {
                cfg.host = host;
                cfg.port = port;
                cfg.use_nerd_icons = use_nerd_icons;
                cfg.auto_discover = auto_discover;
                cfg.broadcast_mask = broadcast_mask;
                cfg.username = if username.is_empty() { None } else { Some(username.clone()) };
                cfg.password = if password.is_empty() { None } else { Some(password.clone()) };
                match cfg.save() {
                    Ok(()) => {
                        client.update_base_url(cfg.base_url());
                        let creds = cfg.username.as_ref()
                            .zip(cfg.password.as_ref())
                            .map(|(u, p)| (u.clone(), p.clone()));
                        client.update_credentials(creds);
                        app.use_nerd_icons = use_nerd_icons;
                        app.config_modal = None;
                        app.connection = ConnectionState::Reconnecting;
                        app.players = vec![];
                        app.active_player = None;
                        app.now_playing = None;
                        app.status_message = Some("Reconnecting...".to_string());
                    }
                    Err(e) => {
                        app.config_modal.as_mut().unwrap().error = Some(format!("Save error: {e}"));
                    }
                }
            }
            _ => {
                app.config_modal.as_mut().unwrap().error = Some("Invalid port (1–65535)".to_string());
            }
        }
    }
}

pub fn handle_config_key(
    app: &mut App,
    key: KeyEvent,
    cfg: &mut config::Config,
    client: &Arc<LmsClient>,
) {
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
                match modal.selected_field {
                    0 => modal.host.push(c),
                    1 => { if c.is_ascii_digit() { modal.port.push(c); } }
                    2 => modal.username.push(c),
                    3 => modal.password.push(c),
                    6 => modal.broadcast_mask.push(c),
                    _ => {}
                }
            }
            KeyCode::Backspace => {
                let modal = app.config_modal.as_mut().unwrap();
                match modal.selected_field {
                    0 => { modal.host.pop(); }
                    1 => { modal.port.pop(); }
                    2 => { modal.username.pop(); }
                    3 => { modal.password.pop(); }
                    6 => { modal.broadcast_mask.pop(); }
                    _ => {}
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
                if modal.selected_field < 8 {
                    modal.selected_field += 1;
                }
            }
            KeyCode::Tab => {
                let modal = app.config_modal.as_mut().unwrap();
                modal.selected_field = (modal.selected_field + 1) % 9;
            }
            KeyCode::Enter | KeyCode::Char('i') => {
                let selected = app.config_modal.as_ref().unwrap().selected_field;
                match selected {
                    4 => { app.config_modal.as_mut().unwrap().use_nerd_icons ^= true; }
                    5 => { app.config_modal.as_mut().unwrap().auto_discover ^= true; }
                    7 => { apply_config_save(app, cfg, client); }
                    8 => { app.config_modal = None; }
                    _ => {
                        let modal = app.config_modal.as_mut().unwrap();
                        modal.editing = true;
                        modal.error = None;
                    }
                }
            }
            KeyCode::Char(' ') => {
                let modal = app.config_modal.as_mut().unwrap();
                match modal.selected_field {
                    4 => modal.use_nerd_icons = !modal.use_nerd_icons,
                    5 => modal.auto_discover = !modal.auto_discover,
                    _ => {}
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

pub async fn handle_main_select(
    app: &mut App,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    match app.main_view.clone() {
        MainView::MyMusic => match app.main_selected {
            0 => {
                app.main_view = MainView::Library(LibraryView::Artists);
                app.main_selected = 0;
            }
            1 => {
                background::load_albums(None, client.clone(), tx.clone());
                app.main_view = MainView::Library(LibraryView::Albums { artist_id: None });
                app.main_selected = 0;
            }
            2 => {
                background::load_all_tracks(client.clone(), tx.clone());
                app.main_view = MainView::Library(LibraryView::Tracks { album_id: None });
                app.main_selected = 0;
            }
            3 => {
                app.folder_items = vec![];
                app.folder_nav_stack = vec![];
                app.folder_title = "Folders".to_string();
                app.main_view = MainView::Library(LibraryView::Folder { folder_id: None });
                app.main_selected = 0;
                background::load_folder_items(None, client.clone(), tx.clone());
            }
            _ => {}
        },
        MainView::Library(LibraryView::Artists) => {
            if let Some(artist) = app.artists.get(app.main_selected) {
                let id = utils::json_id_to_string(&artist.id);
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
                background::load_tracks(id.clone(), client.clone(), tx.clone());
                app.main_view = MainView::Library(LibraryView::Tracks {
                    album_id: Some(id),
                });
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
                        background::load_folder_items(Some(item.id), client.clone(), tx.clone());
                    }
                    FolderItemType::Track => {
                        if let Some(pid) = app.active_player.clone() {
                            let track_id = item.id.to_string();
                            let c = client.clone();
                            tokio::spawn(async move {
                                let _ = c.play_track(&pid, &track_id).await;
                            });
                        }
                    }
                }
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
                            let track_id = utils::json_id_to_string(
                                track.id.as_ref().unwrap_or(&serde_json::Value::Null),
                            );
                            tokio::spawn(async move {
                                let _ = c.play_track(&pid, &track_id).await;
                            });
                        }
                    }
                }
            }
        }
        MainView::Queue => {
            if let Some(pid) = app.active_player.clone() {
                let idx = app.main_selected;
                let c = client.clone();
                tokio::spawn(async move {
                    let _ = c.play_track_index(&pid, idx).await;
                });
            }
        }
        MainView::Players => {
            if app.players_focus_global {
                app.global_volume_control = !app.global_volume_control;
            } else if let Some(player) = app.players.get(app.main_selected) {
                let pid = player.playerid.clone();
                app.active_player = Some(pid.clone());
                background::start_now_playing_loop(pid, client.clone(), tx.clone());
            }
        }
        MainView::Radio => {
            if let Some(item) = app.radio_items.get(app.main_selected).cloned() {
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
                    background::load_radio_items(pid, cmd, item_id, client.clone(), tx.clone());
                } else if item.is_playable()
                    && let (Some(pid), Some(url)) = (app.active_player.clone(), item.url)
                {
                    let c = client.clone();
                    tokio::spawn(async move {
                        let _ = c.play_url(&pid, &url).await;
                    });
                }
            }
        }
        MainView::Apps => {
            if let Some(item) = app.app_items.get(app.main_selected).cloned() {
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
                    background::load_app_items(pid, cmd, item_id, client.clone(), tx.clone());
                } else if item.is_playable()
                    && let (Some(pid), Some(url)) = (app.active_player.clone(), item.url)
                {
                    let c = client.clone();
                    tokio::spawn(async move {
                        let _ = c.play_url(&pid, &url).await;
                    });
                }
            }
        }
        MainView::Favourites => {
            if let Some(item) = app.fav_items.get(app.main_selected).cloned() {
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
                    background::load_fav_items(pid, Some(item_id), client.clone(), tx.clone());
                } else if item.is_playable()
                    && let (Some(pid), Some(url)) = (app.active_player.clone(), item.url)
                {
                    let c = client.clone();
                    tokio::spawn(async move {
                        let _ = c.play_url(&pid, &url).await;
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
                    app.main_view = MainView::Library(LibraryView::Albums {
                        artist_id: Some(id),
                    });
                    app.main_selected = 0;
                }
                SearchResultItem::Album(alb) => {
                    let id = utils::json_id_to_string(&alb.id);
                    background::load_tracks(id.clone(), client.clone(), tx.clone());
                    app.main_view = MainView::Library(LibraryView::Tracks {
                        album_id: Some(id),
                    });
                    app.main_selected = 0;
                }
                SearchResultItem::Track(t) => {
                    if let Some(pid) = app.active_player.clone() {
                        let track_id = utils::json_id_to_string(
                            t.id.as_ref().unwrap_or(&serde_json::Value::Null),
                        );
                        let c = client.clone();
                        tokio::spawn(async move {
                            let _ = c.play_track(&pid, &track_id).await;
                        });
                    }
                }
                SearchResultItem::Playlist(pl) => {
                    if let Some(pid) = app.active_player.clone() {
                        let playlist_id = utils::json_id_to_string(&pl.id);
                        let c = client.clone();
                        tokio::spawn(async move {
                            let _ = c.play_playlist(&pid, &playlist_id).await;
                        });
                    }
                }
                SearchResultItem::AppItem(item) => {
                    if let Some(cmd) = item.cmd {
                        let item_id = item.item_id.clone();
                        let pid = app.active_player.clone().unwrap_or_default();
                        app.app_items = vec![];
                        app.app_nav_stack = vec![];
                        app.app_title = item.name.clone();
                        app.main_view = MainView::Apps;
                        app.main_selected = 0;
                        background::load_app_items(pid, cmd, item_id, client.clone(), tx.clone());
                    }
                }
            }
        }
    }
}

async fn handle_add_parent_to_queue(
    app: &mut App,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    let Some(pid) = app.active_player.clone() else {
        app.status_message = Some("No active player".to_string());
        return;
    };

    match app.main_view.clone() {
        MainView::Library(LibraryView::Tracks { album_id: Some(id) }) => {
            let name = app
                .albums
                .iter()
                .find(|a| utils::json_id_to_string(&a.id) == id)
                .map(|a| a.album.clone())
                .unwrap_or_else(|| "album".to_string());
            let c = client.clone();
            let t = tx.clone();
            tokio::spawn(async move {
                if c.add_album_to_queue(&pid, &id).await.is_ok() {
                    let _ = t
                        .send(AppMsg::StatusMsg(format!("Added \"{}\" to queue", name)))
                        .await;
                }
            });
        }
        MainView::Radio => {
            let urls: Vec<String> = app
                .radio_items
                .iter()
                .filter_map(|i| i.url.clone())
                .collect();
            let title = app.radio_title.clone();
            let c = client.clone();
            let t = tx.clone();
            tokio::spawn(async move {
                let mut added = 0usize;
                for url in &urls {
                    if c.add_url_to_queue(&pid, url).await.is_ok() {
                        added += 1;
                    }
                }
                if added > 0 {
                    let _ = t
                        .send(AppMsg::StatusMsg(format!(
                            "Added {} items from \"{}\" to queue",
                            added, title
                        )))
                        .await;
                }
            });
        }
        MainView::Apps => {
            let urls: Vec<String> = app
                .app_items
                .iter()
                .filter_map(|i| i.url.clone())
                .collect();
            let title = app.app_title.clone();
            let c = client.clone();
            let t = tx.clone();
            tokio::spawn(async move {
                let mut added = 0usize;
                for url in &urls {
                    if c.add_url_to_queue(&pid, url).await.is_ok() {
                        added += 1;
                    }
                }
                if added > 0 {
                    let _ = t
                        .send(AppMsg::StatusMsg(format!(
                            "Added {} items from \"{}\" to queue",
                            added, title
                        )))
                        .await;
                }
            });
        }
        MainView::Favourites => {
            let urls: Vec<String> = app
                .fav_items
                .iter()
                .filter_map(|i| i.url.clone())
                .collect();
            let title = app.fav_title.clone();
            let c = client.clone();
            let t = tx.clone();
            tokio::spawn(async move {
                let mut added = 0usize;
                for url in &urls {
                    if c.add_url_to_queue(&pid, url).await.is_ok() {
                        added += 1;
                    }
                }
                if added > 0 {
                    let _ = t
                        .send(AppMsg::StatusMsg(format!(
                            "Added {} items from \"{}\" to queue",
                            added, title
                        )))
                        .await;
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
                let id = utils::json_id_to_string(&artist.id);
                let name = artist.artist.clone();
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    if c.add_artist_to_queue(&pid, &id).await.is_ok() {
                        let _ = t
                            .send(AppMsg::StatusMsg(format!("Added \"{}\" to queue", name)))
                            .await;
                    }
                });
            }
        }
        MainView::Library(LibraryView::Albums { .. }) => {
            if let Some(album) = app.albums.get(app.main_selected) {
                let id = utils::json_id_to_string(&album.id);
                let name = album.album.clone();
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    if c.add_album_to_queue(&pid, &id).await.is_ok() {
                        let _ = t
                            .send(AppMsg::StatusMsg(format!("Added \"{}\" to queue", name)))
                            .await;
                    }
                });
            }
        }
        MainView::Library(LibraryView::Tracks { .. }) => {
            if let Some(track) = app.tracks.get(app.main_selected) {
                let id = utils::json_id_to_string(
                    track.id.as_ref().unwrap_or(&serde_json::Value::Null),
                );
                let name = track.title.clone();
                let c = client.clone();
                let t = tx.clone();
                tokio::spawn(async move {
                    if c.add_track_to_queue(&pid, &id).await.is_ok() {
                        let _ = t
                            .send(AppMsg::StatusMsg(format!("Added \"{}\" to queue", name)))
                            .await;
                    }
                });
            }
        }
        MainView::Library(LibraryView::Folder { .. }) => {
            if let Some(item) = app.folder_items.get(app.main_selected).cloned() {
                let name = item.filename.clone();
                let c = client.clone();
                let t = tx.clone();
                match item.item_type {
                    FolderItemType::Track => {
                        let id = item.id.to_string();
                        tokio::spawn(async move {
                            if c.add_track_to_queue(&pid, &id).await.is_ok() {
                                let _ = t
                                    .send(AppMsg::StatusMsg(format!(
                                        "Added \"{}\" to queue",
                                        name
                                    )))
                                    .await;
                            }
                        });
                    }
                    FolderItemType::Folder => {
                        let folder_id = item.id;
                        tokio::spawn(async move {
                            if c.add_folder_to_queue(&pid, folder_id).await.is_ok() {
                                let _ = t
                                    .send(AppMsg::StatusMsg(format!(
                                        "Added folder \"{}\" to queue",
                                        name
                                    )))
                                    .await;
                            }
                        });
                    }
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
                    if c.add_url_to_queue(&pid, &url).await.is_ok() {
                        let _ = t
                            .send(AppMsg::StatusMsg(format!("Added \"{}\" to queue", name)))
                            .await;
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
                        let _ = t
                            .send(AppMsg::StatusMsg(format!("Added \"{}\" to queue", name)))
                            .await;
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
                        let _ = t
                            .send(AppMsg::StatusMsg(format!("Added \"{}\" to queue", name)))
                            .await;
                    }
                });
            }
        }
        MainView::Search => {
            match app.search_results.get(app.main_selected).cloned() {
                Some(SearchResultItem::Track(track)) => {
                    let id = utils::json_id_to_string(
                        track.id.as_ref().unwrap_or(&serde_json::Value::Null),
                    );
                    let name = track.title.clone();
                    let c = client.clone();
                    let t = tx.clone();
                    tokio::spawn(async move {
                        if c.add_track_to_queue(&pid, &id).await.is_ok() {
                            let _ = t
                                .send(AppMsg::StatusMsg(format!("Added \"{}\" to queue", name)))
                                .await;
                        }
                    });
                }
                Some(SearchResultItem::Album(alb)) => {
                    let id = utils::json_id_to_string(&alb.id);
                    let name = alb.album.clone();
                    let c = client.clone();
                    let t = tx.clone();
                    tokio::spawn(async move {
                        if c.add_album_to_queue(&pid, &id).await.is_ok() {
                            let _ = t
                                .send(AppMsg::StatusMsg(format!("Added \"{}\" to queue", name)))
                                .await;
                        }
                    });
                }
                Some(SearchResultItem::Artist(artist)) => {
                    let id = utils::json_id_to_string(&artist.id);
                    let name = artist.artist.clone();
                    let c = client.clone();
                    let t = tx.clone();
                    tokio::spawn(async move {
                        if c.add_artist_to_queue(&pid, &id).await.is_ok() {
                            let _ = t
                                .send(AppMsg::StatusMsg(format!("Added \"{}\" to queue", name)))
                                .await;
                        }
                    });
                }
                Some(SearchResultItem::Playlist(pl)) => {
                    let id = utils::json_id_to_string(&pl.id);
                    let name = pl.name.clone();
                    let c = client.clone();
                    let t = tx.clone();
                    tokio::spawn(async move {
                        if c.add_playlist_to_queue(&pid, &id).await.is_ok() {
                            let _ = t
                                .send(AppMsg::StatusMsg(format!("Added \"{}\" to queue", name)))
                                .await;
                        }
                    });
                }
                _ => {}
            }
        }
        _ => {}
    }
}

pub async fn handle_search_input_key(
    app: &mut App,
    key: KeyEvent,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    match key.code {
        KeyCode::Char(c) => {
            app.search_query.push(c);
        }
        KeyCode::Backspace => {
            app.search_query.pop();
        }
        KeyCode::Enter => {
            if !app.search_query.is_empty() {
                app.search_results = vec![];
                app.main_selected = 0;
                let app_services = app.app_services.clone();
                let player_id = app.active_player.clone().unwrap_or_default();
                background::trigger_search(
                    app.search_query.clone(),
                    app_services,
                    player_id,
                    client.clone(),
                    tx.clone(),
                );
                app.search_input_active = false;
            }
        }
        KeyCode::Esc | KeyCode::Down => {
            app.search_input_active = false;
        }
        _ => {}
    }
}
