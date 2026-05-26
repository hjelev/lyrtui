use crate::api::{FolderItemType, NowPlaying};
use crate::app::{App, ConfigModal, ConnectionState, LibraryView, MainView, SearchResultItem, SidebarItem, SyncModal};
use serde_json::Value;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph,
        Scrollbar, ScrollbarOrientation, ScrollbarState,
    },
    Frame,
};
use ratatui_image::{Resize, StatefulImage, protocol::StatefulProtocol};
use std::collections::HashMap;

const THUMB_W: u16 = 4; // image column width in cells
const THUMB_SEP: u16 = 1; // gap between image and text
pub const PLAYERS_PWR_BTN_W: u16 = 3; // width of the power button column in the Players screen
pub const PLAYERS_SYNC_BTN_W: u16 = 3; // width of the sync button column " ⇄ "

fn sidebar_nerd_icon(item: &SidebarItem) -> &'static str {
    match item {
        SidebarItem::MyMusic    => "\u{F001}",  // nf-fa-music
        SidebarItem::Search     => "\u{F002}",  // nf-fa-search
        SidebarItem::Radio      => "\u{F130}",  // nf-fa-microphone
        SidebarItem::Apps       => "\u{F009}",  // nf-fa-th-large
        SidebarItem::Favourites => "\u{F005}",  // nf-fa-star
        SidebarItem::Queue      => "\u{F03A}",  // nf-fa-list
        SidebarItem::Players    => "\u{F028}",  // nf-fa-volume-up
        SidebarItem::Help       => "\u{F059}",  // nf-fa-question-circle
    }
}

fn focus_border_color(accent: Option<[u8; 3]>) -> Color {
    match accent {
        Some([r, g, b]) => Color::Rgb(r, g, b),
        None => Color::Yellow,
    }
}

fn accent_tint(accent: Option<[u8; 3]>, pct: u16, r_off: u16, g_off: u16, b_off: u16, fallback: Color) -> Color {
    match accent {
        Some([r, g, b]) => Color::Rgb(
            (r as u16 * pct / 100 + r_off).min(255) as u8,
            (g as u16 * pct / 100 + g_off).min(255) as u8,
            (b as u16 * pct / 100 + b_off).min(255) as u8,
        ),
        None => fallback,
    }
}

fn unfocus_border_color(accent: Option<[u8; 3]>) -> Color {
    accent_tint(accent, 25, 20, 20, 30, Color::DarkGray)
}

/// Very dark accent tint used as the background for media control buttons.
fn btn_bg_color(accent: Option<[u8; 3]>) -> Color {
    accent_tint(accent, 12, 15, 15, 20, Color::Rgb(28, 32, 45))
}

/// Slightly brighter dark accent tint for active-state toggle buttons (shuffle on, repeat on).
fn btn_active_bg_color(accent: Option<[u8; 3]>) -> Color {
    accent_tint(accent, 22, 15, 15, 20, Color::Rgb(20, 45, 60))
}

/// Dimmed foreground for inactive toggle buttons (shuffle off, repeat off).
fn btn_dim_color(accent: Option<[u8; 3]>) -> Color {
    accent_tint(accent, 30, 15, 15, 20, Color::Rgb(80, 80, 100))
}

/// Mid-brightness color from the accent palette — between the bright focus color and the dark
/// unfocus color. Used for secondary labels that should feel tinted but not dominant.
fn mid_accent_color(accent: Option<[u8; 3]>) -> Color {
    accent_tint(accent, 58, 18, 18, 25, Color::Gray)
}

fn sync_scroll_offset(state: &mut ListState, selected: usize, visible: usize) -> usize {
    let o = *state.offset_mut();
    let new_o = if selected < o {
        selected
    } else if visible > 0 && selected >= o + visible {
        selected + 1 - visible
    } else {
        o
    };
    *state.offset_mut() = new_o;
    new_o
}

fn pill_endcap_left(bg: Color, nerd: bool) -> Span<'static> {
    if nerd {
        Span::styled("\u{e0b6}", Style::default().fg(bg).bg(Color::Reset))
    } else {
        Span::raw(" ")
    }
}

fn pill_endcap_right(bg: Color, nerd: bool) -> Span<'static> {
    if nerd {
        Span::styled("\u{e0b4}", Style::default().fg(bg).bg(Color::Reset))
    } else {
        Span::raw("")
    }
}

fn icon_vol(nerd: bool) -> &'static str {
    if nerd { "\u{F028}" } else { "♪" }
}

fn icon_power(nerd: bool) -> &'static str {
    if nerd { "\u{F011}" } else { "⏻" }  // nf-fa-power-off
}

fn icon_player_dot(nerd: bool) -> &'static str {
    if nerd { "\u{f075a}" } else { "▶" }
}

fn icon_globe(nerd: bool) -> &'static str {
    if nerd { " \u{F0AC}" } else { " ◎" }
}

/// Returns (pill_bg, pill_fg) for the yazi-style pill selector.
fn pill_colors(focused: bool) -> (Color, Color) {
    if focused {
        (Color::Rgb(45, 100, 170), Color::Rgb(220, 235, 255))
    } else {
        (Color::Rgb(50, 50, 68), Color::Rgb(190, 190, 210))
    }
}

/// Pill cursor styles: returns (primary_line_style, secondary_line_style).
/// Focused uses a solid accent color; unfocused uses a dimmed variant.
fn cursor_styles(focused: bool) -> (Style, Style) {
    let (bg, fg) = pill_colors(focused);
    if focused {
        (
            Style::default().bg(bg).fg(fg).add_modifier(Modifier::BOLD),
            Style::default().bg(bg).fg(Color::Rgb(160, 195, 230)),
        )
    } else {
        (
            Style::default().bg(bg).fg(fg),
            Style::default().bg(bg).fg(Color::Rgb(140, 140, 160)),
        )
    }
}

struct RowItem {
    thumb_url: Option<String>,
    line1: Line<'static>,
    line2: Line<'static>,
}

/// Returns (sidebar_area, main_area) for a given terminal area.
pub fn compute_areas(area: Rect, status_height: u16) -> (Rect, Rect) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(status_height), Constraint::Length(1)])
        .split(area);
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(20), Constraint::Min(1)])
        .split(outer[0]);
    (panes[0], panes[1])
}

/// Returns the six clickable button rects in the Now Playing controls row: [Prev, PlayPause, Stop, Next, Shuffle, Repeat].
pub fn compute_statusbar_control_rects(area: Rect, status_height: u16, art_col_w: u16) -> [Rect; 8] {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(status_height), Constraint::Length(1)])
        .split(area);
    let status_inner = Rect::new(
        outer[1].x + 1,
        outer[1].y + 1,
        outer[1].width.saturating_sub(2),
        outer[1].height.saturating_sub(2),
    );
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(art_col_w), Constraint::Length(1), Constraint::Min(1)])
        .split(status_inner);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title
            Constraint::Length(1), // artist
            Constraint::Length(1), // album
            Constraint::Min(0),    // flexible spacer
            Constraint::Length(1), // controls
            Constraint::Length(1), // empty line
            Constraint::Length(1), // progress
        ])
        .split(cols[2]);
    let ctrl = rows[4];
    let btn_w: u16 = 3;
    let gap: u16 = 1;
    let sep: u16 = 2;
    std::array::from_fn(|i| {
        let x = if i < 4 {
            ctrl.x + (i as u16) * (btn_w + gap)
        } else if i < 6 {
            ctrl.x + 4 * (btn_w + gap) + sep + ((i - 4) as u16) * (btn_w + gap)
        } else {
            // volume down (6) and volume up (7) after a second sep gap
            ctrl.x + 4 * (btn_w + gap) + sep + 2 * (btn_w + gap) + sep + ((i - 6) as u16) * (btn_w + gap)
        };
        Rect::new(x, ctrl.y, btn_w, 1)
    })
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    f: &mut Frame,
    app: &App,
    album_art: Option<&mut StatefulProtocol>,
    sidebar_state: &mut ListState,
    main_state: &mut ListState,
    thumbnails: &mut HashMap<String, StatefulProtocol>,
    server_host: &str,
    server_port: u16,
) {
    let area = f.area();

    if app.full_art_mode {
        draw_full_art_mode(f, app, area, album_art, main_state, thumbnails);
    } else {
        // Outer layout: main content | status bar | notification line
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(app.status_height), Constraint::Length(1)])
            .split(area);

        let main_area = outer[0];
        let status_area = outer[1];
        let notif_area = outer[2];

        // Split main into sidebar | content
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(20), Constraint::Min(1)])
            .split(main_area);

        let base = format!("http://{}:{}", server_host, server_port);
        draw_sidebar(f, app, panes[0], sidebar_state);
        draw_main(f, app, panes[1], main_state, thumbnails, &base);
        draw_statusbar(f, app, status_area, album_art);

        if let Some(msg) = &app.status_message {
            let p = Paragraph::new(msg.as_str())
                .style(Style::default().fg(Color::Green));
            f.render_widget(p, notif_area);
        } else {
            let footer = if matches!(app.main_view, MainView::Players) {
                hint_line(&[
                    ("t", "power"), ("s", "sync"), ("Spc", "play/pause"), ("n/p", "next/prev"),
                    ("+/-", "vol"), ("`", "art mode"), ("c", "config"), ("q", "quit"),
                ], app.effective_accent())
            } else if matches!(app.main_view, MainView::Search) {
                if app.search_input_active {
                    hint_line(&[("Type", "query"), ("Enter", "search"), ("Esc/↓", "results"), ("q", "quit")], app.effective_accent())
                } else {
                    hint_line(&[("j/k", "navigate"), ("Enter", "select"), ("i//", "edit query"), ("Esc", "back"), ("q", "quit")], app.effective_accent())
                }
            } else {
                hint_line(&[
                    ("a", "add to queue"), ("Spc", "play/pause"), ("n/p", "next/prev"),
                    ("+/-", "vol"), ("`", "art mode"), ("c", "config"), ("q", "quit"),
                ], app.effective_accent())
            };
            f.render_widget(Paragraph::new(footer), notif_area);
        }
    }

    if app.connection != ConnectionState::Connected {
        draw_disconnected_overlay(f, area, &app.connection);
    }

    if let Some(modal) = &app.config_modal {
        draw_config_modal(f, modal, app.effective_accent());
    }

    if let Some(modal) = &app.sync_modal {
        draw_sync_modal(f, modal, app.effective_accent(), area);
    }

    if app.confirm_clear_queue {
        draw_confirm_clear_queue(f, app.queue.len(), app.clear_queue_selected_button, app.effective_accent());
    }

    if let Some(idx) = app.confirm_delete_queue_item {
        let title = app.queue.get(idx).map(|t| t.title.as_str()).unwrap_or("");
        draw_confirm_delete_queue_item(f, title, app.delete_queue_selected_button, app.effective_accent());
    }

    if app.context_menu.is_some() {
        draw_context_menu(f, app, area);
    }
}

fn draw_sidebar(f: &mut Frame, app: &App, area: Rect, state: &mut ListState) {
    let border_style = if app.focus_sidebar {
        Style::default().fg(focus_border_color(app.effective_accent()))
    } else {
        Style::default().fg(unfocus_border_color(app.effective_accent()))
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title_style(Style::default().fg(focus_border_color(app.effective_accent())))
        .title(" Navigation ");

    let (pill_bg, pill_fg) = pill_colors(app.focus_sidebar);

    let items: Vec<ListItem> = app
        .sidebar_items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let label = app.sidebar_label(item);
            let selected = i == app.sidebar_selected;
            if selected {
                if app.use_nerd_icons {
                    ListItem::new(Line::from(vec![
                        pill_endcap_left(pill_bg, true),
                        Span::styled(format!(" {} ", sidebar_nerd_icon(item)), Style::default().fg(focus_border_color(app.effective_accent())).bg(pill_bg)),
                        Span::styled(format!("{} ", label), Style::default().fg(pill_fg).add_modifier(Modifier::BOLD).bg(pill_bg)),
                        pill_endcap_right(pill_bg, true),
                    ]))
                } else {
                    ListItem::new(Line::from(vec![
                        pill_endcap_left(pill_bg, false),
                        Span::styled(format!(" {} ", label), Style::default().fg(pill_fg).add_modifier(Modifier::BOLD).bg(pill_bg)),
                        pill_endcap_right(pill_bg, false),
                    ]))
                }
            } else if app.use_nerd_icons {
                ListItem::new(Line::from(vec![
                    Span::styled(format!("  {} ", sidebar_nerd_icon(item)), Style::default().fg(focus_border_color(app.effective_accent()))),
                    Span::raw(label.to_string()),
                ]))
            } else {
                ListItem::new(format!("  {}", label))
            }
        })
        .collect();

    let total = items.len();
    let visible = area.height.saturating_sub(2) as usize;

    state.select(Some(app.sidebar_selected));

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default())
        .highlight_symbol("");

    f.render_stateful_widget(list, area, state);

    if total > visible {
        let offset = *state.offset_mut();
        let scroll_area = Rect::new(
            area.x + area.width.saturating_sub(1),
            area.y + 1,
            1,
            area.height.saturating_sub(2),
        );
        let mut ss = ScrollbarState::new(total.saturating_sub(visible)).position(offset);
        let (track_style, thumb_style) = scrollbar_accent_styles(app.effective_accent(), app.focus_sidebar);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_symbol("║")
            .track_symbol(Some("│"))
            .begin_symbol(None)
            .end_symbol(None)
            .track_style(track_style)
            .thumb_style(thumb_style);
        f.render_stateful_widget(scrollbar, scroll_area, &mut ss);
    }
}

fn draw_main(f: &mut Frame, app: &App, area: Rect, state: &mut ListState, thumbnails: &mut HashMap<String, StatefulProtocol>, base: &str) {
    match &app.main_view {
        MainView::MyMusic => draw_my_music(f, app, area, state),
        MainView::Library(lib) => draw_library(f, app, area, lib, state, thumbnails, base),
        MainView::Queue => draw_queue(f, app, area, state, thumbnails),
        MainView::Players => draw_players(f, app, area, state),
        MainView::Radio => draw_radio(f, app, area, state, thumbnails),
        MainView::Apps => draw_apps(f, app, area, state, thumbnails),
        MainView::Favourites => draw_favourites(f, app, area, state, thumbnails),
        MainView::Help => draw_help(f, app, area),
        MainView::Search => draw_search(f, app, area, state, thumbnails, base),
    }
}

fn draw_my_music(f: &mut Frame, app: &App, area: Rect, state: &mut ListState) {
    let focused = !app.focus_sidebar;
    let mid = mid_accent_color(app.effective_accent());
    let border_style = if focused {
        Style::default().fg(focus_border_color(app.effective_accent()))
    } else {
        Style::default().fg(unfocus_border_color(app.effective_accent()))
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title_style(Style::default().fg(focus_border_color(app.effective_accent())))
        .title(" My Music ");

    let entries: [(&str, &str, &str); 6] = if app.use_nerd_icons {
        [
            ("\u{F0C0}", "Artists",       "your music library by artist"), // nf-fa-users
            ("\u{F007}", "Album Artists", "artists with full albums"),     // nf-fa-user
            ("\u{F025}", "Albums",        "all albums"),                   // nf-fa-headphones
            ("\u{F001}", "Tracks",        "all tracks"),                   // nf-fa-music
            ("\u{F07B}", "Folders",       "browse by folder"),             // nf-fa-folder
            ("\u{F0C9}", "Playlists",     "saved playlists"),              // nf-fa-list
        ]
    } else {
        [
            ("▸", "Artists",       "your music library by artist"),
            ("▸", "Album Artists", "artists with full albums"),
            ("▸", "Albums",        "all albums"),
            ("▸", "Tracks",        "all tracks"),
            ("▸", "Folders",       "browse by folder"),
            ("▸", "Playlists",     "saved playlists"),
        ]
    };

    let (pill_bg, pill_fg) = pill_colors(focused);

    let items: Vec<ListItem> = entries.iter().enumerate().map(|(i, (icon, label, sub))| {
        if i == app.main_selected {
            ListItem::new(Line::from(vec![
                pill_endcap_left(pill_bg, app.use_nerd_icons),
                Span::styled(format!(" {}  ", icon), Style::default().fg(focus_border_color(app.effective_accent())).bg(pill_bg)),
                Span::styled(label.to_string(), Style::default().fg(pill_fg).add_modifier(Modifier::BOLD).bg(pill_bg)),
                Span::styled(format!("  — {} ", sub), Style::default().fg(focus_border_color(app.effective_accent())).bg(pill_bg)),
                pill_endcap_right(pill_bg, app.use_nerd_icons),
            ]))
        } else {
            ListItem::new(Line::from(vec![
                Span::styled(format!("  {}  ", icon), Style::default().fg(focus_border_color(app.effective_accent()))),
                Span::raw(label.to_string()),
                Span::styled(format!("  — {}", sub), Style::default().fg(mid)),
            ]))
        }
    }).collect();

    state.select(Some(app.main_selected));

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default())
        .highlight_symbol("");

    f.render_stateful_widget(list, area, state);
}

fn draw_library(f: &mut Frame, app: &App, area: Rect, view: &LibraryView, state: &mut ListState, thumbnails: &mut HashMap<String, StatefulProtocol>, base: &str) {
    let focused = !app.focus_sidebar;
    let mid = mid_accent_color(app.effective_accent());
    match view {
        LibraryView::Artists => {
            let items = app.artists.iter().map(|a| RowItem {
                thumb_url: Some(format!("{}/music/{}/artist.jpg", base, value_id_str(&a.id))),
                line1: if app.use_nerd_icons {
                    Line::from(vec![
                        Span::styled("\u{F007} ", Style::default().fg(focus_border_color(app.effective_accent()))),  // nf-fa-user
                        Span::raw(a.artist.clone()),
                    ])
                } else {
                    Line::from(Span::raw(a.artist.clone()))
                },
                line2: Line::from(Span::styled("artist", Style::default().fg(mid))),
            }).collect();
            draw_two_row_list(f, area, " Artists ", items, app.main_selected, focused, false, state, thumbnails, app.effective_accent());
        }
        LibraryView::AlbumArtists => {
            let items = app.album_artists.iter().map(|a| RowItem {
                thumb_url: Some(format!("{}/music/{}/artist.jpg", base, value_id_str(&a.id))),
                line1: if app.use_nerd_icons {
                    Line::from(vec![
                        Span::styled("\u{F007} ", Style::default().fg(focus_border_color(app.effective_accent()))),  // nf-fa-user
                        Span::raw(a.artist.clone()),
                    ])
                } else {
                    Line::from(Span::raw(a.artist.clone()))
                },
                line2: Line::from(Span::styled("album artist", Style::default().fg(mid))),
            }).collect();
            draw_two_row_list(f, area, " Album Artists ", items, app.main_selected, focused, false, state, thumbnails, app.effective_accent());
        }
        LibraryView::Albums { .. } => {
            let items = app.albums.iter().map(|a| {
                let sub = a.artist.as_deref().unwrap_or("Unknown Artist");
                RowItem {
                    thumb_url: Some(format!("{}/music/{}/cover.jpg", base, value_id_str(&a.id))),
                    line1: if app.use_nerd_icons {
                        Line::from(vec![
                            Span::styled("\u{F025} ", Style::default().fg(focus_border_color(app.effective_accent()))),  // nf-fa-headphones
                            Span::raw(a.album.clone()),
                        ])
                    } else {
                        Line::from(Span::raw(a.album.clone()))
                    },
                    line2: Line::from(Span::styled(sub.to_string(), Style::default().fg(mid))),
                }
            }).collect();
            draw_two_row_list(f, area, " Albums ", items, app.main_selected, focused, app.is_loading, state, thumbnails, app.effective_accent());
        }
        LibraryView::Tracks { album_id } => {
            let title = if album_id.is_some() { " Tracks " } else { " All Tracks " };
            let playing_title = app.now_playing.as_ref().map(|n| n.title.as_str()).unwrap_or("");
            let items = app.tracks.iter().enumerate().map(|(i, t)| {
                let is_current = t.title == playing_title && !playing_title.is_empty();
                let (icon_style, title_style, l2_style) = if is_current {
                    (Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                     Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                     Style::default().fg(Color::Green))
                } else {
                    (Style::default().fg(mid),
                     Style::default().fg(Color::White),
                     Style::default().fg(mid))
                };
                let icon = if is_current {
                    if app.use_nerd_icons { "\u{F04B} " } else { "▶ " }
                } else if app.use_nerd_icons {
                    "\u{F001} "
                } else {
                    "▸ "
                };
                let artist_album = match (t.artist.as_deref(), t.album.as_deref()) {
                    (Some(ar), Some(al)) => format!("{} — {}", ar, al),
                    (Some(ar), None) => ar.to_string(),
                    _ => String::new(),
                };
                let subtitle = if artist_album.is_empty() {
                    format!("{}", i + 1)
                } else {
                    format!("{}  {}", i + 1, artist_album)
                };
                RowItem {
                    thumb_url: t.artwork_url.clone()
                        .or_else(|| t.id.as_ref().map(|id| format!("{}/music/{}/cover.jpg", base, value_id_str(id)))),
                    line1: Line::from(vec![
                        Span::styled(icon, icon_style),
                        Span::styled(t.title.clone(), title_style),
                    ]),
                    line2: Line::from(Span::styled(subtitle, l2_style)),
                }
            }).collect();
            draw_two_row_list(f, area, title, items, app.main_selected, focused, app.is_loading, state, thumbnails, app.effective_accent());
        }
        LibraryView::Folder { .. } => {
            let breadcrumb = breadcrumb_str(
                app.folder_nav_stack.iter().map(|n| n.title.as_str()),
                &app.folder_title,
            );
            let title = format!(" Folders — {} ", breadcrumb);
            let items = app.folder_items.iter().map(|item| {
                let is_track = item.item_type == FolderItemType::Track;
                let (icon, fg) = if is_track {
                    ("▶ ", focus_border_color(app.effective_accent()))
                } else {
                    ("▸ ", Color::White)
                };
                let sub = if is_track {
                    item.duration.as_deref().unwrap_or("").to_string()
                } else {
                    "folder".to_string()
                };
                RowItem {
                    thumb_url: if is_track {
                        Some(format!("{}/music/{}/cover.jpg", base, item.id))
                    } else {
                        None
                    },
                    line1: Line::from(Span::styled(
                        format!("{}{}", icon, item.filename),
                        Style::default().fg(fg),
                    )),
                    line2: Line::from(Span::styled(
                        sub.clone(),
                        Style::default().fg(mid),
                    )),
                }
            }).collect();
            draw_two_row_list(f, area, &title, items, app.main_selected, focused, app.is_loading, state, thumbnails, app.effective_accent());
        }
        LibraryView::Playlists => {
            let items = app.playlists.iter().map(|p| RowItem {
                thumb_url: Some(format!("{}/music/{}/cover.jpg", base, value_id_str(&p.id))),
                line1: if app.use_nerd_icons {
                    Line::from(vec![
                        Span::styled("\u{F0C9} ", Style::default().fg(focus_border_color(app.effective_accent()))),  // nf-fa-list
                        Span::raw(p.name.clone()),
                    ])
                } else {
                    Line::from(Span::raw(p.name.clone()))
                },
                line2: Line::from(Span::styled("playlist", Style::default().fg(mid))),
            }).collect();
            draw_two_row_list(f, area, " Playlists ", items, app.main_selected, focused, app.is_loading, state, thumbnails, app.effective_accent());
        }
    }
}

fn draw_queue(f: &mut Frame, app: &App, area: Rect, state: &mut ListState, thumbnails: &mut HashMap<String, StatefulProtocol>) {
    let focused = !app.focus_sidebar;
    let mid = mid_accent_color(app.effective_accent());
    let cur_idx = app.now_playing.as_ref().and_then(|n| n.playlist_cur_index);

    let items = app.queue.iter().enumerate().map(|(i, t)| {
        let is_current = cur_idx.map(|idx| idx == i).unwrap_or(false);
        let (icon_style, title_style, l2_style) = if is_current {
            (Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
             Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
             Style::default().fg(Color::Green))
        } else {
            (Style::default().fg(mid),
             Style::default().fg(Color::White),
             Style::default().fg(mid))
        };
        let icon = if is_current {
            if app.use_nerd_icons { "\u{F04B} " } else { "▶ " }
        } else if app.use_nerd_icons {
            "\u{F001} "  // nf-fa-music (tracks icon)
        } else {
            "▸ "
        };
        let artist_album = match (t.artist.as_deref(), t.album.as_deref()) {
            (Some(ar), Some(al)) => format!("{} — {}", ar, al),
            (Some(ar), None) => ar.to_string(),
            _ => String::new(),
        };
        let subtitle = if artist_album.is_empty() {
            format!("{}", i + 1)
        } else {
            format!("{}  {}", i + 1, artist_album)
        };
        RowItem {
            thumb_url: t.artwork_url.clone(),
            line1: Line::from(vec![
                Span::styled(icon, icon_style),
                Span::styled(t.title.clone(), title_style),
            ]),
            line2: Line::from(Span::styled(subtitle, l2_style)),
        }
    }).collect();

    draw_two_row_list(f, area, " Queue ", items, app.main_selected, focused, false, state, thumbnails, app.effective_accent());
}

fn draw_players(f: &mut Frame, app: &App, area: Rect, state: &mut ListState) {
    let focused = !app.focus_sidebar;
    let mid = mid_accent_color(app.effective_accent());
    let border_style = if focused {
        Style::default().fg(focus_border_color(app.effective_accent()))
    } else {
        Style::default().fg(unfocus_border_color(app.effective_accent()))
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title_style(Style::default().fg(focus_border_color(app.effective_accent())))
        .title(" Players ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    let pwr_w = PLAYERS_PWR_BTN_W as usize;
    let pwr_icon = icon_power(app.use_nerd_icons);

    // Shared layout: compute bar width once so global row and per-player rows are aligned.
    let vol_icon_str: &str = if app.use_nerd_icons { "\u{F028} " } else { "" };
    // Pad volume to 3 digits so vol_str width is constant (" 🔊  75%" / "  75%").
    let vol_str_w = 1 + vol_icon_str.chars().count() + 4; // 1 space + icon + 3 digits + '%'
    // Reserve 1 col on each side for the pill endcap characters.
    let row_w = (chunks[0].width as usize).saturating_sub(1); // same for all rows (vertical split)
    // Fixed: pwr btn + sync btn + vol string + 1 gap + 3 label padding (" " + "  ")
    let player_fixed_w = pwr_w + PLAYERS_SYNC_BTN_W as usize + vol_str_w + 1 + 3;
    let player_total_flex = row_w.saturating_sub(player_fixed_w);
    let player_bar_w = player_total_flex / 2;
    // Name column width; also used to align the global label so bars share the same column.
    let player_name_col_w = player_total_flex.saturating_sub(player_bar_w);

    let (pill_bg, _pill_fg) = pill_colors(focused);

    // --- Global volume row ---
    let global_avg: u8 = if app.players.is_empty() {
        0
    } else {
        let sum: u32 = app.players.iter()
            .map(|p| app.player_volumes.get(&p.playerid).copied().unwrap_or(0) as u32)
            .sum();
        (sum / app.players.len() as u32) as u8
    };

    let glob_focused = focused && app.players_focus_global;
    let glob_bg = if glob_focused { Color::Rgb(45, 100, 170) } else { Color::Reset };
    let glob_fg = if glob_focused { Color::Rgb(220, 235, 255) } else { mid };

    let checkbox = if app.global_volume_control { "[x]" } else { "[ ]" };
    let vol_str = format!(" {}{:3}%", vol_icon_str, global_avg);
    let filled = if player_bar_w > 0 { (global_avg as usize * player_bar_w) / 100 } else { 0 };
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(player_bar_w.saturating_sub(filled)));

    // Pad the label suffix so the bar starts at the same column as per-player bars.
    // Per-player bar starts at: pwr(3) + label(player_name_col_w+3) + sync(3) = player_name_col_w+9.
    // Global bar must start at: pwr(3) + " "(1) + checkbox(3) + suffix = player_name_col_w+9.
    // So suffix must be player_name_col_w+9-3-1-3 = player_name_col_w+2 chars.
    let glob_suffix_w = player_name_col_w + 2; // " Global vol" = 11 core chars + padding
    let glob_suffix = format!("{:<width$}", " Global vol", width = glob_suffix_w);

    let all_powered = !app.players.is_empty() && app.players.iter().all(|p| p.power > 0);
    let glob_pwr_fg = if all_powered {
        focus_border_color(app.effective_accent())
    } else {
        btn_dim_color(app.effective_accent())
    };
    let checkbox_color = if app.global_volume_control {
        focus_border_color(app.effective_accent())
    } else {
        mid
    };
    let global_line = if glob_focused {
        Line::from(vec![
            pill_endcap_left(pill_bg, app.use_nerd_icons),
            Span::styled(format!(" {} ", pwr_icon), Style::default().fg(glob_pwr_fg).bg(glob_bg)),
            Span::styled(" ", Style::default().fg(glob_fg).bg(glob_bg)),
            Span::styled(checkbox, Style::default().fg(checkbox_color).bg(glob_bg).add_modifier(Modifier::BOLD)),
            Span::styled(glob_suffix, Style::default().fg(glob_fg).bg(glob_bg)),
            Span::styled(bar, Style::default().fg(Color::Rgb(100, 180, 255)).bg(glob_bg)),
            Span::styled(&vol_str, Style::default().fg(Color::White).bg(glob_bg)),
            pill_endcap_right(pill_bg, app.use_nerd_icons),
        ])
    } else {
        Line::from(vec![
            Span::raw(" "),
            Span::styled(format!(" {} ", pwr_icon), Style::default().fg(glob_pwr_fg).bg(glob_bg)),
            Span::styled(" ", Style::default().fg(glob_fg).bg(glob_bg)),
            Span::styled(checkbox, Style::default().fg(checkbox_color).bg(glob_bg).add_modifier(Modifier::BOLD)),
            Span::styled(glob_suffix, Style::default().fg(glob_fg).bg(glob_bg)),
            Span::styled(bar, Style::default().fg(Color::Rgb(60, 80, 110)).bg(glob_bg)),
            Span::styled(&vol_str, Style::default().fg(mid).bg(glob_bg)),
        ])
    };
    f.render_widget(Paragraph::new(global_line), chunks[0]);

    // --- Player list ---
    if app.players.is_empty() {
        state.select(None);
        f.render_widget(
            Paragraph::new("(no players)").style(Style::default().fg(mid)),
            chunks[1],
        );
        return;
    }

    let list_area = chunks[1];
    let total = app.players.len();
    let visible = list_area.height as usize;

    let offset = if !app.players_focus_global {
        sync_scroll_offset(state, app.main_selected, visible)
    } else {
        *state.offset_mut()
    };

    for (vis_i, item_i) in (offset..).zip(0usize..) {
        if vis_i >= total { break; }
        let y = list_area.y + item_i as u16;
        if y >= list_area.y + list_area.height { break; }

        let p = &app.players[vis_i];
        let active = app.active_player.as_deref() == Some(p.playerid.as_str());
        let powered = p.power > 0;
        let is_sel = focused && !app.players_focus_global && vis_i == app.main_selected;
        let vol = app.player_volumes.get(&p.playerid).copied().unwrap_or(0);

        // Use shared bar/name widths so all rows (global + per-player) are aligned.
        let vol_str = format!(" {}{:3}%", vol_icon_str, vol);
        let bar_w = player_bar_w;
        let name_col_w = player_name_col_w;
        let filled = if bar_w > 0 { (vol as usize * bar_w) / 100 } else { 0 };
        let bar_str = format!("{}{}", "█".repeat(filled), "░".repeat(bar_w.saturating_sub(filled)));

        let marker = if active { "● " } else { "○ " };
        let name_raw = format!("{}{}", marker, p.name);

        // Pad/truncate to name_col_w display chars.
        let name_padded = if name_raw.chars().count() > name_col_w {
            let s: String = name_raw.chars().take(name_col_w.saturating_sub(1)).collect();
            format!("{}…", s)
        } else {
            format!("{:<width$}", name_raw, width = name_col_w)
        };
        let label = format!(" {}  ", name_padded);

        let row_bg = if is_sel { Color::Rgb(45, 100, 170) } else { Color::Reset };
        let name_fg = if is_sel { Color::Rgb(220, 235, 255) }
                      else if active { Color::Green }
                      else if powered { Color::White }
                      else { mid };
        let bar_color = if is_sel { Color::Rgb(100, 180, 255) } else { Color::Rgb(60, 80, 110) };
        let vol_fg = if is_sel { Color::White } else { mid };

        // Power button: accent when on, dim when off (always uses btn_bg_color background).
        let player_pwr_fg = if powered {
            focus_border_color(app.effective_accent())
        } else {
            btn_dim_color(app.effective_accent())
        };

        // Sync button — bright accent if player is currently in a sync group, dim otherwise
        let is_synced = app.player_sync_groups
            .get(&p.playerid)
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        let sync_fg = if is_synced {
            focus_border_color(app.effective_accent())
        } else {
            btn_dim_color(app.effective_accent())
        };

        let line = if is_sel {
            Line::from(vec![
                pill_endcap_left(pill_bg, app.use_nerd_icons),
                Span::styled(format!(" {} ", pwr_icon), Style::default().fg(player_pwr_fg).bg(row_bg)),
                Span::styled(label,   Style::default().fg(name_fg).bg(row_bg)),
                Span::styled(" ⇄ ",   Style::default().fg(sync_fg).bg(row_bg)),
                Span::styled(bar_str, Style::default().fg(bar_color).bg(row_bg)),
                Span::styled(vol_str, Style::default().fg(vol_fg).bg(row_bg)),
                pill_endcap_right(pill_bg, app.use_nerd_icons),
            ])
        } else {
            Line::from(vec![
                Span::raw(" "),
                Span::styled(format!(" {} ", pwr_icon), Style::default().fg(player_pwr_fg).bg(row_bg)),
                Span::styled(label,   Style::default().fg(name_fg).bg(row_bg)),
                Span::styled(" ⇄ ",   Style::default().fg(sync_fg).bg(row_bg)),
                Span::styled(bar_str, Style::default().fg(bar_color).bg(row_bg)),
                Span::styled(vol_str, Style::default().fg(vol_fg).bg(row_bg)),
            ])
        };
        f.render_widget(Paragraph::new(line), Rect::new(list_area.x, y, list_area.width, 1));
    }

    if total > visible {
        let scroll_area = Rect::new(
            area.x + area.width.saturating_sub(1),
            list_area.y,
            1,
            list_area.height,
        );
        let mut ss = ScrollbarState::new(total.saturating_sub(visible)).position(offset);
        let (track_style, thumb_style) = scrollbar_accent_styles(app.effective_accent(), focused);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_symbol("║")
            .track_symbol(Some("│"))
            .begin_symbol(None)
            .end_symbol(None)
            .track_style(track_style)
            .thumb_style(thumb_style);
        f.render_stateful_widget(scrollbar, scroll_area, &mut ss);
    }
}

fn draw_radio(f: &mut Frame, app: &App, area: Rect, state: &mut ListState, thumbnails: &mut HashMap<String, StatefulProtocol>) {
    let focused = !app.focus_sidebar;
    let mid = mid_accent_color(app.effective_accent());
    let breadcrumb = breadcrumb_str(app.radio_nav_stack.iter().map(|n| n.title.as_str()), &app.radio_title);
    let title = format!(" {} ", breadcrumb);
    let items = app.radio_items.iter().map(|item| {
        let (icon, fg): (&str, Color) = match (app.use_nerd_icons, item.is_playable()) {
            (true,  true)  => ("\u{F130} ", focus_border_color(app.effective_accent())),  // nf-fa-microphone
            (true,  false) => ("\u{F07B} ", Color::White),                                 // nf-fa-folder
            (false, true)  => ("▶ ", focus_border_color(app.effective_accent())),
            (false, false) => ("▸ ", Color::White),
        };
        RowItem {
            thumb_url: item.artwork_url.clone(),
            line1: Line::from(Span::styled(format!("{}{}", icon, item.name), Style::default().fg(fg))),
            line2: Line::from(Span::styled(
                if item.is_playable() { "stream" } else { "folder" },
                Style::default().fg(mid),
            )),
        }
    }).collect();
    draw_two_row_list(f, area, &title, items, app.main_selected, focused, app.is_loading, state, thumbnails, app.effective_accent());
}

fn draw_apps(f: &mut Frame, app: &App, area: Rect, state: &mut ListState, thumbnails: &mut HashMap<String, StatefulProtocol>) {
    let focused = !app.focus_sidebar;
    let mid = mid_accent_color(app.effective_accent());
    let breadcrumb = breadcrumb_str(app.app_nav_stack.iter().map(|n| n.title.as_str()), &app.app_title);
    let title = format!(" {} ", breadcrumb);
    let items = app.app_items.iter().map(|item| {
        let icon = match (app.use_nerd_icons, item.is_playable()) {
            (true,  true)  => "\u{F130} ",  // nf-fa-microphone
            (true,  false) => "\u{F07B} ",  // nf-fa-folder
            (false, true)  => "▶ ",
            (false, false) => "▸ ",
        };
        RowItem {
            thumb_url: item.artwork_url.clone(),
            line1: Line::from(Span::styled(format!("{}{}", icon, item.name), Style::default().fg(Color::White))),
            line2: Line::from(Span::styled(
                if item.is_playable() { "stream" } else { "folder" },
                Style::default().fg(mid),
            )),
        }
    }).collect();
    draw_two_row_list(f, area, &title, items, app.main_selected, focused, app.is_loading, state, thumbnails, app.effective_accent());
}

fn draw_favourites(f: &mut Frame, app: &App, area: Rect, state: &mut ListState, thumbnails: &mut HashMap<String, StatefulProtocol>) {
    let focused = !app.focus_sidebar;
    let mid = mid_accent_color(app.effective_accent());
    let breadcrumb = breadcrumb_str(app.fav_nav_stack.iter().map(|n| n.title.as_str()), &app.fav_title);
    let title = format!(" {} ", breadcrumb);
    let items = app.fav_items.iter().map(|item| {
        let icon = match (app.use_nerd_icons, item.is_playable()) {
            (true,  true)  => "\u{F130} ",  // nf-fa-microphone
            (true,  false) => "\u{F07B} ",  // nf-fa-folder
            (false, true)  => "▶ ",
            (false, false) => "▸ ",
        };
        RowItem {
            thumb_url: item.artwork_url.clone(),
            line1: Line::from(Span::styled(format!("{}{}", icon, item.name), Style::default().fg(Color::White))),
            line2: Line::from(Span::styled(
                if item.is_playable() { "stream" } else { "folder" },
                Style::default().fg(mid),
            )),
        }
    }).collect();
    draw_two_row_list(f, area, &title, items, app.main_selected, focused, app.is_loading, state, thumbnails, app.effective_accent());
}

fn draw_search(f: &mut Frame, app: &App, area: Rect, state: &mut ListState, thumbnails: &mut HashMap<String, StatefulProtocol>, base: &str) {
    let focused = !app.focus_sidebar;
    let mid = mid_accent_color(app.effective_accent());

    let border_style = if focused {
        Style::default().fg(focus_border_color(app.effective_accent()))
    } else {
        Style::default().fg(unfocus_border_color(app.effective_accent()))
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title_style(Style::default().fg(focus_border_color(app.effective_accent())))
        .title(" Search ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(inner);

    // Search input box
    let input_border_style = if app.search_input_active {
        Style::default().fg(focus_border_color(app.effective_accent()))
    } else {
        Style::default().fg(unfocus_border_color(app.effective_accent()))
    };
    let cursor = if app.search_input_active { "█" } else { "" };
    let search_icon = if app.use_nerd_icons { "\u{F002}" } else { "/" };  // nf-fa-search
    let input_text = format!(" {} {}{}", search_icon, app.search_query, cursor);
    let input = Paragraph::new(input_text)
        .style(Style::default().fg(Color::White))
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(input_border_style));
    f.render_widget(input, chunks[0]);

    let results_area = chunks[1];

    if app.search_results.is_empty() {
        let msg = if app.search_query.is_empty() {
            "Type a query above and press Enter to search"
        } else {
            "No results found"
        };
        f.render_widget(
            Paragraph::new(msg)
                .style(Style::default().fg(mid))
                .alignment(Alignment::Center),
            Rect::new(results_area.x, results_area.y + results_area.height / 2, results_area.width, 1),
        );
        return;
    }

    let results_focused = focused && !app.search_input_active;
    let selected = app.main_selected;
    let visible = ((results_area.height / 2) as usize).max(1);
    let total = app.search_results.len();

    let offset = sync_scroll_offset(state, selected, visible);

    let text_x = results_area.x + THUMB_W + THUMB_SEP;
    let text_w = results_area.width.saturating_sub(THUMB_W + THUMB_SEP);

    for (vis_i, item_i) in (offset..).zip(0usize..) {
        if vis_i >= total { break; }
        let y = results_area.y + (item_i as u16) * 2;
        if y + 1 >= results_area.y + results_area.height { break; }

        let is_sel = vis_i == selected;
        let (s1, s2) = if is_sel { cursor_styles(results_focused) } else { (Style::default(), Style::default()) };

        let thumb_url = match &app.search_results[vis_i] {
            SearchResultItem::Artist(a) => Some(format!("{}/music/{}/artist.jpg", base, value_id_str(&a.id))),
            SearchResultItem::Album(alb) => Some(format!("{}/music/{}/cover.jpg", base, value_id_str(&alb.id))),
            SearchResultItem::Track(t) => t.id.as_ref().map(|id| format!("{}/music/{}/cover.jpg", base, value_id_str(id))),
            SearchResultItem::AppItem(item) => item.artwork_url.clone(),
            SearchResultItem::Playlist(_) => None,
        };
        let thumb_rect = Rect::new(results_area.x, y, THUMB_W, 2);
        let thumb_bg = if is_sel {
            if results_focused { Color::Rgb(45, 100, 170) } else { Color::Rgb(50, 50, 68) }
        } else {
            Color::Rgb(25, 25, 35)
        };
        match thumb_url.as_ref().and_then(|u| thumbnails.get_mut(u)) {
            Some(proto) => {
                let img = StatefulImage::default().resize(Resize::Fit(None));
                f.render_stateful_widget(img, thumb_rect, proto);
            }
            None => {
                f.render_widget(
                    Paragraph::new(if thumb_rect.height >= 2 { "\n ♪" } else { " ♪" })
                        .style(Style::default().fg(Color::Rgb(80, 80, 110)).bg(thumb_bg)),
                    thumb_rect,
                );
            }
        }

        let (line1, line2) = match &app.search_results[vis_i] {
            SearchResultItem::Artist(a) => (
                Line::from(vec![
                    Span::styled(
                        if app.use_nerd_icons { "\u{F007} " } else { "▸ " },  // nf-fa-user
                        Style::default().fg(focus_border_color(app.effective_accent())),
                    ),
                    Span::styled(a.artist.clone(), Style::default().fg(Color::White)),
                ]),
                Line::from(Span::styled("artist", Style::default().fg(mid))),
            ),
            SearchResultItem::Album(alb) => (
                Line::from(vec![
                    Span::styled(
                        if app.use_nerd_icons { "\u{F025} " } else { "▸ " },  // nf-fa-headphones
                        Style::default().fg(focus_border_color(app.effective_accent())),
                    ),
                    Span::styled(alb.album.clone(), Style::default().fg(Color::White)),
                ]),
                Line::from(Span::styled(
                    format!("album  {}", alb.artist.as_deref().unwrap_or("")),
                    Style::default().fg(mid),
                )),
            ),
            SearchResultItem::Track(t) => (
                Line::from(vec![
                    Span::styled("▶ ", Style::default().fg(focus_border_color(app.effective_accent()))),
                    Span::styled(t.title.clone(), Style::default().fg(Color::White)),
                ]),
                Line::from(Span::styled(
                    t.artist.as_deref().unwrap_or("").to_string(),
                    Style::default().fg(mid),
                )),
            ),
            SearchResultItem::Playlist(pl) => (
                Line::from(vec![
                    Span::styled("▸ ", Style::default().fg(Color::Rgb(220, 180, 80))),
                    Span::styled(pl.name.clone(), Style::default().fg(Color::White)),
                ]),
                Line::from(Span::styled("playlist", Style::default().fg(mid))),
            ),
            SearchResultItem::AppItem(item) => (
                Line::from(vec![
                    Span::styled("▸ ", Style::default().fg(Color::Rgb(180, 120, 220))),
                    Span::styled(item.name.clone(), Style::default().fg(Color::White)),
                ]),
                Line::from(Span::styled("app", Style::default().fg(mid))),
            ),
        };

        f.render_widget(
            Paragraph::new(line1).style(s1),
            Rect::new(text_x, y, text_w, 1),
        );
        f.render_widget(
            Paragraph::new(line2).style(s2),
            Rect::new(text_x, y + 1, text_w, 1),
        );
    }

    if total > visible {
        let scroll_area = Rect::new(
            area.x + area.width.saturating_sub(1),
            results_area.y,
            1,
            results_area.height,
        );
        let mut ss = ScrollbarState::new(total.saturating_sub(visible)).position(offset);
        let (track_style, thumb_style) = scrollbar_accent_styles(app.effective_accent(), results_focused);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_symbol("║")
            .track_symbol(Some("│"))
            .begin_symbol(None)
            .end_symbol(None)
            .track_style(track_style)
            .thumb_style(thumb_style);
        f.render_stateful_widget(scrollbar, scroll_area, &mut ss);
    }
}

fn draw_help(f: &mut Frame, app: &App, area: Rect) {
    let accent = focus_border_color(app.effective_accent());
    let mid = mid_accent_color(app.effective_accent());
    let focused = !app.focus_sidebar;
    let border_style = if focused {
        Style::default().fg(accent)
    } else {
        Style::default().fg(unfocus_border_color(app.effective_accent()))
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title_style(Style::default().fg(accent))
        .title(" Keyboard Shortcuts ");

    let inner = block.inner(area);
    f.render_widget(block, area);

    let col_w = inner.width / 2;
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(col_w), Constraint::Min(1)])
        .split(inner);

    let header = |s: &'static str| Line::from(Span::styled(s, Style::default().fg(accent).add_modifier(Modifier::BOLD)));

    let left: Vec<Line> = vec![
        header("Navigation"),
        shortcut("j / ↓",        "Move down",                          mid),
        shortcut("k / ↑",        "Move up",                            mid),
        shortcut("PgDn",         "Jump down 10 items",                 mid),
        shortcut("PgUp",         "Jump up 10 items",                   mid),
        shortcut("Home",         "Jump to top",                        mid),
        shortcut("End",          "Jump to bottom",                     mid),
        shortcut("Enter / l / →", "Select / enter / focus main",       mid),
        shortcut("Esc / h / ←",  "Back / focus sidebar",               mid),
        Line::from(""),
        header("Playback"),
        shortcut("Space",  "Play / pause",                              mid),
        shortcut("n",      "Next track",                                mid),
        shortcut("p",      "Previous track",                            mid),
        shortcut("s",      "Toggle shuffle",                            mid),
        shortcut("r",      "Cycle repeat (off → single → queue → ∞)",  mid),
        shortcut("+ / =",  "Volume up",                                 mid),
        shortcut("-",      "Volume down",                               mid),
    ];

    let right: Vec<Line> = vec![
        header("Library & Queue"),
        shortcut("a",  "Add selected item to queue",                    mid),
        shortcut("d / Del", "Remove selected item from queue",          mid),
        shortcut("x",  "Clear queue",                                   mid),
        Line::from(""),
        header("Players"),
        shortcut("t",  "Toggle player power",                           mid),
        shortcut("Enter (on Global vol)", "Toggle global volume control", mid),
        Line::from(""),
        header("App"),
        shortcut("c",         "Open server configuration",              mid),
        shortcut("q / Ctrl-c", "Quit",                                  mid),
    ];

    let content_lines = left.len().max(right.len()) as u16;
    let visible = inner.height;
    app.help_visible_lines.set(visible);
    let max_scroll = content_lines.saturating_sub(visible);
    let scroll = app.help_scroll.min(max_scroll);

    f.render_widget(Paragraph::new(left).scroll((scroll, 0)), cols[0]);
    f.render_widget(Paragraph::new(right).scroll((scroll, 0)), cols[1]);

    if content_lines > visible {
        let scroll_area = Rect::new(
            area.x + area.width.saturating_sub(1),
            inner.y,
            1,
            inner.height,
        );
        let mut ss = ScrollbarState::new(max_scroll as usize).position(scroll as usize);
        let (track_style, thumb_style) = scrollbar_accent_styles(app.effective_accent(), focused);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_symbol("║")
            .track_symbol(Some("│"))
            .begin_symbol(None)
            .end_symbol(None)
            .track_style(track_style)
            .thumb_style(thumb_style);
        f.render_stateful_widget(scrollbar, scroll_area, &mut ss);
    }
}

fn shortcut(key: &'static str, desc: &'static str, mid: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {:<16}", key), Style::default().fg(Color::White)),
        Span::styled(desc, Style::default().fg(mid)),
    ])
}

/// Builds a styled hint line: keys are White, separators and descriptions use accent mid-tone.
fn hint_line(pairs: &[(&str, &str)], accent: Option<[u8; 3]>) -> Line<'static> {
    let dim = mid_accent_color(accent);
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, (key, action)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", Style::default().fg(dim)));
        }
        spans.push(Span::styled(key.to_string(), Style::default().fg(Color::White)));
        spans.push(Span::styled(format!(":{action}"), Style::default().fg(dim)));
    }
    Line::from(spans)
}

fn draw_statusbar(f: &mut Frame, app: &App, area: Rect, album_art: Option<&mut StatefulProtocol>) {
    let player_name = app.active_player.as_ref()
        .and_then(|id| app.players.iter().find(|p| &p.playerid == id))
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "Now Playing".to_string());
    let mid = mid_accent_color(app.effective_accent());
    let accent = focus_border_color(app.effective_accent());
    let player_icon = icon_player_dot(app.use_nerd_icons);
    let title_line = if let Some(np) = &app.now_playing {
        let vol_icon = icon_vol(app.use_nerd_icons);
        let globe = if app.global_volume_control { icon_globe(app.use_nerd_icons) } else { "" };
        Line::from(vec![
            Span::styled(format!(" {} ", player_icon), Style::default().fg(mid)),
            Span::styled(player_name, Style::default().fg(Color::White)),
            Span::styled(format!("  {} ", vol_icon), Style::default().fg(mid)),
            Span::styled(format!("{}%", np.volume), Style::default().fg(Color::White)),
            Span::styled(format!("{} ", globe), Style::default().fg(accent)),
        ])
    } else {
        Line::from(vec![
            Span::styled(format!(" {} ", player_icon), Style::default().fg(mid)),
            Span::styled(format!("{} ", player_name), Style::default().fg(Color::White)),
        ])
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent))
        .title(title_line);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(np) = &app.now_playing else {
        let msg = Paragraph::new("No player selected — press → then navigate to Players")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(msg, inner);
        return;
    };

    // Compute art column width from the actual rendered inner height so the image fills all
    // available vertical space in the Now Playing bar regardless of terminal resize.
    let fw = app.font_size.0.max(1) as u32;
    let fh = app.font_size.1.max(1) as u32;
    let art_col_w = ((inner.height as u32 * fh) / fw).max(4) as u16;

    // Split: art column | 1-col gap | info column
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(art_col_w), Constraint::Length(1), Constraint::Min(1)])
        .split(inner);

    if let Some(proto) = album_art {
        let img = StatefulImage::<StatefulProtocol>::default().resize(Resize::Fit(None));
        f.render_stateful_widget(img, cols[0], proto);
    }

    draw_now_playing_info(f, app, np, cols[2], false);
}

fn draw_now_playing_info(f: &mut Frame, app: &App, np: &NowPlaying, area: Rect, bigscreen: bool) {
    // Controls and progress bar are pinned to the bottom; metadata fills the top.
    let rows = if bigscreen {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // [0] title
                Constraint::Length(1), // [1] artist
                Constraint::Length(1), // [2] album
                Constraint::Length(1), // [3] empty line
                Constraint::Length(1), // [4] player + volume
                Constraint::Min(0),    // [5] flexible spacer
                Constraint::Length(1), // [6] playback controls
                Constraint::Length(1), // [7] empty line
                Constraint::Length(1), // [8] progress bar
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // [0] title
                Constraint::Length(1), // [1] artist
                Constraint::Length(1), // [2] album
                Constraint::Min(0),    // [3] flexible spacer
                Constraint::Length(1), // [4] playback controls
                Constraint::Length(1), // [5] empty line
                Constraint::Length(1), // [6] progress bar
            ])
            .split(area)
    };
    let ctrl_row = if bigscreen { 6 } else { 4 };
    let progress_row = if bigscreen { 8 } else { 6 };
    let indent: &str = if bigscreen { " " } else { "" };
    let mid = mid_accent_color(app.effective_accent());

    let play_icon = if app.use_nerd_icons {
        if np.is_playing { "\u{F04B}" } else { "\u{F04C}" }  // nf-fa-play / nf-fa-pause
    } else if np.is_playing {
        "▶"
    } else {
        "⏸"
    };
    let shuffle_icon = if np.shuffle > 0 {
        if app.use_nerd_icons { " \u{F074}" } else { " ⇌" }  // nf-fa-random
    } else {
        ""
    };
    let repeat_icon = if app.use_nerd_icons {
        match np.repeat {
            1 => " \u{F01E}1",  // repeat single track
            2 => " \u{F01E}",   // repeat queue
            3 => " \u{221E}",   // don't stop the music
            _ => "",
        }
    } else {
        match np.repeat {
            1 => " ↺1",  // repeat single track
            2 => " ↺",   // repeat queue
            3 => " ∞",   // don't stop the music
            _ => "",
        }
    };

    let queue_pos = match (np.playlist_cur_index, np.playlist_tracks) {
        (Some(idx), Some(total)) => format!("  ({}/{})", idx + 1, total),
        _ => String::new(),
    };
    let title_line = Line::from(vec![
        Span::raw(indent),
        Span::styled(format!("{} ", play_icon), Style::default().fg(Color::Green)),
        Span::styled(np.title.clone(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::styled(queue_pos, Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{}{}", shuffle_icon, repeat_icon), Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(title_line), rows[0]);

    let accent = focus_border_color(app.effective_accent());
    let artist_label = if app.use_nerd_icons { "  \u{F007} " } else { "  by " };  // nf-fa-user
    let artist_line = Line::from(vec![
        Span::raw(indent),
        Span::styled(artist_label, Style::default().fg(mid)),
        Span::styled(np.artist.clone(), Style::default().fg(accent)),
    ]);
    f.render_widget(Paragraph::new(artist_line), rows[1]);

    let album_text = match np.year {
        Some(y) => format!("  {} ({})", np.album, y),
        None => format!("  {}", np.album),
    };
    let album_label = if app.use_nerd_icons { "  \u{F025} " } else { "  from " };  // nf-fa-headphones
    let album_line = Line::from(vec![
        Span::raw(indent),
        Span::styled(album_label, Style::default().fg(mid)),
        Span::styled(album_text.trim_start().to_string(), Style::default().fg(accent)),
    ]);
    f.render_widget(Paragraph::new(album_line), rows[2]);

    if bigscreen {
        let player_name = app.active_player.as_ref()
            .and_then(|id| app.players.iter().find(|p| &p.playerid == id))
            .map(|p| p.name.as_str())
            .unwrap_or("—");
        let vol_icon = icon_vol(app.use_nerd_icons);
        let globe_icon = if app.global_volume_control { icon_globe(app.use_nerd_icons) } else { "" };
        let player_vol_line = Line::from(vec![
            Span::styled(if app.use_nerd_icons { " \u{f075a} " } else { " ▶ " }, Style::default().fg(mid)),
            Span::styled(player_name.to_string(), Style::default().fg(Color::White)),
            Span::styled(format!("  {} ", vol_icon), Style::default().fg(mid)),
            Span::styled(format!("{}%", np.volume), Style::default().fg(Color::White)),
            Span::styled(globe_icon, Style::default().fg(accent)),
        ]);
        f.render_widget(Paragraph::new(player_vol_line), rows[4]);
    }

    // Playback control buttons: Prev | Play/Pause | Stop | Next  Shuffle | Repeat
    {
        let ctrl = rows[ctrl_row];
        let ctrl_x = ctrl.x + if bigscreen { 1 } else { 0 };
        let ctrl_max_x = ctrl.x + ctrl.width;
        let play_pause_icon = if app.use_nerd_icons {
            if np.is_playing { "\u{F04C}" } else { "\u{F04B}" }  // nf-fa-pause / nf-fa-play
        } else if np.is_playing {
            "⏸"
        } else {
            "▶"
        };
        let prev_icon = if app.use_nerd_icons { "\u{F048}" } else { "⏮" };  // nf-fa-step-backward
        let stop_icon = if app.use_nerd_icons { "\u{F04D}" } else { "⏹" };  // nf-fa-stop
        let next_icon = if app.use_nerd_icons { "\u{F051}" } else { "⏭" };  // nf-fa-step-forward
        let btn_w: u16 = 3;
        let gap: u16 = 1;
        let sep: u16 = 2;
        let media_icons = [prev_icon, play_pause_icon, stop_icon, next_icon];
        for (i, icon) in media_icons.iter().enumerate() {
            let x = ctrl_x + (i as u16) * (btn_w + gap);
            if x + btn_w > ctrl_max_x { break; }
            f.render_widget(
                Paragraph::new(format!(" {} ", icon))
                    .style(Style::default().fg(focus_border_color(app.effective_accent())).bg(btn_bg_color(app.effective_accent()))),
                Rect::new(x, ctrl.y, btn_w, 1),
            );
        }
        let shuf_icon = if app.use_nerd_icons { "\u{F074}" } else { "⇌" };  // nf-fa-random
        let shuffle_x = ctrl_x + 4 * (btn_w + gap) + sep;
        if shuffle_x + btn_w <= ctrl_max_x {
            let (sfg, sbg) = if np.shuffle > 0 {
                (focus_border_color(app.effective_accent()), btn_active_bg_color(app.effective_accent()))
            } else {
                (btn_dim_color(app.effective_accent()), btn_bg_color(app.effective_accent()))
            };
            f.render_widget(
                Paragraph::new(format!(" {} ", shuf_icon)).style(Style::default().fg(sfg).bg(sbg)),
                Rect::new(shuffle_x, ctrl.y, btn_w, 1),
            );
        }
        let repeat_x = shuffle_x + btn_w + gap;
        if repeat_x + btn_w <= ctrl_max_x {
            let (rfg, rbg, rep_btn) = match np.repeat {
                1 => (
                    focus_border_color(app.effective_accent()),
                    btn_active_bg_color(app.effective_accent()),
                    if app.use_nerd_icons { " \u{F01E}1".to_string() } else { " ↺1".to_string() },
                ),
                2 => (
                    focus_border_color(app.effective_accent()),
                    btn_active_bg_color(app.effective_accent()),
                    if app.use_nerd_icons { " \u{F01E} ".to_string() } else { " ↺ ".to_string() },
                ),
                3 => (
                    focus_border_color(app.effective_accent()),
                    btn_active_bg_color(app.effective_accent()),
                    " ∞ ".to_string(),
                ),
                _ => (
                    btn_dim_color(app.effective_accent()),
                    btn_bg_color(app.effective_accent()),
                    if app.use_nerd_icons { " \u{F01E} ".to_string() } else { " ↺ ".to_string() },
                ),
            };
            f.render_widget(
                Paragraph::new(rep_btn).style(Style::default().fg(rfg).bg(rbg)),
                Rect::new(repeat_x, ctrl.y, btn_w, 1),
            );
        }
        let vol_down_x = repeat_x + btn_w + sep;
        if vol_down_x + btn_w <= ctrl_max_x {
            let vol_down_icon = if app.use_nerd_icons { "\u{F027}" } else { "−" };  // nf-fa-volume-down
            f.render_widget(
                Paragraph::new(format!(" {} ", vol_down_icon))
                    .style(Style::default().fg(focus_border_color(app.effective_accent())).bg(btn_bg_color(app.effective_accent()))),
                Rect::new(vol_down_x, ctrl.y, btn_w, 1),
            );
        }
        let vol_up_x = vol_down_x + btn_w + gap;
        if vol_up_x + btn_w <= ctrl_max_x {
            let vol_up_icon = if app.use_nerd_icons { "\u{F028}" } else { "+" };  // nf-fa-volume-up
            f.render_widget(
                Paragraph::new(format!(" {} ", vol_up_icon))
                    .style(Style::default().fg(focus_border_color(app.effective_accent())).bg(btn_bg_color(app.effective_accent()))),
                Rect::new(vol_up_x, ctrl.y, btn_w, 1),
            );
        }
    }

    let pct = if np.duration > 0.0 {
        ((np.elapsed / np.duration) * 100.0).clamp(0.0, 100.0) as usize
    } else {
        0
    };
    let prog = rows[progress_row];
    let prog_x = prog.x + if bigscreen { 1 } else { 0 };
    let prog_w = prog.width.saturating_sub(if bigscreen { 2 } else { 1 });
    let bar_w = prog_w as usize;
    let filled = pct * bar_w / 100;

    let time = format!(
        "{} / {}",
        format_duration(np.elapsed),
        format_duration(np.duration),
    );
    let tw = time.chars().count();

    // Right-align time text over the progress bar; blend colors at the fill boundary
    let text_start = bar_w.saturating_sub(tw);
    let pure_filled = text_start.min(filled);
    let pure_unfilled = text_start.saturating_sub(filled);
    let over_filled = filled.saturating_sub(text_start).min(tw);
    let _over_unfilled = tw.saturating_sub(over_filled);

    let text_bytes: Vec<char> = time.chars().collect();
    let over_filled_text: String = text_bytes[..over_filled].iter().collect();
    let over_unfilled_text: String = text_bytes[over_filled..].iter().collect();

    let (accent, track_color) = match app.effective_accent() {
        Some([r, g, b]) => (
            Color::Rgb(r, g, b),
            Color::Rgb(
                (r as u16 * 25 / 100 + 20).min(255) as u8,
                (g as u16 * 25 / 100 + 20).min(255) as u8,
                (b as u16 * 25 / 100 + 30).min(255) as u8,
            ),
        ),
        None => (Color::Yellow, Color::Rgb(55, 55, 70)),
    };
    let bar = Line::from(vec![
        Span::styled("█".repeat(pure_filled), Style::default().fg(accent)),
        Span::styled(" ".repeat(pure_unfilled), Style::default().bg(track_color)),
        Span::styled(
            over_filled_text,
            Style::default().bg(accent).fg(Color::Black).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            over_unfilled_text,
            Style::default().bg(track_color).fg(Color::Rgb(210, 215, 225)),
        ),
    ]);
    let progress_rect = Rect::new(prog_x, prog.y, prog_w, prog.height);
    f.render_widget(Paragraph::new(bar), progress_rect);
}

/// Returns the number of terminal columns the album art will actually occupy after
/// proportional scaling, replicating ratatui-image's `Resize::Scale` logic.
/// Falls back to half the available width when image dimensions are unknown.
fn art_rendered_cols(app: &App, area: Rect) -> u16 {
    let Some((img_w, img_h)) = app.art_image_size else {
        return area.width / 2;
    };
    if img_w == 0 || img_h == 0 {
        return area.width / 2;
    }
    let (fw, fh) = app.font_size;
    if fw == 0 || fh == 0 {
        return area.width / 2;
    }
    let avail_w_px = area.width as u32 * fw as u32;
    let avail_h_px = area.height as u32 * fh as u32;
    let wratio = avail_w_px as f64 / img_w as f64;
    let hratio = avail_h_px as f64 / img_h as f64;
    let ratio = wratio.min(hratio);
    let rendered_w_px = (img_w as f64 * ratio).round() as u32;
    let rendered_cols = (rendered_w_px as f32 / fw as f32).ceil() as u16;
    rendered_cols.min(area.width)
}

fn draw_full_art_mode(
    f: &mut Frame,
    app: &App,
    area: Rect,
    album_art: Option<&mut StatefulProtocol>,
    main_state: &mut ListState,
    thumbnails: &mut HashMap<String, StatefulProtocol>,
) {
    // Outer: content row | footer
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let content_area = outer[0];
    let footer_area = outer[1];

    // Compute how many columns the image will actually occupy after proportional scaling,
    // so the right panel can claim any leftover space.
    let image_col_w = art_rendered_cols(app, content_area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(image_col_w), Constraint::Min(1)])
        .split(content_area);

    let image_area = cols[0];
    let info_area = cols[1];

    if let Some(proto) = album_art {
        let img = StatefulImage::<StatefulProtocol>::default().resize(Resize::Scale(None));
        f.render_stateful_widget(img, image_area, proto);
    } else {
        let placeholder = Paragraph::new("♪")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        f.render_widget(placeholder, image_area);
    }

    let info_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(1)])
        .split(info_area);

    let np_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(focus_border_color(app.effective_accent())))
        .title_style(Style::default().fg(focus_border_color(app.effective_accent())))
        .title(" Now Playing ");
    let np_inner = np_block.inner(info_rows[0]);
    f.render_widget(np_block, info_rows[0]);

    if let Some(np) = &app.now_playing {
        draw_now_playing_info(f, app, np, np_inner, false);
    } else {
        let msg = Paragraph::new("No player selected — press → then navigate to Players")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(msg, np_inner);
    }

    draw_queue(f, app, info_rows[1], main_state, thumbnails);

    let mid = mid_accent_color(app.effective_accent());
    let accent = focus_border_color(app.effective_accent());
    let player_name = app.active_player.as_ref()
        .and_then(|id| app.players.iter().find(|p| &p.playerid == id))
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "—".to_string());
    let vol = app.now_playing.as_ref().map(|np| np.volume).unwrap_or(0);
    let vol_icon = icon_vol(app.use_nerd_icons);
    let player_icon = icon_player_dot(app.use_nerd_icons);
    let globe = if app.global_volume_control { icon_globe(app.use_nerd_icons) } else { "" };
    let vol_str = format!("{}%", vol);
    let right_w = art_footer_player_width(app);

    let footer_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(1), Constraint::Length(right_w)])
        .split(footer_area);

    let footer = hint_line(&[
        ("`", "exit art"), ("Spc", "play/pause"), ("n/p", "next/prev"),
        ("+/-", "vol"), ("c", "config"), ("q", "quit"),
    ], app.effective_accent());
    f.render_widget(Paragraph::new(footer), footer_cols[0]);

    let right_line = Line::from(vec![
        Span::styled(format!(" {} ", player_icon), Style::default().fg(mid)),
        Span::styled(player_name, Style::default().fg(Color::White)),
        Span::styled(format!("  {} ", vol_icon), Style::default().fg(mid)),
        Span::styled(vol_str, Style::default().fg(Color::White)),
        Span::styled(format!("{} ", globe), Style::default().fg(accent)),
    ]);
    f.render_widget(Paragraph::new(right_line), footer_cols[1]);
}

fn draw_disconnected_overlay(f: &mut Frame, area: Rect, state: &ConnectionState) {
    let msg = match state {
        ConnectionState::Disconnected => " Disconnected from Lyrion server ",
        ConnectionState::Reconnecting => " Reconnecting to Lyrion server... ",
        ConnectionState::Connected => return,
    };

    let popup_area = centered_rect(40, 3, area);
    f.render_widget(Clear, popup_area);
    let p = Paragraph::new(msg)
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(Color::Red)))
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));
    f.render_widget(p, popup_area);
}

fn breadcrumb_str<'a>(stack: impl Iterator<Item = &'a str>, current: &'a str) -> String {
    let parts: Vec<&str> = stack.chain(std::iter::once(current)).collect();
    parts.join(" › ")
}


#[allow(clippy::too_many_arguments)]
fn draw_two_row_list(
    f: &mut Frame,
    area: Rect,
    title: &str,
    items: Vec<RowItem>,
    selected: usize,
    focused: bool,
    is_loading: bool,
    state: &mut ListState,
    thumbnails: &mut HashMap<String, StatefulProtocol>,
    accent: Option<[u8; 3]>,
) {
    let border_style = if focused {
        Style::default().fg(focus_border_color(accent))
    } else {
        Style::default().fg(unfocus_border_color(accent))
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title_style(Style::default().fg(focus_border_color(accent)))
        .title(title);

    if items.is_empty() {
        state.select(None);
        let msg = if is_loading { "  Loading..." } else { "(empty)" };
        f.render_widget(
            Paragraph::new(msg).block(block).style(Style::default().fg(Color::DarkGray)),
            area,
        );
        return;
    }

    let inner = block.inner(area);
    f.render_widget(block, area);

    let visible = ((inner.height / 2) as usize).max(1);
    let needs_scroll = items.len() > visible;

    let offset = sync_scroll_offset(state, selected, visible);

    let text_x = inner.x + THUMB_W + THUMB_SEP;
    let text_w = inner.width.saturating_sub(THUMB_W + THUMB_SEP);

    for (vis_i, item_i) in (offset..).zip(0usize..) {
        if vis_i >= items.len() { break; }
        let y = inner.y + (item_i as u16) * 2;
        if y + 1 >= inner.y + inner.height { break; }

        let item = &items[vis_i];
        let is_sel = vis_i == selected;

        let (s1, s2) = if is_sel { cursor_styles(focused) } else { (Style::default(), Style::default()) };

        let thumb_rect = Rect::new(inner.x, y, THUMB_W, 2);
        let thumb_bg = if is_sel {
            if focused { Color::Rgb(45, 100, 170) } else { Color::Rgb(50, 50, 68) }
        } else {
            Color::Rgb(25, 25, 35)
        };
        match item.thumb_url.as_ref().and_then(|u| thumbnails.get_mut(u)) {
            Some(proto) => {
                let img = StatefulImage::default().resize(Resize::Fit(None));
                f.render_stateful_widget(img, thumb_rect, proto);
            }
            None => {
                f.render_widget(
                    Paragraph::new(if thumb_rect.height >= 2 { "\n ♪" } else { " ♪" })
                        .style(Style::default().fg(Color::Rgb(80, 80, 110)).bg(thumb_bg)),
                    thumb_rect,
                );
            }
        }

        f.render_widget(
            Paragraph::new(item.line1.clone()).style(s1),
            Rect::new(text_x, y, text_w, 1),
        );
        f.render_widget(
            Paragraph::new(item.line2.clone()).style(s2),
            Rect::new(text_x, y + 1, text_w, 1),
        );
    }

    if needs_scroll {
        // Place on the right border column; skip the two corner cells
        let scroll_area = Rect::new(
            area.x + area.width.saturating_sub(1),
            area.y + 1,
            1,
            area.height.saturating_sub(2),
        );
        let mut ss = ScrollbarState::new(items.len().saturating_sub(visible))
            .position(offset);
        let (track_style, thumb_style) = scrollbar_accent_styles(accent, focused);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_symbol("║")
            .track_symbol(Some("│"))
            .begin_symbol(None)
            .end_symbol(None)
            .track_style(track_style)
            .thumb_style(thumb_style);
        f.render_stateful_widget(scrollbar, scroll_area, &mut ss);
    }
}

/// Returns (track_style, thumb_style) for a scrollbar tinted from the accent color.
/// Uses focus/unfocus border color to match the surrounding panel border.
fn scrollbar_accent_styles(accent: Option<[u8; 3]>, focused: bool) -> (Style, Style) {
    let color = if focused { focus_border_color(accent) } else { unfocus_border_color(accent) };
    let style = Style::default().fg(color);
    (style, style)
}

fn value_id_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn format_duration(secs: f64) -> String {
    let s = secs as u64;
    let m = s / 60;
    let h = m / 60;
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m % 60, s % 60)
    } else {
        format!("{}:{:02}", m, s % 60)
    }
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let w = area.width * percent_x / 100;
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height - height) / 2;
    Rect::new(x, y, w, height)
}

/// Returns the queue widget rect in full art mode (right column, below the Now Playing block).
pub fn compute_full_art_queue_rect(area: Rect, app: &App) -> Rect {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let content_area = outer[0];
    let image_col_w = art_rendered_cols(app, content_area);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(image_col_w), Constraint::Min(1)])
        .split(content_area);
    let info_area = cols[1];
    let info_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(1)])
        .split(info_area);
    info_rows[1]
}

/// Returns the eight clickable button rects in the big-screen (full art) controls row: [Prev, PlayPause, Stop, Next, Shuffle, Repeat, VolumeDown, VolumeUp].
pub fn compute_full_art_control_rects(area: Rect, app: &App) -> [Rect; 8] {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let content_area = outer[0];
    let image_col_w = art_rendered_cols(app, content_area);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(image_col_w), Constraint::Min(1)])
        .split(content_area);
    let info_area = cols[1];
    let info_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(1)])
        .split(info_area);
    let np_inner = Rect::new(
        info_rows[0].x + 1,
        info_rows[0].y + 1,
        info_rows[0].width.saturating_sub(2),
        info_rows[0].height.saturating_sub(2),
    );
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title
            Constraint::Length(1), // artist
            Constraint::Length(1), // album
            Constraint::Min(0),    // flexible spacer
            Constraint::Length(1), // controls
            Constraint::Length(1), // empty line
            Constraint::Length(1), // progress
        ])
        .split(np_inner);
    let ctrl = rows[4];
    let btn_w: u16 = 3;
    let gap: u16 = 1;
    let sep: u16 = 2;
    std::array::from_fn(|i| {
        let x = if i < 4 {
            ctrl.x + (i as u16) * (btn_w + gap)
        } else if i < 6 {
            ctrl.x + 4 * (btn_w + gap) + sep + ((i - 4) as u16) * (btn_w + gap)
        } else {
            // volume down (6) and volume up (7) after a second sep gap
            ctrl.x + 4 * (btn_w + gap) + sep + 2 * (btn_w + gap) + sep + ((i - 6) as u16) * (btn_w + gap)
        };
        Rect::new(x, ctrl.y, btn_w, 1)
    })
}

/// Returns the rect covering the "`:exit art`" footer hint at the bottom-left of big-screen mode.
pub fn compute_full_art_footer_exit_rect(area: Rect) -> Rect {
    let footer_y = area.y + area.height.saturating_sub(1);
    // "`" (1) + ":exit art" (9) = 10 chars; add 2 for padding
    Rect::new(area.x, footer_y, 12, 1)
}

fn art_footer_player_width(app: &App) -> u16 {
    let player_name = app.active_player.as_ref()
        .and_then(|id| app.players.iter().find(|p| &p.playerid == id))
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "—".to_string());
    let vol = app.now_playing.as_ref().map(|np| np.volume).unwrap_or(0);
    let vol_icon = icon_vol(app.use_nerd_icons);
    let player_icon = icon_player_dot(app.use_nerd_icons);
    let globe = if app.global_volume_control { icon_globe(app.use_nerd_icons) } else { "" };
    let vol_str = format!("{}%", vol);
    (1 + player_icon.chars().count() + 1
        + player_name.chars().count()
        + 2 + vol_icon.chars().count() + 1
        + vol_str.chars().count()
        + globe.chars().count()
        + 1) as u16
}

/// Returns the rect covering the player name + volume area in the art mode footer (right side).
pub fn compute_full_art_footer_player_rect(area: Rect, app: &App) -> Rect {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let footer_area = outer[1];
    let right_w = art_footer_player_width(app);
    let footer_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(1), Constraint::Length(right_w)])
        .split(footer_area);
    footer_cols[1]
}

/// Returns the left-column image area in full art mode.
pub fn compute_full_art_image_rect(area: Rect, app: &App) -> Rect {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let content_area = outer[0];
    let image_col_w = art_rendered_cols(app, content_area);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(image_col_w), Constraint::Min(1)])
        .split(content_area);
    cols[0]
}

/// Returns the top border row of the Now Playing status bar (player name + volume title area).
pub fn compute_statusbar_title_area(area: Rect, status_height: u16) -> Rect {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(status_height), Constraint::Length(1)])
        .split(area);
    Rect::new(outer[1].x, outer[1].y, outer[1].width, 1)
}

/// Returns the album art column rect inside the Now Playing status bar.
pub fn compute_statusbar_art_rect(area: Rect, status_height: u16, art_col_w: u16) -> Rect {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(status_height), Constraint::Length(1)])
        .split(area);
    let status_inner = Rect::new(
        outer[1].x + 1,
        outer[1].y + 1,
        outer[1].width.saturating_sub(2),
        outer[1].height.saturating_sub(2),
    );
    Rect::new(status_inner.x, status_inner.y, art_col_w.min(status_inner.width), status_inner.height)
}

/// Returns the rect for the song title row in the Now Playing status bar info column.
pub fn compute_statusbar_np_title_rect(area: Rect, status_height: u16, art_col_w: u16) -> Rect {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(status_height), Constraint::Length(1)])
        .split(area);
    let status_inner = Rect::new(
        outer[1].x + 1,
        outer[1].y + 1,
        outer[1].width.saturating_sub(2),
        outer[1].height.saturating_sub(2),
    );
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(art_col_w), Constraint::Length(1), Constraint::Min(1)])
        .split(status_inner);
    Rect::new(cols[2].x, cols[2].y, cols[2].width, 1)
}

pub fn compute_context_menu_rect(area: Rect, option_count: usize) -> Rect {
    centered_rect_abs(44, (option_count + 2) as u16, area)
}

fn draw_context_menu(f: &mut Frame, app: &App, area: Rect) {
    let Some(menu) = &app.context_menu else { return };

    let popup = compute_context_menu_rect(area, menu.option_count());
    f.render_widget(Clear, popup);

    let accent = focus_border_color(app.effective_accent());
    let (pill_bg, pill_fg) = pill_colors(true);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent))
        .title_style(Style::default().fg(accent))
        .title(" What do you want to do? ");

    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let options = menu.options();
    let last = options.len() - 1;
    let items: Vec<ListItem> = options.iter().enumerate().map(|(i, o)| {
        if i == menu.selected {
            ListItem::new(Line::from(vec![
                pill_endcap_left(pill_bg, app.use_nerd_icons),
                Span::styled(format!(" {} ", o), Style::default().fg(pill_fg).add_modifier(Modifier::BOLD).bg(pill_bg)),
                pill_endcap_right(pill_bg, app.use_nerd_icons),
            ]))
        } else if i == last {
            ListItem::new(Line::from(Span::styled(format!("  {}", o), Style::default().fg(Color::DarkGray))))
        } else {
            ListItem::new(Line::from(Span::raw(format!("  {}", o))))
        }
    }).collect();

    let mut state = ListState::default();
    state.select(Some(menu.selected));

    let list = List::new(items)
        .highlight_style(Style::default())
        .highlight_symbol("");

    f.render_stateful_widget(list, inner, &mut state);
}

fn centered_rect_abs(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width.min(area.width), height.min(area.height))
}

/// Returns (popup_rect, [host, port, username, password, nerd_icons, auto_discover, broadcast_mask, disable_auto_colors]).
pub fn compute_config_modal_rects(area: Rect) -> (Rect, [Rect; 8]) {
    let popup = centered_rect_abs(54, 18, area);
    let inner_x = popup.x + 1;
    let inner_y = popup.y + 1;
    let inner_w = popup.width.saturating_sub(2);
    // row layout: [pad, host, port, username, password, divider, nerd, auto_discover, broadcast_mask, disable_auto_colors, error, spacer, help]
    let host_rect          = Rect::new(inner_x, inner_y + 1, inner_w, 1);
    let port_rect          = Rect::new(inner_x, inner_y + 2, inner_w, 1);
    let user_rect          = Rect::new(inner_x, inner_y + 3, inner_w, 1);
    let pass_rect          = Rect::new(inner_x, inner_y + 4, inner_w, 1);
    let nerd_rect          = Rect::new(inner_x, inner_y + 6, inner_w, 1);
    let auto_rect          = Rect::new(inner_x, inner_y + 7, inner_w, 1);
    let mask_rect          = Rect::new(inner_x, inner_y + 8, inner_w, 1);
    let no_colors_rect     = Rect::new(inner_x, inner_y + 9, inner_w, 1);
    (popup, [host_rect, port_rect, user_rect, pass_rect, nerd_rect, auto_rect, mask_rect, no_colors_rect])
}

fn draw_confirm_clear_queue(f: &mut Frame, queue_len: usize, selected_button: u8, accent: Option<[u8; 3]>) {
    let area = f.area();
    let popup = centered_rect_abs(44, 7, area);

    f.render_widget(Clear, popup);

    let accent_color = accent
        .map(|c| Color::Rgb(c[0], c[1], c[2]))
        .unwrap_or(Color::Yellow);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent_color))
        .title_style(Style::default().fg(accent_color))
        .title(" Clear Queue ");

    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);

    let song_word = if queue_len == 1 { "song" } else { "songs" };
    let msg = Paragraph::new(format!("Remove {} {} from the queue?", queue_len, song_word))
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::White));
    f.render_widget(msg, rows[1]);

    let btn_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[3]);

    let ok_style = if selected_button == 0 {
        Style::default().fg(Color::Black).bg(accent_color).bold()
    } else {
        Style::default().fg(Color::White)
    };
    let cancel_style = if selected_button == 1 {
        Style::default().fg(Color::Black).bg(accent_color).bold()
    } else {
        Style::default().fg(Color::White)
    };

    f.render_widget(
        Paragraph::new("[ OK ]").alignment(Alignment::Center).style(ok_style),
        btn_cols[0],
    );
    f.render_widget(
        Paragraph::new("[ Cancel ]").alignment(Alignment::Center).style(cancel_style),
        btn_cols[1],
    );
}

/// Returns (popup_rect, [ok_button_rect, cancel_button_rect]).
pub fn compute_clear_queue_button_rects(area: Rect) -> (Rect, [Rect; 2]) {
    let popup = centered_rect_abs(44, 7, area);
    let inner_x = popup.x + 1;
    let inner_y = popup.y + 1;
    let inner_w = popup.width.saturating_sub(2);
    // Layout rows: pad(1), message(1), spacer(min→2), buttons(1) → buttons at offset 4
    let btn_y = inner_y + 4;
    let half_w = inner_w / 2;
    let ok_rect = Rect::new(inner_x, btn_y, half_w, 1);
    let cancel_rect = Rect::new(inner_x + half_w, btn_y, inner_w - half_w, 1);
    (popup, [ok_rect, cancel_rect])
}

fn sync_modal_popup_height(n_players: usize) -> u16 {
    // border(2) + pad(1) + player_rows(n) + pad(1) + buttons(1) + hint(1) = n + 6
    (n_players as u16) + 6
}

fn draw_sync_modal(f: &mut Frame, modal: &SyncModal, accent: Option<[u8; 3]>, area: Rect) {
    let n = modal.other_players.len();
    let height = sync_modal_popup_height(n);
    let popup = centered_rect_abs(54, height, area);

    f.render_widget(Clear, popup);

    let accent_color = accent
        .map(|c| Color::Rgb(c[0], c[1], c[2]))
        .unwrap_or(Color::Yellow);

    let title = format!(" Sync: {} ", modal.player_name);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent_color))
        .title_style(Style::default().fg(accent_color))
        .title(title);

    let inner = block.inner(popup);
    f.render_widget(block, popup);

    // Layout: pad | player rows (n) | pad | buttons | hint
    let mut constraints = vec![Constraint::Length(1)];
    for _ in 0..n {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1)); // pad
    constraints.push(Constraint::Length(1)); // buttons
    constraints.push(Constraint::Length(1)); // hint

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    // Player rows start at index 1
    for (i, player) in modal.other_players.iter().enumerate() {
        let row_idx = 1 + i;
        let checked = modal.checked.get(i).copied().unwrap_or(false);
        let is_sel = !modal.focus_buttons && modal.list_selected == i;

        let checkbox = if checked { "[x]" } else { "[ ]" };
        let check_fg = if checked { accent_color } else { Color::DarkGray };
        let row_bg = if is_sel { Color::Rgb(45, 100, 170) } else { Color::Reset };
        let name_fg = if is_sel { Color::Rgb(220, 235, 255) } else { Color::White };

        let row_line = Line::from(vec![
            Span::styled(" ", Style::default().bg(row_bg)),
            Span::styled(checkbox, Style::default().fg(check_fg).bg(row_bg)),
            Span::styled(" ", Style::default().bg(row_bg)),
            Span::styled(player.name.clone(), Style::default().fg(name_fg).bg(row_bg)),
        ]);
        f.render_widget(Paragraph::new(row_line), rows[row_idx]);
    }

    // Buttons row
    let btn_row = rows[n + 2];
    let btn_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(btn_row);

    let sync_style = if modal.focus_buttons && modal.selected_button == 0 {
        Style::default().fg(Color::Black).bg(accent_color).bold()
    } else {
        Style::default().fg(Color::White)
    };
    let cancel_style = if modal.focus_buttons && modal.selected_button == 1 {
        Style::default().fg(Color::Black).bg(accent_color).bold()
    } else {
        Style::default().fg(Color::White)
    };

    f.render_widget(
        Paragraph::new("[ Synchronize ]").alignment(Alignment::Center).style(sync_style),
        btn_cols[0],
    );
    f.render_widget(
        Paragraph::new("[ Cancel ]").alignment(Alignment::Center).style(cancel_style),
        btn_cols[1],
    );

    // // Hint row
    // let hint_row = rows[1 + n + 2];
    // let hint = Paragraph::new("↑/↓:move  Space:toggle  Tab:buttons  Enter:confirm  Esc:cancel")
    //     .alignment(Alignment::Center)
    //     .style(Style::default().fg(Color::DarkGray));
    // f.render_widget(hint, hint_row);
}

/// Returns (popup_rect, player_row_rects, [sync_button_rect, cancel_button_rect]).
pub fn compute_sync_modal_rects(area: Rect, n_players: usize) -> (Rect, Vec<Rect>, [Rect; 2]) {
    let height = sync_modal_popup_height(n_players);
    let popup = centered_rect_abs(54, height, area);
    let inner_x = popup.x + 1;
    let inner_y = popup.y + 1;
    let inner_w = popup.width.saturating_sub(2);

    // player rows start at inner_y + 1 (after top pad)
    let player_rects: Vec<Rect> = (0..n_players)
        .map(|i| Rect::new(inner_x, inner_y + 1 + i as u16, inner_w, 1))
        .collect();

    // buttons row: inner_y + 1(pad) + n(players) + 1(pad) = inner_y + n + 2
    let btn_y = inner_y + 1 + n_players as u16 + 1;
    let half_w = inner_w / 2;
    let sync_rect = Rect::new(inner_x, btn_y, half_w, 1);
    let cancel_rect = Rect::new(inner_x + half_w, btn_y, inner_w - half_w, 1);

    (popup, player_rects, [sync_rect, cancel_rect])
}

fn draw_confirm_delete_queue_item(f: &mut Frame, title: &str, selected_button: u8, accent: Option<[u8; 3]>) {
    let area = f.area();
    let popup = centered_rect_abs(54, 7, area);

    f.render_widget(Clear, popup);

    let accent_color = accent
        .map(|c| Color::Rgb(c[0], c[1], c[2]))
        .unwrap_or(Color::Yellow);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent_color))
        .title_style(Style::default().fg(accent_color))
        .title(" Remove from Queue ");

    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);

    let display_title = if title.len() > 46 {
        format!("{}…", &title[..45])
    } else {
        title.to_string()
    };
    let msg = Paragraph::new(format!("Remove \"{}\"?", display_title))
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::White));
    f.render_widget(msg, rows[1]);

    let btn_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[3]);

    let ok_style = if selected_button == 0 {
        Style::default().fg(Color::Black).bg(accent_color).bold()
    } else {
        Style::default().fg(Color::White)
    };
    let cancel_style = if selected_button == 1 {
        Style::default().fg(Color::Black).bg(accent_color).bold()
    } else {
        Style::default().fg(Color::White)
    };

    f.render_widget(
        Paragraph::new("[ OK ]").alignment(Alignment::Center).style(ok_style),
        btn_cols[0],
    );
    f.render_widget(
        Paragraph::new("[ Cancel ]").alignment(Alignment::Center).style(cancel_style),
        btn_cols[1],
    );
}

/// Returns (popup_rect, [ok_button_rect, cancel_button_rect]).
pub fn compute_delete_queue_button_rects(area: Rect) -> (Rect, [Rect; 2]) {
    let popup = centered_rect_abs(54, 7, area);
    let inner_x = popup.x + 1;
    let inner_y = popup.y + 1;
    let inner_w = popup.width.saturating_sub(2);
    let btn_y = inner_y + 4;
    let half_w = inner_w / 2;
    let ok_rect = Rect::new(inner_x, btn_y, half_w, 1);
    let cancel_rect = Rect::new(inner_x + half_w, btn_y, inner_w - half_w, 1);
    (popup, [ok_rect, cancel_rect])
}

fn draw_config_modal(f: &mut Frame, modal: &ConfigModal, accent: Option<[u8; 3]>) {
    let area = f.area();
    let popup = centered_rect_abs(54, 18, area);

    f.render_widget(Clear, popup);

    let accent_bright = focus_border_color(accent);
    let accent_mid    = mid_accent_color(accent);
    let accent_dim    = unfocus_border_color(accent);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent_bright))
        .title_style(Style::default().fg(accent_bright))
        .title(" Configuration ");

    let inner = block.inner(popup);
    f.render_widget(block, popup);

    // rows: pad | host | port | username | password | divider | nerd-icons | auto-discover | broadcast-mask | disable-auto-colors | error | spacer | help
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // [0] top pad
            Constraint::Length(1), // [1] host
            Constraint::Length(1), // [2] port
            Constraint::Length(1), // [3] username
            Constraint::Length(1), // [4] password
            Constraint::Length(1), // [5] divider
            Constraint::Length(1), // [6] nerd-icons
            Constraint::Length(1), // [7] auto-discover
            Constraint::Length(1), // [8] broadcast-mask
            Constraint::Length(1), // [9] disable-auto-colors
            Constraint::Length(1), // [10] error
            Constraint::Min(0),    // [11] spacer
            Constraint::Length(1), // [12] help
        ])
        .split(inner);

    // Text input fields: (label, value, field_index)
    let pass_masked = "*".repeat(modal.password.len());
    let text_fields: &[(&str, &str, usize)] = &[
        ("Host",     &modal.host,     0),
        ("Port",     &modal.port,     1),
        ("Username", &modal.username, 2),
        ("Password", &pass_masked,    3),
    ];
    for (i, (label, value, idx)) in text_fields.iter().enumerate() {
        let is_selected = modal.selected_field == *idx;
        let is_editing = is_selected && modal.editing;

        let cursor = if is_editing { "█" } else { "" };
        let display = format!("{}{}", value, cursor);

        let val_style = if is_editing {
            Style::default().fg(Color::Black).bg(accent_bright)
        } else if is_selected {
            Style::default().fg(accent_bright).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let label_style = if is_selected {
            Style::default().fg(accent_bright)
        } else {
            Style::default().fg(accent_mid)
        };
        let line = Line::from(vec![
            Span::styled(format!("  {:>8}: ", label), label_style),
            Span::styled(display, val_style),
        ]);
        f.render_widget(Paragraph::new(line), rows[i + 1]);
    }

    // Divider
    f.render_widget(
        Paragraph::new(Span::styled(
            "─".repeat(inner.width as usize),
            Style::default().fg(accent_dim),
        )),
        rows[5],
    );

    // Toggle helper closure
    let render_toggle = |f: &mut Frame, row: Rect, label: &str, checked: bool, selected: bool| {
        let checkbox = if checked { "[x]" } else { "[ ]" };
        let val_style = if selected {
            Style::default().fg(accent_bright).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let lbl_style = if selected {
            Style::default().fg(accent_bright)
        } else {
            Style::default().fg(accent_mid)
        };
        let line = Line::from(vec![
            Span::styled(format!("  {:>13}: ", label), lbl_style),
            Span::styled(checkbox, val_style),
        ]);
        f.render_widget(Paragraph::new(line), row);
    };

    render_toggle(f, rows[6], "Nerd icons",    modal.use_nerd_icons, modal.selected_field == 4);
    render_toggle(f, rows[7], "Auto discover", modal.auto_discover,   modal.selected_field == 5);

    // Broadcast mask text field (field 6)
    {
        let is_selected = modal.selected_field == 6;
        let is_editing = is_selected && modal.editing;
        let cursor = if is_editing { "█" } else { "" };
        let display = format!("{}{}", modal.broadcast_mask, cursor);
        let val_style = if is_editing {
            Style::default().fg(Color::Black).bg(accent_bright)
        } else if is_selected {
            Style::default().fg(accent_bright).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let lbl_style = if is_selected {
            Style::default().fg(accent_bright)
        } else {
            Style::default().fg(accent_mid)
        };
        let line = Line::from(vec![
            Span::styled("  Bcast mask: ", lbl_style),
            Span::styled(display, val_style),
        ]);
        f.render_widget(Paragraph::new(line), rows[8]);
    }

    render_toggle(f, rows[9], "Disable auto colors", modal.disable_auto_colors, modal.selected_field == 7);

    if let Some(err) = &modal.error {
        let p = Paragraph::new(err.as_str())
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Red));
        f.render_widget(p, rows[10]);
    }

    let btn_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[12]);

    let ok_style = if modal.selected_field == 8 {
        Style::default().fg(Color::Black).bg(accent_bright).bold()
    } else {
        Style::default().fg(Color::White)
    };
    let cancel_style = if modal.selected_field == 9 {
        Style::default().fg(Color::Black).bg(accent_bright).bold()
    } else {
        Style::default().fg(Color::White)
    };

    f.render_widget(
        Paragraph::new("[ OK ]").alignment(Alignment::Center).style(ok_style),
        btn_cols[0],
    );
    f.render_widget(
        Paragraph::new("[ Cancel ]").alignment(Alignment::Center).style(cancel_style),
        btn_cols[1],
    );
}

/// Returns (popup_rect, [ok_button_rect, cancel_button_rect]).
pub fn compute_config_modal_button_rects(area: Rect) -> (Rect, [Rect; 2]) {
    let popup = centered_rect_abs(54, 18, area);
    let inner_x = popup.x + 1;
    let inner_y = popup.y + 1;
    let inner_w = popup.width.saturating_sub(2);
    // Layout rows: 11 fixed (0-10) + spacer(min→4) + buttons(1) → buttons at offset 15
    let btn_y = inner_y + 15;
    let half_w = inner_w / 2;
    let ok_rect = Rect::new(inner_x, btn_y, half_w, 1);
    let cancel_rect = Rect::new(inner_x + half_w, btn_y, inner_w - half_w, 1);
    (popup, [ok_rect, cancel_rect])
}
