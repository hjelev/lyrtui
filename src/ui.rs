use crate::api::{FolderItemType, NowPlaying, Track};
use crate::app::{
    App, ConfigModal, ConnectionState, IMAGE_PROTOCOLS, LibraryView, MainView, SearchResultItem,
    SearchScope, SidebarItem, SyncModal,
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState,
    },
};
use ratatui_image::{Resize, StatefulImage, protocol::StatefulProtocol};
use std::collections::HashMap;

pub const THUMB_W: u16 = 4; // image column width in cells
const THUMB_SEP: u16 = 1; // gap between image and text
pub const PLAYERS_PWR_BTN_W: u16 = 3; // width of the power button column in the Players screen
pub const PLAYERS_SYNC_BTN_W: u16 = 3; // width of the sync button column " ⇄ "

/// Column widths for one row of the Players screen. Computed once and shared between the
/// renderer (`draw_players`) and the mouse hit-tester (`handle_mouse_event`) so click targets
/// can never drift out of sync with what is drawn.
pub struct PlayersRowLayout {
    /// Width of the trailing " 🔊 NNN%" volume string (icon padded to constant width).
    pub vol_str_w: usize,
    /// Width of the volume bar column (shared by the global row and per-player rows).
    pub bar_w: usize,
    /// Width of the player-name column (also aligns the global "Global vol" label).
    pub name_col_w: usize,
}

/// Derive the shared Players-row column widths from the panel `area` (the outer panel rect, the
/// same value `draw_players` gets and the mouse handler holds as `main_area`). Inner usable
/// width is `area.width - 3` (2 borders + 1 reserved for the pill endcap).
pub fn players_row_layout(area: Rect, use_nerd_icons: bool) -> PlayersRowLayout {
    let vol_icon_w: usize = if use_nerd_icons { 2 } else { 0 };
    let vol_str_w = 1 + vol_icon_w + 4; // 1 space + icon + 3 digits + '%'
    let row_w = (area.width.saturating_sub(3)) as usize;
    let fixed = PLAYERS_PWR_BTN_W as usize + PLAYERS_SYNC_BTN_W as usize + vol_str_w + 1 + 3;
    let flex = row_w.saturating_sub(fixed);
    let bar_w = flex / 2;
    let name_col_w = flex.saturating_sub(bar_w);
    PlayersRowLayout {
        vol_str_w,
        bar_w,
        name_col_w,
    }
}

const PILL_BG_FOCUSED: Color = Color::Rgb(45, 100, 170);
const PILL_FG_FOCUSED: Color = Color::Rgb(220, 235, 255);
const PILL_BG_UNFOCUSED: Color = Color::Rgb(50, 50, 68);
const THUMB_BG_DEFAULT: Color = Color::Rgb(25, 25, 35);
const THUMB_PLACEHOLDER: Color = Color::Rgb(80, 80, 110);
const BAR_FOCUSED: Color = Color::Rgb(100, 180, 255);
const BAR_UNFOCUSED: Color = Color::Rgb(60, 80, 110);

fn accent_to_color(accent: Option<[u8; 3]>) -> Color {
    accent.map(|c| Color::Rgb(c[0], c[1], c[2])).unwrap_or(Color::Yellow)
}

fn styled_block<'a>(title: &'a str, border_style: Style, title_color: Color) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title_style(Style::default().fg(title_color))
        .title(title)
}

fn build_bar_fill(value: u8, width: usize) -> String {
    let filled = (value as usize * width) / 100;
    format!(
        "{}{}",
        "█".repeat(filled),
        "░".repeat(width.saturating_sub(filled))
    )
}

fn sidebar_nerd_icon(item: &SidebarItem) -> &'static str {
    match item {
        SidebarItem::MyMusic => "\u{F001}",    // nf-fa-music
        SidebarItem::Search => "\u{F002}",     // nf-fa-search
        SidebarItem::Radio => "\u{F130}",      // nf-fa-microphone
        SidebarItem::Apps => "\u{F009}",       // nf-fa-th-large
        SidebarItem::Favourites => "\u{F005}", // nf-fa-star
        SidebarItem::Queue => "\u{F03A}",      // nf-fa-list
        SidebarItem::Players => "\u{F028}",    // nf-fa-volume-up
        SidebarItem::Help => "\u{F059}",       // nf-fa-question-circle
    }
}

fn focus_border_color(accent: Option<[u8; 3]>) -> Color {
    match accent {
        Some([r, g, b]) => Color::Rgb(r, g, b),
        None => Color::Yellow,
    }
}

fn accent_tint(
    accent: Option<[u8; 3]>,
    pct: u16,
    r_off: u16,
    g_off: u16,
    b_off: u16,
    fallback: Color,
) -> Color {
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

fn border_style_for_focus(focused: bool, accent: Option<[u8; 3]>) -> Style {
    if focused {
        Style::default().fg(focus_border_color(accent))
    } else {
        Style::default().fg(unfocus_border_color(accent))
    }
}

fn thumbnail_bg_color(is_selected: bool, focused: bool) -> Color {
    if is_selected {
        if focused {
            PILL_BG_FOCUSED
        } else {
            PILL_BG_UNFOCUSED
        }
    } else {
        THUMB_BG_DEFAULT
    }
}

fn render_thumbnail(
    f: &mut Frame,
    thumbnails: &mut HashMap<String, StatefulProtocol>,
    thumb_url: Option<&str>,
    thumb_rect: Rect,
    bg: Color,
) {
    match thumb_url.and_then(|u| thumbnails.get_mut(u)) {
        Some(proto) => {
            let img = StatefulImage::default().resize(Resize::Fit(None));
            f.render_stateful_widget(img, thumb_rect, proto);
        }
        None => {
            f.render_widget(
                Paragraph::new(if thumb_rect.height >= 2 {
                    "\n ♪"
                } else {
                    " ♪"
                })
                .style(Style::default().fg(THUMB_PLACEHOLDER).bg(bg)),
                thumb_rect,
            );
        }
    }
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

fn icon_mute(nerd: bool) -> &'static str {
    if nerd { "\u{F026}" } else { "×" } // nf-fa-volume-off
}

fn icon_vol_or_mute(nerd: bool, vol: u8) -> &'static str {
    if vol == 0 {
        icon_mute(nerd)
    } else {
        icon_vol(nerd)
    }
}

fn icon_power(nerd: bool) -> &'static str {
    if nerd { "\u{F011}" } else { "o" } // nf-fa-power-off
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
        (PILL_BG_FOCUSED, PILL_FG_FOCUSED)
    } else {
        (PILL_BG_UNFOCUSED, Color::Rgb(190, 190, 210))
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
    duration: Option<String>,
}

/// Returns (sidebar_area, main_area) for a given terminal area.
pub fn compute_areas(area: Rect, status_height: u16) -> (Rect, Rect) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(status_height),
            Constraint::Length(1),
        ])
        .split(area);
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(20), Constraint::Min(1)])
        .split(outer[0]);
    (panes[0], panes[1])
}

/// Returns (input_rect, tab_bar_rect) for the search panel within main_area.
pub fn compute_search_panel_rects(main_area: Rect) -> (Rect, Rect) {
    let inner_x = main_area.x + 1;
    let inner_y = main_area.y + 1;
    let inner_w = main_area.width.saturating_sub(2);
    let input_rect = Rect::new(inner_x, inner_y, inner_w, 3);
    let tab_rect = Rect::new(inner_x, inner_y + 3, inner_w, 1);
    (input_rect, tab_rect)
}

/// Returns the SearchScope whose tab was clicked at `col` in the tab bar.
/// Separator clicks select the preceding tab; clicks past the last tab select All.
pub fn search_scope_at_col(col: u16, tab_bar: Rect, use_nerd_icons: bool) -> Option<SearchScope> {
    if col < tab_bar.x {
        return None;
    }
    let rel = (col - tab_bar.x) as usize;
    let labels = ["My Music", "Radios", "Apps", "All"];
    let scopes = [
        SearchScope::MyMusic,
        SearchScope::Radios,
        SearchScope::Apps,
        SearchScope::All,
    ];
    let extra = if use_nerd_icons { 2usize } else { 0 };
    let sep = 3usize; // " │ "
    let mut start = 0usize;
    for (label, scope) in labels.iter().zip(scopes.iter()) {
        let tab_w = extra + label.len();
        if rel < start + tab_w + sep {
            return Some(scope.clone());
        }
        start += tab_w + sep;
    }
    Some(SearchScope::All)
}

/// Returns the six clickable button rects in the Now Playing controls row: [Prev, PlayPause, Stop, Next, Shuffle, Repeat].
pub fn compute_statusbar_control_rects(
    area: Rect,
    status_height: u16,
    art_col_w: u16,
) -> [Rect; 8] {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(status_height),
            Constraint::Length(1),
        ])
        .split(area);
    let status_inner = Rect::new(
        outer[1].x + 1,
        outer[1].y + 1,
        outer[1].width.saturating_sub(2),
        outer[1].height.saturating_sub(2),
    );
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(art_col_w),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
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
            ctrl.x
                + 4 * (btn_w + gap)
                + sep
                + 2 * (btn_w + gap)
                + sep
                + ((i - 6) as u16) * (btn_w + gap)
        };
        Rect::new(x, ctrl.y, btn_w, 1)
    })
}

/// Derives the clickable progress-bar fill rect from the now-playing info area,
/// mirroring the bar math in `draw_now_playing_info` (non-bigscreen). The fill
/// region starts after the 1-col left pill endcap and spans `bar_w` columns.
fn progress_fill_rect(prog: Rect, use_nerd_icons: bool) -> Rect {
    let prog_w = prog.width.saturating_sub(1);
    let endcap_cols: u16 = if use_nerd_icons { 2 } else { 1 };
    let bar_w = prog_w.saturating_sub(endcap_cols);
    Rect::new(prog.x + 1, prog.y, bar_w, prog.height)
}

/// Returns the clickable progress-bar fill rect in the bottom status bar.
pub fn compute_statusbar_progress_rect(
    area: Rect,
    status_height: u16,
    art_col_w: u16,
    app: &App,
) -> Rect {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(status_height),
            Constraint::Length(1),
        ])
        .split(area);
    let status_inner = Rect::new(
        outer[1].x + 1,
        outer[1].y + 1,
        outer[1].width.saturating_sub(2),
        outer[1].height.saturating_sub(2),
    );
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(art_col_w),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
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
    progress_fill_rect(rows[6], app.use_nerd_icons)
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    f: &mut Frame,
    app: &App,
    album_art: Option<&mut StatefulProtocol>,
    album_art_full: Option<&mut StatefulProtocol>,
    sidebar_state: &mut ListState,
    main_state: &mut ListState,
    thumbnails: &mut HashMap<String, StatefulProtocol>,
    server_host: &str,
    server_port: u16,
) {
    let area = f.area();

    if app.full_art_mode {
        draw_full_art_mode(f, app, area, album_art_full, main_state, thumbnails);
    } else {
        // Outer layout: main content | status bar | notification line
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(app.status_height),
                Constraint::Length(1),
            ])
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
            let p = Paragraph::new(msg.as_str()).style(Style::default().fg(Color::Green));
            f.render_widget(p, notif_area);
        } else {
            let footer = if matches!(app.main_view, MainView::Players) {
                hint_line(
                    &[
                        ("t", "power"),
                        ("s", "sync"),
                        ("Spc", "play/pause"),
                        ("n/p", "next/prev"),
                        ("+/-", "vol"),
                        ("`", "art mode"),
                        ("c", "config"),
                        ("q", "quit"),
                    ],
                    app.effective_accent(),
                )
            } else if matches!(app.main_view, MainView::Search) {
                if app.search_input_active {
                    hint_line(
                        &[
                            ("Type", "query"),
                            ("←/→", "cursor"),
                            ("Tab", "scope"),
                            ("Enter", "search"),
                            ("Esc/↓", "results"),
                            ("q", "quit"),
                        ],
                        app.effective_accent(),
                    )
                } else {
                    hint_line(
                        &[
                            ("j/k", "navigate"),
                            ("Enter", "select"),
                            ("Esc", "back"),
                            ("q", "quit"),
                        ],
                        app.effective_accent(),
                    )
                }
            } else {
                hint_line(
                    &[
                        ("a", "add to queue"),
                        ("Spc", "play/pause"),
                        ("n/p", "next/prev"),
                        ("+/-", "vol"),
                        ("`", "art mode"),
                        ("c", "config"),
                        ("q", "quit"),
                    ],
                    app.effective_accent(),
                )
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
        draw_confirm_clear_queue(
            f,
            app.queue.len(),
            app.clear_queue_selected_button,
            app.effective_accent(),
        );
    }

    if app.confirm_quit {
        draw_confirm_quit(f, app.quit_selected_button, app.effective_accent());
    }

    if let Some(idx) = app.confirm_delete_queue_item {
        let queue_len = app.queue.len();
        let track = app.queue.get(idx);
        let title = track.map(|t| t.title.as_str()).unwrap_or("");
        let artist = track.and_then(|t| t.artist.as_deref()).unwrap_or("");
        let album = track.and_then(|t| t.album.as_deref()).unwrap_or("");
        let accent = app.effective_accent();
        draw_confirm_delete_queue_item(
            f, title, artist, album, idx, queue_len,
            app.delete_queue_selected_button, accent,
        );
    }

    if app.context_menu.is_some() {
        draw_context_menu(f, app, area);
    }
}

fn draw_sidebar(f: &mut Frame, app: &App, area: Rect, state: &mut ListState) {
    let border_style = border_style_for_focus(app.focus_sidebar, app.effective_accent());

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
                        Span::styled(
                            format!(" {} ", sidebar_nerd_icon(item)),
                            Style::default()
                                .fg(focus_border_color(app.effective_accent()))
                                .bg(pill_bg),
                        ),
                        Span::styled(
                            format!("{} ", label),
                            Style::default()
                                .fg(pill_fg)
                                .add_modifier(Modifier::BOLD)
                                .bg(pill_bg),
                        ),
                        pill_endcap_right(pill_bg, true),
                    ]))
                } else {
                    ListItem::new(Line::from(vec![
                        pill_endcap_left(pill_bg, false),
                        Span::styled(
                            format!(" {} ", label),
                            Style::default()
                                .fg(pill_fg)
                                .add_modifier(Modifier::BOLD)
                                .bg(pill_bg),
                        ),
                        pill_endcap_right(pill_bg, false),
                    ]))
                }
            } else if app.use_nerd_icons {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("  {} ", sidebar_nerd_icon(item)),
                        Style::default().fg(focus_border_color(app.effective_accent())),
                    ),
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
        render_scrollbar(
            f,
            scroll_area,
            total.saturating_sub(visible),
            offset,
            visible,
            app.effective_accent(),
            app.focus_sidebar,
        );
    }
}

fn draw_main(
    f: &mut Frame,
    app: &App,
    area: Rect,
    state: &mut ListState,
    thumbnails: &mut HashMap<String, StatefulProtocol>,
    base: &str,
) {
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
        MainView::AppSearch { .. } => draw_app_search(f, app, area, state, thumbnails),
    }
}

fn draw_my_music(f: &mut Frame, app: &App, area: Rect, state: &mut ListState) {
    let focused = !app.focus_sidebar;
    let mid = mid_accent_color(app.effective_accent());
    let border_style = border_style_for_focus(focused, app.effective_accent());

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title_style(Style::default().fg(focus_border_color(app.effective_accent())))
        .title(" My Music ");

    let entries: [(&str, &str, &str); 8] = if app.use_nerd_icons {
        [
            ("\u{F0C0}", "Artists", "your music library by artist"),        // nf-fa-users
            ("\u{F2BD}", "Album Artists", "artists with full albums"),       // nf-fa-user_circle
            ("\u{F017}", "Recently Played Artists", "artists you played lately"), // nf-fa-clock-o
            ("\u{EDE9}", "Albums", "all albums"),                           // nf-cod-disc
            ("\u{F3D8}", "Popular Albums", "most recently added albums"),    // nf-fa-fire
            ("\u{F025}", "Tracks", "all tracks"),                           // nf-fa-headphones
            ("\u{F0C9}", "Playlists", "saved playlists"),                   // nf-fa-list
            ("\u{F07B}", "Folders", "browse by folder"),                    // nf-fa-folder
        ]
    } else {
        [
            ("▸", "Artists", "your music library by artist"),
            ("▸", "Album Artists", "artists with full albums"),
            ("▸", "Recently Played Artists", "artists you played lately"),
            ("▸", "Albums", "all albums"),
            ("▸", "Popular Albums", "most recently added albums"),
            ("▸", "Tracks", "all tracks"),
            ("▸", "Playlists", "saved playlists"),
            ("▸", "Folders", "browse by folder"),
        ]
    };

    let (pill_bg, pill_fg) = pill_colors(focused);

    let items: Vec<ListItem> = entries
        .iter()
        .enumerate()
        .map(|(i, (icon, label, sub))| {
            if i == app.main_selected {
                ListItem::new(Line::from(vec![
                    pill_endcap_left(pill_bg, app.use_nerd_icons),
                    Span::styled(
                        format!(" {}  ", icon),
                        Style::default()
                            .fg(focus_border_color(app.effective_accent()))
                            .bg(pill_bg),
                    ),
                    Span::styled(
                        label.to_string(),
                        Style::default()
                            .fg(pill_fg)
                            .add_modifier(Modifier::BOLD)
                            .bg(pill_bg),
                    ),
                    Span::styled(
                        format!("  — {} ", sub),
                        Style::default()
                            .fg(focus_border_color(app.effective_accent()))
                            .bg(pill_bg),
                    ),
                    pill_endcap_right(pill_bg, app.use_nerd_icons),
                ]))
            } else {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("  {}  ", icon),
                        Style::default().fg(focus_border_color(app.effective_accent())),
                    ),
                    Span::raw(label.to_string()),
                    Span::styled(format!("  — {}", sub), Style::default().fg(mid)),
                ]))
            }
        })
        .collect();

    state.select(Some(app.main_selected));

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default())
        .highlight_symbol("");

    f.render_stateful_widget(list, area, state);
}

/// Build the shared two-row item for a track, used by both the Tracks library view and the
/// Queue. `is_current` paints the row green; `thumb_url` is passed in because the Tracks view
/// falls back to a cover-by-id URL while the Queue uses the track's own artwork URL.
fn track_row_item(
    t: &Track,
    index: usize,
    is_current: bool,
    thumb_url: Option<String>,
    mid: Color,
    nerd: bool,
) -> RowItem {
    let (icon_style, title_style, l2_style) = if is_current {
        (
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(Color::Green),
        )
    } else {
        (
            Style::default().fg(mid),
            Style::default().fg(Color::White),
            Style::default().fg(mid),
        )
    };
    let icon = if is_current {
        if nerd { "\u{F04B} " } else { "▶ " }
    } else if nerd {
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
        format!("{}", index + 1)
    } else {
        format!("{}  {}", index + 1, artist_album)
    };
    RowItem {
        thumb_url,
        line1: Line::from(vec![
            Span::styled(icon, icon_style),
            Span::styled(t.title.clone(), title_style),
        ]),
        line2: Line::from(Span::styled(subtitle, l2_style)),
        duration: t.duration.map(format_duration),
    }
}

/// Primary list line `<icon> <name>`: the icon is tinted with the focus accent when nerd
/// icons are enabled, otherwise just the name is shown. Collapses the identical
/// `if app.use_nerd_icons { ... } else { ... }` blocks repeated across the library views.
fn nerd_line(app: &App, icon: &'static str, name: String) -> Line<'static> {
    if app.use_nerd_icons {
        Line::from(vec![
            Span::styled(
                icon,
                Style::default().fg(focus_border_color(app.effective_accent())),
            ),
            Span::raw(name),
        ])
    } else {
        Line::from(Span::raw(name))
    }
}

/// Shared tail for the library views: renders a collected `RowItem` list with the common
/// arguments (selection, focus, accent) so each arm only supplies its title, items and
/// loading flag.
#[allow(clippy::too_many_arguments)]
fn render_two_row_view(
    f: &mut Frame,
    app: &App,
    area: Rect,
    title: &str,
    items: Vec<RowItem>,
    is_loading: bool,
    state: &mut ListState,
    thumbnails: &mut HashMap<String, StatefulProtocol>,
) {
    draw_two_row_list(
        f,
        area,
        title,
        items,
        app.main_selected,
        !app.focus_sidebar,
        is_loading,
        state,
        thumbnails,
        app.effective_accent(),
    );
}

fn draw_library(
    f: &mut Frame,
    app: &App,
    area: Rect,
    view: &LibraryView,
    state: &mut ListState,
    thumbnails: &mut HashMap<String, StatefulProtocol>,
    base: &str,
) {
    let mid = mid_accent_color(app.effective_accent());
    match view {
        LibraryView::Artists => {
            let items: Vec<RowItem> = app
                .artists
                .iter()
                .map(|a| RowItem {
                    thumb_url: crate::utils::artist_artwork_url(app, &a.id),
                    line1: nerd_line(app, "\u{F007} ", a.artist.clone()), // nf-fa-user
                    line2: Line::from(Span::styled("artist", Style::default().fg(mid))),
                    duration: None,
                })
                .collect();
            let title = format!(" Artists ({}) ", items.len());
            render_two_row_view(f, app, area, &title, items, false, state, thumbnails);
        }
        LibraryView::AlbumArtists => {
            let items: Vec<RowItem> = app
                .album_artists
                .iter()
                .map(|a| RowItem {
                    thumb_url: crate::utils::artist_artwork_url(app, &a.id),
                    line1: nerd_line(app, "\u{F2BD} ", a.artist.clone()), // nf-fa-user_circle
                    line2: Line::from(Span::styled("album artist", Style::default().fg(mid))),
                    duration: None,
                })
                .collect();
            let title = format!(" Album Artists ({}) ", items.len());
            render_two_row_view(f, app, area, &title, items, false, state, thumbnails);
        }
        LibraryView::Albums { .. } => {
            let items: Vec<RowItem> = app
                .albums
                .iter()
                .map(|a| {
                    let sub = a.artist.as_deref().unwrap_or("Unknown Artist");
                    RowItem {
                        thumb_url: Some(a.cover_url(base)),
                        line1: nerd_line(app, "\u{EDE9} ", a.album.clone()), // nf-fa-compact_disc
                        line2: Line::from(Span::styled(sub.to_string(), Style::default().fg(mid))),
                        duration: None,
                    }
                })
                .collect();
            let title = format!(" Albums ({}) ", items.len());
            render_two_row_view(f, app, area, &title, items, app.is_loading, state, thumbnails);
        }
        LibraryView::Tracks { album_id } => {
            let label = if album_id.is_some() { "Tracks" } else { "All Tracks" };
            let playing_title = app
                .now_playing
                .as_ref()
                .map(|n| n.title.as_str())
                .unwrap_or("");
            let items: Vec<RowItem> = app
                .tracks
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let is_current = t.title == playing_title && !playing_title.is_empty();
                    let thumb_url = t.artwork_url.clone().or_else(|| {
                        t.id.as_ref().map(|id| {
                            crate::utils::music_image_url(
                                base,
                                crate::utils::json_id_to_string(id),
                                "cover.jpg",
                            )
                        })
                    });
                    track_row_item(t, i, is_current, thumb_url, mid, app.use_nerd_icons)
                })
                .collect();
            let title = format!(" {} ({}) ", label, items.len());
            render_two_row_view(f, app, area, &title, items, app.is_loading, state, thumbnails);
        }
        LibraryView::Folder { .. } => {
            let breadcrumb = breadcrumb_str(
                app.folder_nav_stack.iter().map(|n| n.title.as_str()),
                &app.folder_title,
            );
            let items: Vec<RowItem> = app
                .folder_items
                .iter()
                .map(|item| {
                    let is_track = item.item_type == FolderItemType::Track;
                    let (icon, fg) = if is_track {
                        (
                            if app.use_nerd_icons { "\u{F001} " } else { "▶ " },
                            focus_border_color(app.effective_accent()),
                        )
                    } else {
                        (
                            if app.use_nerd_icons { "\u{F07B} " } else { "▸ " },
                            Color::White,
                        )
                    };
                    RowItem {
                        thumb_url: if is_track {
                            Some(crate::utils::music_image_url(base, item.id, "cover.jpg"))
                        } else {
                            app.folder_artwork.get(&item.id).cloned().flatten()
                        },
                        line1: Line::from(Span::styled(
                            format!("{}{}", icon, item.filename),
                            Style::default().fg(fg),
                        )),
                        line2: Line::from(Span::styled(
                            if is_track {
                                String::new()
                            } else {
                                "folder".to_string()
                            },
                            Style::default().fg(mid),
                        )),
                        duration: if is_track {
                            item.duration.map(format_duration)
                        } else {
                            None
                        },
                    }
                })
                .collect();
            let title = format!(" {} ({}) ", breadcrumb, items.len());
            render_two_row_view(f, app, area, &title, items, app.is_loading, state, thumbnails);
        }
        LibraryView::Playlists => {
            let items: Vec<RowItem> = app
                .playlists
                .iter()
                .map(|p| RowItem {
                    thumb_url: Some(crate::utils::music_image_url(
                        base,
                        crate::utils::json_id_to_string(&p.id),
                        "cover.jpg",
                    )),
                    line1: nerd_line(app, "\u{F0C9} ", p.name.clone()), // nf-fa-list
                    line2: Line::from(Span::styled("playlist", Style::default().fg(mid))),
                    duration: None,
                })
                .collect();
            let title = format!(" Playlists ({}) ", items.len());
            render_two_row_view(f, app, area, &title, items, app.is_loading, state, thumbnails);
        }
        LibraryView::RecentlyPlayedArtists => {
            let items: Vec<RowItem> = app
                .recent_artists
                .iter()
                .map(|a| RowItem {
                    thumb_url: crate::utils::artist_artwork_url(app, &a.id),
                    line1: nerd_line(app, "\u{F007} ", a.artist.clone()), // nf-fa-user
                    line2: Line::from(Span::styled(
                        "recently played",
                        Style::default().fg(mid),
                    )),
                    duration: None,
                })
                .collect();
            let title = format!(" Recently Played Artists ({}) ", items.len());
            render_two_row_view(f, app, area, &title, items, app.is_loading, state, thumbnails);
        }
        LibraryView::PopularAlbums => {
            let items: Vec<RowItem> = app
                .popular_albums
                .iter()
                .map(|a| {
                    let sub = a.artist.as_deref().unwrap_or("Unknown Artist");
                    RowItem {
                        thumb_url: Some(a.cover_url(base)),
                        line1: nerd_line(app, "\u{EDE9} ", a.album.clone()), // nf-fa-compact_disc
                        line2: Line::from(Span::styled(sub.to_string(), Style::default().fg(mid))),
                        duration: None,
                    }
                })
                .collect();
            let title = format!(" Popular Albums ({}) ", items.len());
            render_two_row_view(f, app, area, &title, items, app.is_loading, state, thumbnails);
        }
    }
}

fn draw_queue(
    f: &mut Frame,
    app: &App,
    area: Rect,
    state: &mut ListState,
    thumbnails: &mut HashMap<String, StatefulProtocol>,
) {
    let focused = !app.focus_sidebar;
    let mid = mid_accent_color(app.effective_accent());
    let cur_idx = app.now_playing.as_ref().and_then(|n| n.playlist_cur_index);

    let items: Vec<RowItem> = app
        .queue
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let is_current = cur_idx.map(|idx| idx == i).unwrap_or(false);
            track_row_item(
                t,
                i,
                is_current,
                t.artwork_url.clone(),
                mid,
                app.use_nerd_icons,
            )
        })
        .collect();

    let queue_title = format!(" Queue ({}) ", items.len());
    draw_two_row_list(
        f,
        area,
        &queue_title,
        items,
        app.main_selected,
        focused,
        false,
        state,
        thumbnails,
        app.effective_accent(),
    );
}

fn draw_players(f: &mut Frame, app: &App, area: Rect, state: &mut ListState) {
    let focused = !app.focus_sidebar;
    let accent = app.effective_accent();
    let mid = mid_accent_color(accent);
    let border_style = border_style_for_focus(focused, accent);

    let block = styled_block(" Players ", border_style, focus_border_color(accent));
    let inner = render_bordered_panel(f, block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    let pwr_icon = icon_power(app.use_nerd_icons);

    // Shared layout: compute bar/name widths once via the same helper the mouse handler uses,
    // so global row and per-player rows align and click targets match what's drawn.
    let vol_icon_str: &str = if app.use_nerd_icons { "\u{F028} " } else { "" };
    let mute_icon_str: &str = if app.use_nerd_icons { "\u{F026} " } else { "" };
    let PlayersRowLayout {
        vol_str_w: _,
        bar_w: player_bar_w,
        name_col_w: player_name_col_w,
    } = players_row_layout(area, app.use_nerd_icons);

    let (pill_bg, _pill_fg) = pill_colors(focused);

    // --- Global volume row ---
    let global_avg: u8 = if app.players.is_empty() {
        0
    } else {
        let sum: u32 = app
            .players
            .iter()
            .map(|p| app.player_volumes.get(&p.playerid).copied().unwrap_or(0) as u32)
            .sum();
        (sum / app.players.len() as u32) as u8
    };

    let glob_focused = focused && app.players_focus_global;
    let glob_bg = if glob_focused { PILL_BG_FOCUSED } else { Color::Reset };
    let glob_fg = if glob_focused { PILL_FG_FOCUSED } else { mid };

    let checkbox = if app.global_volume_control {
        "[x]"
    } else {
        "[ ]"
    };
    let glob_icon = if global_avg == 0 {
        mute_icon_str
    } else {
        vol_icon_str
    };
    let vol_str = format!(" {}{:3}%", glob_icon, global_avg);
    let bar = build_bar_fill(global_avg, player_bar_w);

    // Pad the label suffix so the bar starts at the same column as per-player bars.
    // Per-player bar starts at: pwr(3) + label(player_name_col_w+3) + sync(3) = player_name_col_w+9.
    // Global bar must start at: pwr(3) + " "(1) + checkbox(3) + suffix = player_name_col_w+9.
    // So suffix must be player_name_col_w+9-3-1-3 = player_name_col_w+2 chars.
    let glob_suffix_w = player_name_col_w + 2; // " Global vol" = 11 core chars + padding
    let glob_suffix = format!("{:<width$}", " Global vol", width = glob_suffix_w);

    let all_powered = !app.players.is_empty() && app.players.iter().all(|p| p.power > 0);
    let glob_pwr_fg = if all_powered {
        focus_border_color(accent)
    } else {
        btn_dim_color(accent)
    };
    let checkbox_color = if app.global_volume_control {
        focus_border_color(accent)
    } else {
        mid
    };
    let global_line = if glob_focused {
        Line::from(vec![
            pill_endcap_left(pill_bg, app.use_nerd_icons),
            Span::styled(
                format!(" {} ", pwr_icon),
                Style::default().fg(glob_pwr_fg).bg(glob_bg),
            ),
            Span::styled(" ", Style::default().fg(glob_fg).bg(glob_bg)),
            Span::styled(
                checkbox,
                Style::default()
                    .fg(checkbox_color)
                    .bg(glob_bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(glob_suffix, Style::default().fg(glob_fg).bg(glob_bg)),
            Span::styled(bar, Style::default().fg(BAR_FOCUSED).bg(glob_bg)),
            Span::styled(&vol_str, Style::default().fg(Color::White).bg(glob_bg)),
            pill_endcap_right(pill_bg, app.use_nerd_icons),
        ])
    } else {
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!(" {} ", pwr_icon),
                Style::default().fg(glob_pwr_fg).bg(glob_bg),
            ),
            Span::styled(" ", Style::default().fg(glob_fg).bg(glob_bg)),
            Span::styled(
                checkbox,
                Style::default()
                    .fg(checkbox_color)
                    .bg(glob_bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(glob_suffix, Style::default().fg(glob_fg).bg(glob_bg)),
            Span::styled(bar, Style::default().fg(BAR_UNFOCUSED).bg(glob_bg)),
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
        if vis_i >= total {
            break;
        }
        let y = list_area.y + item_i as u16;
        if y >= list_area.y + list_area.height {
            break;
        }

        let p = &app.players[vis_i];
        let active = app.active_player.as_deref() == Some(p.playerid.as_str());
        let powered = p.power > 0;
        let is_sel = focused && !app.players_focus_global && vis_i == app.main_selected;
        let vol = app.player_volumes.get(&p.playerid).copied().unwrap_or(0);

        // Use shared bar/name widths so all rows (global + per-player) are aligned.
        let player_vol_icon = if vol == 0 {
            mute_icon_str
        } else {
            vol_icon_str
        };
        let vol_str = format!(" {}{:3}%", player_vol_icon, vol);
        let bar_w = player_bar_w;
        let name_col_w = player_name_col_w;
        let bar_str = build_bar_fill(vol, bar_w);

        let marker = if active { "● " } else { "○ " };
        let name_raw = format!("{}{}", marker, p.name);

        // Pad/truncate to name_col_w display chars.
        let name_padded = if name_raw.chars().count() > name_col_w {
            let s: String = name_raw
                .chars()
                .take(name_col_w.saturating_sub(1))
                .collect();
            format!("{}…", s)
        } else {
            format!("{:<width$}", name_raw, width = name_col_w)
        };
        let label = format!(" {}  ", name_padded);

        let row_bg = if is_sel { PILL_BG_FOCUSED } else { Color::Reset };
        let name_fg = if is_sel {
            PILL_FG_FOCUSED
        } else if active {
            Color::Green
        } else if powered {
            Color::White
        } else {
            mid
        };
        let bar_color = if is_sel { BAR_FOCUSED } else { BAR_UNFOCUSED };
        let vol_fg = if is_sel { Color::White } else { mid };

        // Power button: accent when on, dim when off (always uses btn_bg_color background).
        let player_pwr_fg = if powered {
            focus_border_color(accent)
        } else {
            btn_dim_color(accent)
        };

        // Sync button — bright accent if player is currently in a sync group, dim otherwise
        let is_synced = app
            .player_sync_groups
            .get(&p.playerid)
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        let sync_fg = if is_synced {
            focus_border_color(accent)
        } else {
            btn_dim_color(accent)
        };

        let line = if is_sel {
            Line::from(vec![
                pill_endcap_left(pill_bg, app.use_nerd_icons),
                Span::styled(
                    format!(" {} ", pwr_icon),
                    Style::default().fg(player_pwr_fg).bg(row_bg),
                ),
                Span::styled(label, Style::default().fg(name_fg).bg(row_bg)),
                Span::styled(" ⇄ ", Style::default().fg(sync_fg).bg(row_bg)),
                Span::styled(bar_str, Style::default().fg(bar_color).bg(row_bg)),
                Span::styled(vol_str, Style::default().fg(vol_fg).bg(row_bg)),
                pill_endcap_right(pill_bg, app.use_nerd_icons),
            ])
        } else {
            Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    format!(" {} ", pwr_icon),
                    Style::default().fg(player_pwr_fg).bg(row_bg),
                ),
                Span::styled(label, Style::default().fg(name_fg).bg(row_bg)),
                Span::styled(" ⇄ ", Style::default().fg(sync_fg).bg(row_bg)),
                Span::styled(bar_str, Style::default().fg(bar_color).bg(row_bg)),
                Span::styled(vol_str, Style::default().fg(vol_fg).bg(row_bg)),
            ])
        };
        f.render_widget(
            Paragraph::new(line),
            Rect::new(list_area.x, y, list_area.width, 1),
        );
    }

    if total > visible {
        let scroll_area = Rect::new(
            area.x + area.width.saturating_sub(1),
            list_area.y,
            1,
            list_area.height,
        );
        let mut ss = ScrollbarState::new(total)
            .position(offset)
            .viewport_content_length(visible);
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

/// Shared renderer for the Radio / Apps / Favourites browse views. They differ only in their
/// data source, breadcrumb title, and whether playable item names are accent-tinted (Radio) or
/// rendered white (Apps/Favourites).
#[allow(clippy::too_many_arguments)]
fn draw_browse_list(
    f: &mut Frame,
    app: &App,
    area: Rect,
    items: &[crate::api::RadioItem],
    title: &str,
    tint_playable_name: bool,
    state: &mut ListState,
    thumbnails: &mut HashMap<String, StatefulProtocol>,
) {
    let focused = !app.focus_sidebar;
    let mid = mid_accent_color(app.effective_accent());
    let accent = app.effective_accent();
    let nerd = app.use_nerd_icons;
    let row_items: Vec<RowItem> = items
        .iter()
        .map(|item| {
            let (icon, icon_fg): (&str, Color) = match (nerd, item.is_playable()) {
                (true, true) => ("\u{F130} ", focus_border_color(accent)), // nf-fa-microphone
                (true, false) => ("\u{F07B} ", Color::White),              // nf-fa-folder
                (false, true) => ("▶ ", focus_border_color(accent)),
                (false, false) => ("▸ ", Color::White),
            };
            let name_fg = if tint_playable_name {
                icon_fg
            } else {
                Color::White
            };
            RowItem {
                thumb_url: item.artwork_url.clone(),
                line1: Line::from(Span::styled(
                    format!("{}{}", icon, item.name),
                    Style::default().fg(name_fg),
                )),
                line2: Line::from(Span::styled(
                    if item.is_playable() {
                        "stream"
                    } else {
                        "folder"
                    },
                    Style::default().fg(mid),
                )),
                duration: None,
            }
        })
        .collect();
    let titled = format!("{} ({}) ", title.trim_end(), row_items.len());
    draw_two_row_list(
        f,
        area,
        &titled,
        row_items,
        app.main_selected,
        focused,
        app.is_loading,
        state,
        thumbnails,
        accent,
    );
}

/// Title bar for a browse view (` Radio `, ` Apps `, ` Favourites `, and their sub-levels):
/// the current level name preceded by the breadcrumb of parent levels. Shared by the three
/// browse wrappers below.
fn browse_title(nav_stack: &[crate::app::RadioNav], current: &str) -> String {
    format!(
        " {} ",
        breadcrumb_str(nav_stack.iter().map(|n| n.title.as_str()), current)
    )
}

fn draw_radio(
    f: &mut Frame,
    app: &App,
    area: Rect,
    state: &mut ListState,
    thumbnails: &mut HashMap<String, StatefulProtocol>,
) {
    let title = browse_title(&app.radio_nav_stack, &app.radio_title);
    draw_browse_list(
        f,
        app,
        area,
        &app.radio_items,
        &title,
        true,
        state,
        thumbnails,
    );
}

fn draw_apps(
    f: &mut Frame,
    app: &App,
    area: Rect,
    state: &mut ListState,
    thumbnails: &mut HashMap<String, StatefulProtocol>,
) {
    let title = browse_title(&app.app_nav_stack, &app.app_title);
    draw_browse_list(
        f,
        app,
        area,
        &app.app_items,
        &title,
        false,
        state,
        thumbnails,
    );
}

fn draw_favourites(
    f: &mut Frame,
    app: &App,
    area: Rect,
    state: &mut ListState,
    thumbnails: &mut HashMap<String, StatefulProtocol>,
) {
    let title = browse_title(&app.fav_nav_stack, &app.fav_title);
    draw_browse_list(
        f,
        app,
        area,
        &app.fav_items,
        &title,
        false,
        state,
        thumbnails,
    );
}

fn draw_search(
    f: &mut Frame,
    app: &App,
    area: Rect,
    state: &mut ListState,
    thumbnails: &mut HashMap<String, StatefulProtocol>,
    base: &str,
) {
    let focused = !app.focus_sidebar;
    let accent = app.effective_accent();
    let mid = mid_accent_color(accent);

    let border_style = border_style_for_focus(focused, accent);
    let search_title = if app.search_results.is_empty() {
        " Search ".to_string()
    } else {
        format!(" Search ({}) ", app.search_results.len())
    };
    let block = styled_block(&search_title, border_style, focus_border_color(accent));
    let inner = render_bordered_panel(f, block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    // Search input box
    let input_border_style = if app.search_input_active {
        Style::default().fg(focus_border_color(accent))
    } else {
        Style::default().fg(unfocus_border_color(accent))
    };
    let search_icon = if app.use_nerd_icons { "\u{F002}" } else { "/" }; // nf-fa-search
    let input_text = format!(" {} {}", search_icon, app.search_query);
    let input = Paragraph::new(input_text)
        .style(Style::default().fg(Color::White))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(input_border_style),
        );
    f.render_widget(input, chunks[0]);
    if app.search_input_active {
        // prefix inside block: left border(1) + " icon "(3) + cursor_pos
        f.set_cursor_position((
            chunks[0].x + 1 + 3 + app.search_cursor_pos as u16,
            chunks[0].y + 1,
        ));
    }

    // Scope tab bar
    {
        let accent = focus_border_color(app.effective_accent());
        let dim = unfocus_border_color(app.effective_accent());
        let tabs: [(&str, &str, SearchScope); 4] = if app.use_nerd_icons {
            [
                ("\u{F001}", "My Music", SearchScope::MyMusic), // nf-fa-music
                ("\u{F130}", "Radios", SearchScope::Radios),    // nf-fa-microphone
                ("\u{F109}", "Apps", SearchScope::Apps),        // nf-fa-laptop
                ("\u{F002}", "All", SearchScope::All),          // nf-fa-search
            ]
        } else {
            [
                ("♪", "My Music", SearchScope::MyMusic),
                ("○", "Radios", SearchScope::Radios),
                ("□", "Apps", SearchScope::Apps),
                ("*", "All", SearchScope::All),
            ]
        };
        let mut spans: Vec<Span> = Vec::new();
        for (i, (icon, label, scope)) in tabs.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" │ ", Style::default().fg(dim)));
            }
            let is_sel = &app.search_scope == scope;
            let style = if is_sel {
                Style::default().fg(accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(dim)
            };
            let text = if app.use_nerd_icons {
                format!("{} {}", icon, label)
            } else {
                label.to_string()
            };
            spans.push(Span::styled(text, style));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), chunks[1]);
    }

    let results_area = chunks[2];

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
            Rect::new(
                results_area.x,
                results_area.y + results_area.height / 2,
                results_area.width,
                1,
            ),
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
        if vis_i >= total {
            break;
        }
        let y = results_area.y + (item_i as u16) * 2;
        if y + 1 >= results_area.y + results_area.height {
            break;
        }

        let is_sel = vis_i == selected;
        let (s1, s2) = if is_sel {
            cursor_styles(results_focused)
        } else {
            (Style::default(), Style::default())
        };

        let thumb_url = match &app.search_results[vis_i] {
            SearchResultItem::Artist(a) => crate::utils::artist_artwork_url(app, &a.id),
            SearchResultItem::Album(alb) => Some(alb.cover_url(base)),
            SearchResultItem::Track(t) => t.id.as_ref().map(|id| {
                crate::utils::music_image_url(
                    base,
                    crate::utils::json_id_to_string(id),
                    "cover.jpg",
                )
            }),
            SearchResultItem::AppItem(item) | SearchResultItem::RadioItem(item) => {
                item.artwork_url.clone()
            }
            SearchResultItem::Playlist(_) => None,
        };
        let thumb_rect = Rect::new(results_area.x, y, THUMB_W, 2);
        let thumb_bg = thumbnail_bg_color(is_sel, results_focused);
        render_thumbnail(f, thumbnails, thumb_url.as_deref(), thumb_rect, thumb_bg);

        let (line1, line2, duration) = match &app.search_results[vis_i] {
            SearchResultItem::Artist(a) => (
                Line::from(vec![
                    Span::styled(
                        if app.use_nerd_icons {
                            "\u{F007} "
                        } else {
                            "▸ "
                        }, // nf-fa-user
                        Style::default().fg(focus_border_color(accent)),
                    ),
                    Span::styled(a.artist.clone(), Style::default().fg(Color::White)),
                ]),
                Line::from(Span::styled("artist", Style::default().fg(mid))),
                None,
            ),
            SearchResultItem::Album(alb) => (
                Line::from(vec![
                    Span::styled(
                        if app.use_nerd_icons {
                            "\u{EDE9} "
                        } else {
                            "▸ "
                        }, // nf-fa-compact_disc
                        Style::default().fg(focus_border_color(accent)),
                    ),
                    Span::styled(alb.album.clone(), Style::default().fg(Color::White)),
                ]),
                Line::from(Span::styled(
                    format!("album  {}", alb.artist.as_deref().unwrap_or("")),
                    Style::default().fg(mid),
                )),
                None,
            ),
            SearchResultItem::Track(t) => (
                Line::from(vec![
                    Span::styled(
                        "▶ ",
                        Style::default().fg(focus_border_color(accent)),
                    ),
                    Span::styled(t.title.clone(), Style::default().fg(Color::White)),
                ]),
                Line::from(Span::styled(
                    t.artist.as_deref().unwrap_or("").to_string(),
                    Style::default().fg(mid),
                )),
                t.duration.map(format_duration),
            ),
            SearchResultItem::Playlist(pl) => (
                Line::from(vec![
                    Span::styled("▸ ", Style::default().fg(Color::Rgb(220, 180, 80))),
                    Span::styled(pl.name.clone(), Style::default().fg(Color::White)),
                ]),
                Line::from(Span::styled("playlist", Style::default().fg(mid))),
                None,
            ),
            SearchResultItem::AppItem(item) => (
                Line::from(vec![
                    Span::styled("▸ ", Style::default().fg(Color::Rgb(180, 120, 220))),
                    Span::styled(item.name.clone(), Style::default().fg(Color::White)),
                ]),
                Line::from(Span::styled("app", Style::default().fg(mid))),
                None,
            ),
            SearchResultItem::RadioItem(item) => (
                Line::from(vec![
                    Span::styled("▸ ", Style::default().fg(Color::Rgb(100, 180, 220))),
                    Span::styled(item.name.clone(), Style::default().fg(Color::White)),
                ]),
                Line::from(Span::styled("radio", Style::default().fg(mid))),
                None,
            ),
        };

        if let Some(dur) = duration.as_deref().filter(|d| !d.is_empty()) {
            let dur_w = dur.len() as u16;
            if dur_w < text_w {
                let title_w = text_w - dur_w;
                f.render_widget(
                    Paragraph::new(line1).style(s1),
                    Rect::new(text_x, y, title_w, 1),
                );
                f.render_widget(
                    Paragraph::new(dur).style(s2),
                    Rect::new(text_x + title_w, y, dur_w, 1),
                );
            } else {
                f.render_widget(
                    Paragraph::new(line1).style(s1),
                    Rect::new(text_x, y, text_w, 1),
                );
            }
        } else {
            f.render_widget(
                Paragraph::new(line1).style(s1),
                Rect::new(text_x, y, text_w, 1),
            );
        }
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
        render_scrollbar(
            f,
            scroll_area,
            total.saturating_sub(visible),
            offset,
            visible,
            app.effective_accent(),
            focused,
        );
    }
}

fn draw_app_search(
    f: &mut Frame,
    app: &App,
    area: Rect,
    state: &mut ListState,
    thumbnails: &mut HashMap<String, StatefulProtocol>,
) {
    let focused = !app.focus_sidebar;
    let raw_accent = app.effective_accent();
    let mid = mid_accent_color(raw_accent);
    let accent = focus_border_color(raw_accent);

    let border_style = if focused {
        Style::default().fg(accent)
    } else {
        Style::default().fg(unfocus_border_color(raw_accent))
    };
    let block = styled_block(" Spotify Search ", border_style, accent);
    let inner = render_bordered_panel(f, block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(inner);

    // Search input box
    let input_border_style = if app.app_search_input_active {
        Style::default().fg(accent)
    } else {
        Style::default().fg(unfocus_border_color(raw_accent))
    };
    let search_icon = if app.use_nerd_icons { "\u{F002}" } else { "/" };
    let input_text = format!(" {} {}", search_icon, app.app_search_query);
    let input = Paragraph::new(input_text)
        .style(Style::default().fg(Color::White))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(input_border_style),
        );
    f.render_widget(input, chunks[0]);
    if app.app_search_input_active {
        f.set_cursor_position((
            chunks[0].x + 1 + 3 + app.app_search_cursor_pos as u16,
            chunks[0].y + 1,
        ));
    }

    let results_area = chunks[1];

    if app.app_search_results.is_empty() {
        let msg = if app.is_loading {
            "Searching..."
        } else if app.app_search_query.is_empty() {
            "Type a query above and press Enter to search"
        } else {
            "No results found"
        };
        f.render_widget(
            Paragraph::new(msg)
                .style(Style::default().fg(mid))
                .alignment(Alignment::Center),
            Rect::new(
                results_area.x,
                results_area.y + results_area.height / 2,
                results_area.width,
                1,
            ),
        );
        return;
    }

    let results_focused = focused && !app.app_search_input_active;
    let selected = app.main_selected;
    let visible = ((results_area.height / 2) as usize).max(1);
    let total = app.app_search_results.len();

    let offset = sync_scroll_offset(state, selected, visible);

    let text_x = results_area.x + THUMB_W + THUMB_SEP;
    let text_w = results_area.width.saturating_sub(THUMB_W + THUMB_SEP);

    for (vis_i, item_i) in (offset..).zip(0usize..) {
        if vis_i >= total {
            break;
        }
        let y = results_area.y + (item_i as u16) * 2;
        if y + 1 >= results_area.y + results_area.height {
            break;
        }

        let item = &app.app_search_results[vis_i];
        let is_sel = vis_i == selected;
        let (s1, s2) = if is_sel {
            cursor_styles(results_focused)
        } else {
            (Style::default(), Style::default())
        };

        let thumb_url = item.artwork_url.clone();
        let thumb_rect = Rect::new(results_area.x, y, THUMB_W, 2);
        let thumb_bg = thumbnail_bg_color(is_sel, results_focused);
        render_thumbnail(f, thumbnails, thumb_url.as_deref(), thumb_rect, thumb_bg);

        // Determine type label and icon from item metadata
        let (icon, type_label, icon_color) = if item.is_audio || item.item_type == "audio" {
            let icon = if app.use_nerd_icons { "  " } else { "▶ " };
            (icon, "song", accent)
        } else if item.item_type == "playlist" {
            let icon = if app.use_nerd_icons { "  " } else { "▸ " };
            (icon, "playlist", Color::Rgb(220, 180, 80))
        } else if item.has_items {
            let icon = if app.use_nerd_icons { "  " } else { "▸ " };
            (icon, "folder", Color::Rgb(100, 200, 180))
        } else {
            let icon = if app.use_nerd_icons { "  " } else { "▸ " };
            (icon, item.item_type.as_str(), mid)
        };

        let line1 = Line::from(vec![
            Span::styled(icon, Style::default().fg(icon_color)),
            Span::styled(item.name.clone(), Style::default().fg(Color::White)),
        ]);
        let line2 = Line::from(Span::styled(type_label, Style::default().fg(mid)));

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
        render_scrollbar(
            f,
            scroll_area,
            total.saturating_sub(visible),
            offset,
            visible,
            raw_accent,
            results_focused,
        );
    }
}

fn draw_help(f: &mut Frame, app: &App, area: Rect) {
    let raw_accent = app.effective_accent();
    let accent = focus_border_color(raw_accent);
    let mid = mid_accent_color(raw_accent);
    let focused = !app.focus_sidebar;
    let border_style = if focused {
        Style::default().fg(accent)
    } else {
        Style::default().fg(unfocus_border_color(raw_accent))
    };
    let block = styled_block(" Keyboard Shortcuts ", border_style, accent);
    let inner = render_bordered_panel(f, block, area);

    let col_w = inner.width / 2;
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(col_w), Constraint::Min(1)])
        .split(inner);

    let header = |s: &'static str| {
        Line::from(Span::styled(
            s,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ))
    };

    let left: Vec<Line> = vec![
        header("Navigation"),
        shortcut("j / ↓", "Move down", mid),
        shortcut("k / ↑", "Move up", mid),
        shortcut("PgDn", "Jump down 10 items", mid),
        shortcut("PgUp", "Jump up 10 items", mid),
        shortcut("Home", "Jump to top", mid),
        shortcut("End", "Jump to bottom", mid),
        shortcut("Enter / l / →", "Select / enter / focus main", mid),
        shortcut("Esc / h / ←", "Back / focus sidebar", mid),
        shortcut("1–8", "Jump to sidebar item directly", mid),
        Line::from(""),
        header("Playback"),
        shortcut("Space", "Play / pause", mid),
        shortcut("n", "Next track", mid),
        shortcut("p", "Previous track", mid),
        shortcut("s", "Toggle shuffle", mid),
        shortcut("r", "Cycle repeat (off → single → queue → ∞)", mid),
        shortcut("+ / =", "Volume up", mid),
        shortcut("-", "Volume down", mid),
    ];

    let right: Vec<Line> = vec![
        header("Library & Queue"),
        shortcut("a", "Add selected item to queue", mid),
        shortcut("d / Del", "Remove selected item from queue", mid),
        shortcut("x", "Clear queue", mid),
        Line::from(""),
        header("Search"),
        shortcut("[ / ]", "Cycle search scope (prev / next)", mid),
        Line::from(""),
        header("Players"),
        shortcut("t", "Toggle player power", mid),
        shortcut("Enter (on Global vol)", "Toggle global volume control", mid),
        Line::from(""),
        header("App"),
        shortcut("`", "Toggle Big Art Mode", mid),
        shortcut("c", "Open server configuration", mid),
        shortcut("q / Ctrl-c", "Quit", mid),
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
        let mut ss = ScrollbarState::new(content_lines as usize)
            .position(scroll as usize)
            .viewport_content_length(visible as usize);
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

/// Builds a styled hint line: keys are rendered as dark-background chips, descriptions beside them.
fn hint_line(pairs: &[(&str, &str)], accent: Option<[u8; 3]>) -> Line<'static> {
    let dim = mid_accent_color(accent);
    let chip_bg = btn_bg_color(accent);
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, (key, action)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", Style::default().fg(dim)));
        }
        spans.push(Span::styled(
            format!(" {key} "),
            Style::default().fg(Color::White).bg(chip_bg),
        ));
        spans.push(Span::styled(format!(" {action}"), Style::default().fg(dim)));
    }
    Line::from(spans)
}

fn draw_statusbar(f: &mut Frame, app: &App, area: Rect, album_art: Option<&mut StatefulProtocol>) {
    let player_name = app
        .active_player
        .as_ref()
        .and_then(|id| app.players.iter().find(|p| &p.playerid == id))
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "Now Playing".to_string());
    let mid = mid_accent_color(app.effective_accent());
    let accent = focus_border_color(app.effective_accent());
    let player_icon = icon_player_dot(app.use_nerd_icons);
    let left_title = Line::from(vec![
        Span::styled(format!(" {} ", player_icon), Style::default().fg(mid)),
        Span::styled(format!("{} ", player_name), Style::default().fg(Color::White)),
    ]);
    let block = if let Some(np) = &app.now_playing {
        let vol_icon = icon_vol_or_mute(app.use_nerd_icons, np.volume);
        let globe = if app.global_volume_control {
            icon_globe(app.use_nerd_icons)
        } else {
            ""
        };
        let right_title = Line::from(vec![
            Span::styled(format!(" {} ", vol_icon), Style::default().fg(mid)),
            Span::styled(format!("{}%", np.volume), Style::default().fg(Color::White)),
            Span::styled(
                if globe.is_empty() { " ".to_string() } else { format!(" {} ", globe) },
                Style::default().fg(accent),
            ),
        ])
        .alignment(Alignment::Right);
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(accent))
            .title(left_title)
            .title(right_title)
    } else {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(accent))
            .title(left_title)
    };
    let inner = render_bordered_panel(f, block, area);

    let Some(np) = &app.now_playing else {
        let msg = Paragraph::new("No player selected — press → then navigate to Players")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(msg, inner);
        return;
    };

    // Compute art column width from actual image dimensions so the image fills the full height.
    let art_col_w = art_rendered_cols(app, Rect::new(inner.x, inner.y, inner.width, inner.height));

    // Split: art column | 1-col gap | info column
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(art_col_w),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);

    if let Some(proto) = album_art {
        let img = StatefulImage::<StatefulProtocol>::default().resize(Resize::Scale(None));
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
    let raw_accent = app.effective_accent();
    let mid = mid_accent_color(raw_accent);

    let play_icon = if app.use_nerd_icons {
        if np.is_playing {
            "\u{F04B}"
        } else {
            "\u{F04C}"
        } // nf-fa-play / nf-fa-pause
    } else if np.is_playing {
        "▶"
    } else {
        "⏸"
    };
    let shuffle_icon = if np.shuffle > 0 {
        if app.use_nerd_icons {
            " \u{F074}"
        } else {
            " ⇌"
        } // nf-fa-random
    } else {
        ""
    };
    let repeat_icon = if app.use_nerd_icons {
        match np.repeat {
            1 => " \u{F01E}1", // repeat single track
            2 => " \u{F01E}",  // repeat queue
            3 => " \u{221E}",  // don't stop the music
            _ => "",
        }
    } else {
        match np.repeat {
            1 => " ↺1", // repeat single track
            2 => " ↺",  // repeat queue
            3 => " ∞",  // don't stop the music
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
        Span::styled(
            np.title.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(queue_pos, Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{}{}", shuffle_icon, repeat_icon),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(Paragraph::new(title_line), rows[0]);

    let accent = focus_border_color(raw_accent);
    let artist_label = if app.use_nerd_icons {
        "  \u{F007} "
    } else {
        "  by "
    }; // nf-fa-user
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
    let album_label = if app.use_nerd_icons {
        "  \u{F025} "
    } else {
        "  from "
    }; // nf-fa-headphones
    let album_line = Line::from(vec![
        Span::raw(indent),
        Span::styled(album_label, Style::default().fg(mid)),
        Span::styled(
            album_text.trim_start().to_string(),
            Style::default().fg(accent),
        ),
    ]);
    f.render_widget(Paragraph::new(album_line), rows[2]);

    if bigscreen {
        let player_name = app
            .active_player
            .as_ref()
            .and_then(|id| app.players.iter().find(|p| &p.playerid == id))
            .map(|p| p.name.as_str())
            .unwrap_or("—");
        let vol_icon = icon_vol_or_mute(app.use_nerd_icons, np.volume);
        let globe_icon = if app.global_volume_control {
            icon_globe(app.use_nerd_icons)
        } else {
            ""
        };
        let player_vol_line = Line::from(vec![
            Span::styled(
                if app.use_nerd_icons {
                    " \u{f075a} "
                } else {
                    " ▶ "
                },
                Style::default().fg(mid),
            ),
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
            if np.is_playing {
                "\u{F04C}"
            } else {
                "\u{F04B}"
            } // nf-fa-pause / nf-fa-play
        } else if np.is_playing {
            "‖"
        } else {
            "▶"
        };
        let prev_icon = if app.use_nerd_icons { "\u{F048}" } else { "«" }; // nf-fa-step-backward
        let stop_icon = if app.use_nerd_icons {
            "\u{F04D}"
        } else {
            "■"
        }; // nf-fa-stop
        let next_icon = if app.use_nerd_icons { "\u{F051}" } else { "»" }; // nf-fa-step-forward
        let btn_w: u16 = 3;
        let gap: u16 = 1;
        let sep: u16 = 2;
        let media_icons = [prev_icon, play_pause_icon, stop_icon, next_icon];
        for (i, icon) in media_icons.iter().enumerate() {
            let x = ctrl_x + (i as u16) * (btn_w + gap);
            if x + btn_w > ctrl_max_x {
                break;
            }
            f.render_widget(
                Paragraph::new(format!(" {} ", icon)).style(
                    Style::default()
                        .fg(focus_border_color(raw_accent))
                        .bg(btn_bg_color(raw_accent)),
                ),
                Rect::new(x, ctrl.y, btn_w, 1),
            );
        }
        let shuf_icon = if app.use_nerd_icons {
            "\u{F074}"
        } else {
            "⇄"
        }; // nf-fa-random
        let shuffle_x = ctrl_x + 4 * (btn_w + gap) + sep;
        if shuffle_x + btn_w <= ctrl_max_x {
            let (sfg, sbg) = if np.shuffle > 0 {
                (
                    focus_border_color(raw_accent),
                    btn_active_bg_color(raw_accent),
                )
            } else {
                (
                    btn_dim_color(raw_accent),
                    btn_bg_color(raw_accent),
                )
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
                    focus_border_color(raw_accent),
                    btn_active_bg_color(raw_accent),
                    if app.use_nerd_icons {
                        " \u{F01E}1".to_string()
                    } else {
                        " ↺1".to_string()
                    },
                ),
                2 => (
                    focus_border_color(raw_accent),
                    btn_active_bg_color(raw_accent),
                    if app.use_nerd_icons {
                        " \u{F01E} ".to_string()
                    } else {
                        " ↺ ".to_string()
                    },
                ),
                3 => (
                    focus_border_color(raw_accent),
                    btn_active_bg_color(raw_accent),
                    " ∞ ".to_string(),
                ),
                _ => (
                    btn_dim_color(raw_accent),
                    btn_bg_color(raw_accent),
                    if app.use_nerd_icons {
                        " \u{F01E} ".to_string()
                    } else {
                        " ↺ ".to_string()
                    },
                ),
            };
            f.render_widget(
                Paragraph::new(rep_btn).style(Style::default().fg(rfg).bg(rbg)),
                Rect::new(repeat_x, ctrl.y, btn_w, 1),
            );
        }
        let vol_down_x = repeat_x + btn_w + sep;
        if vol_down_x + btn_w <= ctrl_max_x {
            let vol_down_icon = if app.use_nerd_icons {
                "\u{F027}"
            } else {
                "−"
            }; // nf-fa-volume-down
            f.render_widget(
                Paragraph::new(format!(" {} ", vol_down_icon)).style(
                    Style::default()
                        .fg(focus_border_color(raw_accent))
                        .bg(btn_bg_color(raw_accent)),
                ),
                Rect::new(vol_down_x, ctrl.y, btn_w, 1),
            );
        }
        let vol_up_x = vol_down_x + btn_w + gap;
        if vol_up_x + btn_w <= ctrl_max_x {
            let vol_up_icon = if app.use_nerd_icons { "\u{F028}" } else { "+" }; // nf-fa-volume-up
            f.render_widget(
                Paragraph::new(format!(" {} ", vol_up_icon)).style(
                    Style::default()
                        .fg(focus_border_color(raw_accent))
                        .bg(btn_bg_color(raw_accent)),
                ),
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
    // Reserve columns for pill endcaps (nerd: left+right = 2 cols; plain: left space = 1 col)
    let endcap_cols: u16 = if app.use_nerd_icons { 2 } else { 1 };
    let bar_w = prog_w.saturating_sub(endcap_cols) as usize;
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

    let text_bytes: Vec<char> = time.chars().collect();
    let over_filled_text: String = text_bytes[..over_filled].iter().collect();
    let over_unfilled_text: String = text_bytes[over_filled..].iter().collect();

    let (accent, track_color) = match raw_accent {
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
    let left_cap_color = if pct > 0 { accent } else { track_color };
    let right_cap_color = if filled >= bar_w { accent } else { track_color };
    let bar = Line::from(vec![
        pill_endcap_left(left_cap_color, app.use_nerd_icons),
        Span::styled(" ".repeat(pure_filled), Style::default().bg(accent)),
        Span::styled(" ".repeat(pure_unfilled), Style::default().bg(track_color)),
        Span::styled(
            over_filled_text,
            Style::default()
                .bg(accent)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            over_unfilled_text,
            Style::default()
                .bg(track_color)
                .fg(Color::Rgb(210, 215, 225)),
        ),
        pill_endcap_right(right_cap_color, app.use_nerd_icons),
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

    let fc = focus_border_color(app.effective_accent());
    let mid = mid_accent_color(app.effective_accent());
    let player_name = app
        .active_player
        .as_ref()
        .and_then(|id| app.players.iter().find(|p| &p.playerid == id))
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "Now Playing".to_string());
    let player_icon = icon_player_dot(app.use_nerd_icons);
    let left_title = Line::from(vec![
        Span::styled(format!(" {} ", player_icon), Style::default().fg(mid)),
        Span::styled(format!("{} ", player_name), Style::default().fg(Color::White)),
    ]);
    let np_block = if let Some(np) = &app.now_playing {
        let vol_icon = icon_vol_or_mute(app.use_nerd_icons, np.volume);
        let globe = if app.global_volume_control {
            icon_globe(app.use_nerd_icons)
        } else {
            ""
        };
        let right_title = Line::from(vec![
            Span::styled(format!(" {} ", vol_icon), Style::default().fg(mid)),
            Span::styled(format!("{}%", np.volume), Style::default().fg(Color::White)),
            Span::styled(
                if globe.is_empty() { " ".to_string() } else { format!(" {} ", globe) },
                Style::default().fg(fc),
            ),
        ])
        .alignment(Alignment::Right);
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(fc))
            .title(left_title)
            .title(right_title)
    } else {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(fc))
            .title(left_title)
    };
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

    let footer = hint_line(
        &[
            ("`", "exit art"),
            ("Spc", "play/pause"),
            ("n/p", "next/prev"),
            ("+/-", "vol"),
            ("c", "config"),
            ("q", "quit"),
        ],
        app.effective_accent(),
    );
    f.render_widget(Paragraph::new(footer), footer_area);
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
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Red)),
        )
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
    let block = styled_block(title, border_style, focus_border_color(accent));

    if items.is_empty() {
        state.select(None);
        let msg = if is_loading {
            "  Loading..."
        } else {
            "(empty)"
        };
        f.render_widget(
            Paragraph::new(msg)
                .block(block)
                .style(Style::default().fg(Color::DarkGray)),
            area,
        );
        return;
    }

    let inner = render_bordered_panel(f, block, area);

    let visible = ((inner.height / 2) as usize).max(1);
    let needs_scroll = items.len() > visible;

    let offset = sync_scroll_offset(state, selected, visible);

    let text_x = inner.x + THUMB_W + THUMB_SEP;
    let text_w = inner.width.saturating_sub(THUMB_W + THUMB_SEP);

    for (vis_i, item_i) in (offset..).zip(0usize..) {
        if vis_i >= items.len() {
            break;
        }
        let y = inner.y + (item_i as u16) * 2;
        if y + 1 >= inner.y + inner.height {
            break;
        }

        let item = &items[vis_i];
        let is_sel = vis_i == selected;

        let (s1, s2) = if is_sel {
            cursor_styles(focused)
        } else {
            (Style::default(), Style::default())
        };

        let thumb_rect = Rect::new(inner.x, y, THUMB_W, 2);
        let thumb_bg = thumbnail_bg_color(is_sel, focused);
        render_thumbnail(
            f,
            thumbnails,
            item.thumb_url.as_deref(),
            thumb_rect,
            thumb_bg,
        );

        if let Some(dur) = item.duration.as_deref().filter(|d| !d.is_empty()) {
            let dur_w = dur.len() as u16;
            if dur_w < text_w {
                let title_w = text_w - dur_w;
                f.render_widget(
                    Paragraph::new(item.line1.clone()).style(s1),
                    Rect::new(text_x, y, title_w, 1),
                );
                f.render_widget(
                    Paragraph::new(dur).style(s2),
                    Rect::new(text_x + title_w, y, dur_w, 1),
                );
            } else {
                f.render_widget(
                    Paragraph::new(item.line1.clone()).style(s1),
                    Rect::new(text_x, y, text_w, 1),
                );
            }
        } else {
            f.render_widget(
                Paragraph::new(item.line1.clone()).style(s1),
                Rect::new(text_x, y, text_w, 1),
            );
        }
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
        render_scrollbar(
            f,
            scroll_area,
            items.len().saturating_sub(visible),
            offset,
            visible,
            accent,
            focused,
        );
    }
}

/// Render a left/right button pair into `row`, highlighting the one matching
/// `selected_button` (0 = left, 1 = right) with the accent background.
fn render_two_button_dialog(
    f: &mut Frame,
    row: Rect,
    selected_button: u8,
    accent_color: Color,
    left_label: &str,
    right_label: &str,
) {
    let btn_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(row);
    let btn_style = |selected: bool| {
        if selected {
            Style::default().fg(Color::Black).bg(accent_color).bold()
        } else {
            Style::default().fg(Color::White)
        }
    };
    f.render_widget(
        Paragraph::new(left_label)
            .alignment(Alignment::Center)
            .style(btn_style(selected_button == 0)),
        btn_cols[0],
    );
    f.render_widget(
        Paragraph::new(right_label)
            .alignment(Alignment::Center)
            .style(btn_style(selected_button == 1)),
        btn_cols[1],
    );
}

/// Render `block` into `area` and return its inner area. Pairs the two-step
/// "compute inner, then render border" dance used by every bordered panel/modal.
fn render_bordered_panel(f: &mut Frame, block: Block<'_>, area: Rect) -> Rect {
    let inner = block.inner(area);
    f.render_widget(block, area);
    inner
}

/// Render a vertical accent-tinted scrollbar into `scroll_area`. `content_len` is the
/// scrollable span (items beyond the visible window), `offset` the current top index, and
/// `visible` the number of items visible at once (used to size the thumb proportionally).
fn render_scrollbar(
    f: &mut Frame,
    scroll_area: Rect,
    content_len: usize,
    offset: usize,
    visible: usize,
    accent: Option<[u8; 3]>,
    focused: bool,
) {
    let mut ss = ScrollbarState::new(content_len + visible)
        .position(offset)
        .viewport_content_length(visible);
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

/// Returns (track_style, thumb_style) for a scrollbar tinted from the accent color.
/// Uses focus/unfocus border color to match the surrounding panel border.
fn scrollbar_accent_styles(accent: Option<[u8; 3]>, focused: bool) -> (Style, Style) {
    let color = if focused {
        focus_border_color(accent)
    } else {
        unfocus_border_color(accent)
    };
    let style = Style::default().fg(color);
    (style, style)
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
            ctrl.x
                + 4 * (btn_w + gap)
                + sep
                + 2 * (btn_w + gap)
                + sep
                + ((i - 6) as u16) * (btn_w + gap)
        };
        Rect::new(x, ctrl.y, btn_w, 1)
    })
}

/// Returns the clickable progress-bar fill rect in big-screen (full art) mode.
pub fn compute_full_art_progress_rect(area: Rect, app: &App) -> Rect {
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
    progress_fill_rect(rows[6], app.use_nerd_icons)
}

/// Returns the rect covering the "`:exit art`" footer hint at the bottom-left of big-screen mode.
pub fn compute_full_art_footer_exit_rect(area: Rect) -> Rect {
    let footer_y = area.y + area.height.saturating_sub(1);
    // "`" (1) + ":exit art" (9) = 10 chars; add 2 for padding
    Rect::new(area.x, footer_y, 12, 1)
}

fn compute_full_art_np_block_rect(area: Rect, app: &App) -> Rect {
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
    let info_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(1)])
        .split(cols[1]);
    info_rows[0]
}

/// Returns the rect covering the player name area in the Now Playing block border (left title).
pub fn compute_full_art_footer_player_rect(area: Rect, app: &App) -> Rect {
    let np_rect = compute_full_art_np_block_rect(area, app);
    let player_icon = icon_player_dot(app.use_nerd_icons);
    let player_name = app
        .active_player
        .as_ref()
        .and_then(|id| app.players.iter().find(|p| &p.playerid == id))
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "Now Playing".to_string());
    // Left title: " {player_icon} {player_name} " — starts after the left corner char
    let w = (1 + player_icon.chars().count() + 1 + player_name.chars().count() + 1) as u16;
    Rect::new(np_rect.x + 1, np_rect.y, w.min(np_rect.width.saturating_sub(2)), 1)
}

/// Returns the rect of the volume icon in the Now Playing block border (right title).
/// Returns None when there is no now-playing track.
pub fn compute_full_art_footer_vol_icon_rect(area: Rect, app: &App) -> Option<Rect> {
    app.now_playing.as_ref()?;
    let np_rect = compute_full_art_np_block_rect(area, app);
    let vol = app.now_playing.as_ref().map(|np| np.volume).unwrap_or(0);
    let vol_icon = icon_vol_or_mute(app.use_nerd_icons, vol);
    let vol_str = format!("{}%", vol);
    let globe = if app.global_volume_control {
        icon_globe(app.use_nerd_icons)
    } else {
        ""
    };
    let globe_part_w = if globe.is_empty() { 1usize } else { 1 + globe.chars().count() + 1 };
    // Right title (right-aligned): " {vol_icon} {vol%}{globe_part}"
    let right_title_w =
        1 + vol_icon.chars().count() + 1 + vol_str.chars().count() + globe_part_w;
    // Title ends at np_rect.x + np_rect.width - 2 (before right corner)
    let right_title_start_x =
        (np_rect.x + np_rect.width).saturating_sub(1 + right_title_w as u16);
    let x = right_title_start_x + 1; // skip leading space
    let w = (vol_icon.chars().count() + 1) as u16; // icon + trailing space
    if x + w > np_rect.x + np_rect.width {
        return None;
    }
    Some(Rect::new(x, np_rect.y, w, 1))
}

/// Returns the rect of the volume icon in the Now Playing statusbar block title (right title).
/// Returns None when there is no now-playing track.
pub fn compute_statusbar_vol_icon_rect(area: Rect, status_height: u16, app: &App) -> Option<Rect> {
    app.now_playing.as_ref()?;
    let title_rect = compute_statusbar_title_area(area, status_height);
    let vol = app.now_playing.as_ref().map(|np| np.volume).unwrap_or(0);
    let vol_icon = icon_vol_or_mute(app.use_nerd_icons, vol);
    let vol_str = format!("{}%", vol);
    let globe = if app.global_volume_control {
        icon_globe(app.use_nerd_icons)
    } else {
        ""
    };
    let globe_part_w = if globe.is_empty() { 1usize } else { 1 + globe.chars().count() + 1 };
    // Right title (right-aligned): " {vol_icon} {vol%}{globe_part}"
    let right_title_w =
        1 + vol_icon.chars().count() + 1 + vol_str.chars().count() + globe_part_w;
    // Title ends at title_rect.x + title_rect.width - 2 (before right corner)
    let right_title_start_x =
        (title_rect.x + title_rect.width).saturating_sub(1 + right_title_w as u16);
    let x = right_title_start_x + 1; // skip leading space
    let w = (vol_icon.chars().count() + 1) as u16; // icon + trailing space
    if x + w > title_rect.x + title_rect.width {
        return None;
    }
    Some(Rect::new(x, title_rect.y, w, 1))
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
        .constraints([
            Constraint::Min(1),
            Constraint::Length(status_height),
            Constraint::Length(1),
        ])
        .split(area);
    Rect::new(outer[1].x, outer[1].y, outer[1].width, 1)
}

/// Returns the album art column rect inside the Now Playing status bar.
pub fn compute_statusbar_art_rect(area: Rect, status_height: u16, art_col_w: u16) -> Rect {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(status_height),
            Constraint::Length(1),
        ])
        .split(area);
    let status_inner = Rect::new(
        outer[1].x + 1,
        outer[1].y + 1,
        outer[1].width.saturating_sub(2),
        outer[1].height.saturating_sub(2),
    );
    Rect::new(
        status_inner.x,
        status_inner.y,
        art_col_w.min(status_inner.width),
        status_inner.height,
    )
}

/// Returns the rect for the song title row in the Now Playing status bar info column.
pub fn compute_statusbar_np_title_rect(area: Rect, status_height: u16, art_col_w: u16) -> Rect {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(status_height),
            Constraint::Length(1),
        ])
        .split(area);
    let status_inner = Rect::new(
        outer[1].x + 1,
        outer[1].y + 1,
        outer[1].width.saturating_sub(2),
        outer[1].height.saturating_sub(2),
    );
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(art_col_w),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(status_inner);
    Rect::new(cols[2].x, cols[2].y, cols[2].width, 1)
}

pub fn compute_context_menu_rect(area: Rect, option_count: usize) -> Rect {
    centered_rect_abs(44, (option_count + 2) as u16, area)
}

fn draw_context_menu(f: &mut Frame, app: &App, area: Rect) {
    let Some(menu) = &app.context_menu else {
        return;
    };

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

    let inner = render_bordered_panel(f, block, popup);

    let options = menu.options();
    let last = options.len() - 1;
    let items: Vec<ListItem> = options
        .iter()
        .enumerate()
        .map(|(i, o)| {
            if i == menu.selected {
                ListItem::new(Line::from(vec![
                    pill_endcap_left(pill_bg, app.use_nerd_icons),
                    Span::styled(
                        format!(" {} ", o),
                        Style::default()
                            .fg(pill_fg)
                            .add_modifier(Modifier::BOLD)
                            .bg(pill_bg),
                    ),
                    pill_endcap_right(pill_bg, app.use_nerd_icons),
                ]))
            } else if i == last {
                ListItem::new(Line::from(Span::styled(
                    format!("  {}", o),
                    Style::default().fg(Color::DarkGray),
                )))
            } else {
                ListItem::new(Line::from(Span::raw(format!("  {}", o))))
            }
        })
        .collect();

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

/// Returns (popup_rect, field_rects) where `field_rects[i]` is the clickable row for config
/// field index `i` (0=host … 9+N=image-protocol, with N discovered-server rows in between).
/// Must mirror `draw_config_modal`'s dynamic layout exactly so click hit-testing lands on the
/// same rows that are rendered. OK/Cancel are returned by `compute_config_modal_button_rects`.
pub fn compute_config_modal_rects(area: Rect, n_servers: usize) -> (Rect, Vec<Rect>) {
    let popup = centered_rect_abs(54, 20 + n_servers as u16, area);
    let inner_x = popup.x + 1;
    let inner_y = popup.y + 1;
    let inner_w = popup.width.saturating_sub(2);
    // Offsets mirror the renderer's constraint rows (top pad=0, divider=5, scan=9):
    //   host..pass = 1..4, nerd=6, auto=7, mask=8, scan=9,
    //   discovered servers = 10..9+N, disable-auto-colors = 10+N, image-protocol = 11+N.
    let row = |off: u16| Rect::new(inner_x, inner_y + off, inner_w, 1);
    let mut rects = vec![
        row(1), // 0 host
        row(2), // 1 port
        row(3), // 2 username
        row(4), // 3 password
        row(5), // 4 auto-discover
        row(6), // 5 broadcast-mask
        row(7), // 6 scan-button
    ];
    for j in 0..n_servers as u16 {
        rects.push(row(8 + j)); // 7+j discovered server
    }
    // [8+N] divider sits here (not clickable)
    rects.push(row(9 + n_servers as u16)); // 7+N nerd-icons
    rects.push(row(10 + n_servers as u16)); // 8+N disable-auto-colors
    rects.push(row(11 + n_servers as u16)); // 9+N image-protocol
    (popup, rects)
}

fn draw_confirm_clear_queue(
    f: &mut Frame,
    queue_len: usize,
    selected_button: u8,
    accent: Option<[u8; 3]>,
) {
    let area = f.area();
    let popup = centered_rect_abs(44, 7, area);

    f.render_widget(Clear, popup);

    let ac = accent_to_color(accent);
    let block = styled_block(" Clear Queue ", Style::default().fg(ac), ac);
    let inner = render_bordered_panel(f, block, popup);

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
    let msg = Paragraph::new(format!(
        "Remove {} {} from the queue?",
        queue_len, song_word
    ))
    .alignment(Alignment::Center)
    .style(Style::default().fg(Color::White));
    f.render_widget(msg, rows[1]);

    render_two_button_dialog(f, rows[3], selected_button, ac, "[ OK ]", "[ Cancel ]");
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

fn draw_confirm_quit(f: &mut Frame, selected_button: u8, accent: Option<[u8; 3]>) {
    let area = f.area();
    let popup = centered_rect_abs(44, 7, area);

    f.render_widget(Clear, popup);

    let ac = accent_to_color(accent);
    let block = styled_block(" Quit lyrtui ", Style::default().fg(ac), ac);
    let inner = render_bordered_panel(f, block, popup);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);

    let msg = Paragraph::new("Close lyrtui?")
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::White));
    f.render_widget(msg, rows[1]);

    render_two_button_dialog(f, rows[3], selected_button, ac, "[ Quit ]", "[ Cancel ]");
}

/// Returns (popup_rect, [quit_button_rect, cancel_button_rect]) for the quit dialog.
/// Same geometry as the clear-queue confirmation.
pub fn compute_quit_button_rects(area: Rect) -> (Rect, [Rect; 2]) {
    compute_clear_queue_button_rects(area)
}

/// Returns the rect covering the " Navigation " title text on the sidebar's top border.
pub fn compute_sidebar_nav_title_rect(area: Rect, status_height: u16) -> Rect {
    let (sidebar_area, _) = compute_areas(area, status_height);
    // ratatui renders a left-aligned block title starting one column in from the corner.
    let title_w = " Navigation ".chars().count() as u16;
    let w = title_w.min(sidebar_area.width.saturating_sub(1));
    Rect::new(sidebar_area.x + 1, sidebar_area.y, w, 1)
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

    let ac = accent_to_color(accent);
    let title = format!(" Sync: {} ", modal.player_name);
    let block = styled_block(&title, Style::default().fg(ac), ac);
    let inner = render_bordered_panel(f, block, popup);

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
        let check_fg = if checked { ac } else { Color::DarkGray };
        let row_bg = if is_sel { PILL_BG_FOCUSED } else { Color::Reset };
        let name_fg = if is_sel { PILL_FG_FOCUSED } else { Color::White };

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
        Style::default().fg(Color::Black).bg(ac).bold()
    } else {
        Style::default().fg(Color::White)
    };
    let cancel_style = if modal.focus_buttons && modal.selected_button == 1 {
        Style::default().fg(Color::Black).bg(ac).bold()
    } else {
        Style::default().fg(Color::White)
    };

    f.render_widget(
        Paragraph::new("[ Synchronize ]")
            .alignment(Alignment::Center)
            .style(sync_style),
        btn_cols[0],
    );
    f.render_widget(
        Paragraph::new("[ Cancel ]")
            .alignment(Alignment::Center)
            .style(cancel_style),
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

#[allow(clippy::too_many_arguments)]
fn draw_confirm_delete_queue_item(
    f: &mut Frame,
    title: &str,
    artist: &str,
    album: &str,
    idx: usize,
    queue_len: usize,
    selected: u8,
    accent: Option<[u8; 3]>,
) {
    let area = f.area();
    let popup = centered_rect_abs(54, 12, area);

    f.render_widget(Clear, popup);

    let ac = accent_to_color(accent);
    let block = styled_block(" Remove from Queue ", Style::default().fg(ac), ac);
    let inner = render_bordered_panel(f, block, popup);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // padding
            Constraint::Length(1), // song title
            Constraint::Length(1), // artist · album
            Constraint::Length(1), // padding
            Constraint::Length(1), // option 0
            Constraint::Length(1), // option 1
            Constraint::Length(1), // option 2
            Constraint::Length(1), // option 3
            Constraint::Length(1), // option 4 (cancel)
            Constraint::Min(0),
        ])
        .split(inner);

    let max_w = inner.width.saturating_sub(2) as usize;

    let display_title = truncate_ellipsis(title, max_w);
    f.render_widget(
        Paragraph::new(display_title)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        rows[1],
    );

    let meta = match (artist, album) {
        ("", "") => String::new(),
        (a, "") => a.to_string(),
        ("", al) => al.to_string(),
        (a, al) => format!("{} · {}", a, al),
    };
    if !meta.is_empty() {
        let display_meta = truncate_ellipsis(&meta, max_w);
        f.render_widget(
            Paragraph::new(display_meta)
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray)),
            rows[2],
        );
    }

    let options = [
        ("Remove this song", true),
        ("Remove songs before this", idx > 0),
        ("Remove songs after this", idx + 1 < queue_len),
        ("Clear entire queue", true),
        ("Cancel", true),
    ];

    for (i, (label, enabled)) in options.iter().enumerate() {
        let is_selected = selected as usize == i;
        let prefix = if is_selected { "▶ " } else { "  " };
        let style = if !enabled {
            Style::default().fg(Color::DarkGray)
        } else if is_selected {
            Style::default().fg(ac).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        f.render_widget(Paragraph::new(format!("{}{}", prefix, label)).style(style), rows[4 + i]);
    }
}

fn truncate_ellipsis(s: &str, max_chars: usize) -> String {
    if s.chars().count() > max_chars {
        let cut: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{}…", cut)
    } else {
        s.to_string()
    }
}

/// Returns (popup_rect, [option_rects; 5]) for click-hit-testing the 5 menu rows.
pub fn compute_delete_queue_button_rects(area: Rect) -> (Rect, [Rect; 5]) {
    let popup = centered_rect_abs(54, 12, area);
    let inner_x = popup.x + 1;
    let inner_y = popup.y + 1;
    let inner_w = popup.width.saturating_sub(2);
    // rows: padding(0), title(1), meta(2), padding(3), opt0(4), opt1(5), opt2(6), opt3(7), opt4(8)
    let rects = [
        Rect::new(inner_x, inner_y + 4, inner_w, 1),
        Rect::new(inner_x, inner_y + 5, inner_w, 1),
        Rect::new(inner_x, inner_y + 6, inner_w, 1),
        Rect::new(inner_x, inner_y + 7, inner_w, 1),
        Rect::new(inner_x, inner_y + 8, inner_w, 1),
    ];
    (popup, rects)
}

fn draw_config_modal(f: &mut Frame, modal: &ConfigModal, accent: Option<[u8; 3]>) {
    use crate::app::FieldKind;

    // Right-align every field label to this width so all colons share one column.
    const CONFIG_LABEL_W: usize = 19; // == "Disable auto colors".len(), the longest label

    let area = f.area();
    let n_servers = modal.discovered_servers.len();
    // +1 for scan button row, +n_servers for server entries, +1 base for header growth
    let popup_height = 20u16 + n_servers as u16;
    let popup = centered_rect_abs(54, popup_height, area);

    f.render_widget(Clear, popup);

    let accent_bright = focus_border_color(accent);
    let accent_mid = mid_accent_color(accent);
    let accent_dim = unfocus_border_color(accent);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent_bright))
        .title_style(Style::default().fg(accent_bright))
        .title(" Configuration ");

    let inner = render_bordered_panel(f, block, popup);

    // Build constraints dynamically:
    // [0] pad | [1] host | [2] port | [3] username | [4] password
    // [5] auto-discover | [6] broadcast-mask | [7] scan-button | [8..7+N] discovered servers
    // [8+N] divider | [9+N] nerd-icons | [10+N] disable-auto-colors | [11+N] image-protocol
    // [12+N] error | [13+N] spacer | [14+N] help/buttons
    let mut constraints = vec![
        Constraint::Length(1), // [0] top pad
        Constraint::Length(1), // [1] host
        Constraint::Length(1), // [2] port
        Constraint::Length(1), // [3] username
        Constraint::Length(1), // [4] password
        Constraint::Length(1), // [5] divider
        Constraint::Length(1), // [6] nerd-icons
        Constraint::Length(1), // [7] auto-discover
        Constraint::Length(1), // [8] broadcast-mask
        Constraint::Length(1), // [9] scan-button
    ];
    for _ in 0..n_servers {
        constraints.push(Constraint::Length(1)); // discovered server entry
    }
    constraints.push(Constraint::Length(1)); // disable-auto-colors
    constraints.push(Constraint::Length(1)); // image-protocol
    constraints.push(Constraint::Length(1)); // error
    constraints.push(Constraint::Min(0)); // spacer
    constraints.push(Constraint::Length(1)); // buttons

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    // Row indices. Group 1 (above divider): host..password, auto-discover, broadcast-mask,
    // scan-button, then the N discovered-server rows. Divider. Group 2: nerd-icons,
    // disable-auto-colors, image-protocol. Then error, flexible spacer, buttons.
    let row_auto = 5usize;
    let row_mask = 6usize;
    let row_scan = 7usize;
    // discovered servers render at rows[8 + i]
    let row_divider = 8 + n_servers;
    let row_nerd = 9 + n_servers;
    let row_colors = 10 + n_servers;
    let row_proto = 11 + n_servers;
    let row_error = 12 + n_servers;
    let row_buttons = 14 + n_servers;

    // Text input fields: (label, value, field_index)
    let pass_masked = "*".repeat(modal.password.len());
    let text_fields: &[(&str, &str, usize)] = &[
        ("Host", &modal.host, 0),
        ("Port", &modal.port, 1),
        ("Username", &modal.username, 2),
        ("Password", &pass_masked, 3),
    ];
    for (i, (label, value, idx)) in text_fields.iter().enumerate() {
        let is_selected = modal.selected_field == *idx;
        let is_editing = is_selected && modal.editing;

        let val_style = if is_editing {
            Style::default().fg(Color::Black).bg(accent_bright)
        } else if is_selected {
            Style::default()
                .fg(accent_bright)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let label_style = if is_selected {
            Style::default().fg(accent_bright)
        } else {
            Style::default().fg(accent_mid)
        };

        let line = Line::from(vec![
            Span::styled(format!("  {:>w$}: ", label, w = CONFIG_LABEL_W), label_style),
            Span::styled(value.to_string(), val_style),
        ]);
        f.render_widget(Paragraph::new(line), rows[i + 1]);
    }

    // Divider (separates connection/discovery settings from display settings)
    f.render_widget(
        Paragraph::new(Span::styled(
            "─".repeat(inner.width as usize),
            Style::default().fg(accent_dim),
        )),
        rows[row_divider],
    );

    // Toggle helper closure
    let render_toggle = |f: &mut Frame, row: Rect, label: &str, checked: bool, selected: bool| {
        let checkbox = if checked { "[x]" } else { "[ ]" };
        let val_style = if selected {
            Style::default()
                .fg(accent_bright)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let lbl_style = if selected {
            Style::default().fg(accent_bright)
        } else {
            Style::default().fg(accent_mid)
        };
        let line = Line::from(vec![
            Span::styled(format!("  {:>w$}: ", label, w = CONFIG_LABEL_W), lbl_style),
            Span::styled(checkbox, val_style),
        ]);
        f.render_widget(Paragraph::new(line), row);
    };

    render_toggle(
        f,
        rows[row_nerd],
        "Nerd icons",
        modal.use_nerd_icons,
        modal.field_kind(modal.selected_field) == FieldKind::ToggleNerd,
    );
    render_toggle(
        f,
        rows[row_auto],
        "Auto discover",
        modal.auto_discover,
        modal.field_kind(modal.selected_field) == FieldKind::ToggleDiscover,
    );

    // Broadcast mask text field
    {
        let is_selected = modal.field_kind(modal.selected_field) == FieldKind::TextMask;
        let is_editing = is_selected && modal.editing;
        let val_style = if is_editing {
            Style::default().fg(Color::Black).bg(accent_bright)
        } else if is_selected {
            Style::default()
                .fg(accent_bright)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let lbl_style = if is_selected {
            Style::default().fg(accent_bright)
        } else {
            Style::default().fg(accent_mid)
        };
        let line = Line::from(vec![
            Span::styled(
                format!("  {:>w$}: ", "Bcast mask", w = CONFIG_LABEL_W),
                lbl_style,
            ),
            Span::styled(modal.broadcast_mask.clone(), val_style),
        ]);
        f.render_widget(Paragraph::new(line), rows[row_mask]);
    }

    // Scan button
    {
        let is_selected = modal.field_kind(modal.selected_field) == FieldKind::ScanButton;
        let no_results =
            modal.scan_attempted && !modal.is_scanning && modal.discovered_servers.is_empty();
        let (btn_text, btn_style) = if modal.is_scanning {
            (
                "[ Scanning... ]",
                Style::default().fg(accent_dim).add_modifier(Modifier::DIM),
            )
        } else if no_results {
            ("[ No servers found ]", Style::default().fg(accent_dim))
        } else if is_selected {
            (
                "[ Scan Servers ]",
                Style::default().fg(Color::Black).bg(accent_bright).bold(),
            )
        } else {
            ("[ Scan Servers ]", Style::default().fg(Color::White))
        };
        f.render_widget(
            Paragraph::new(btn_text)
                .alignment(Alignment::Center)
                .style(btn_style),
            rows[row_scan],
        );
    }

    // Discovered server entries (fields 7..6+N)
    for (i, ip) in modal.discovered_servers.iter().enumerate() {
        let field_idx = 7 + i;
        let is_selected = modal.selected_field == field_idx;
        let (prefix, ip_style, hint) = if is_selected {
            (
                "  ▶ ",
                Style::default()
                    .fg(accent_bright)
                    .add_modifier(Modifier::BOLD),
                "  (Enter to use)",
            )
        } else {
            ("    ", Style::default().fg(Color::White), "")
        };
        let lbl_style = if is_selected {
            Style::default().fg(accent_bright)
        } else {
            Style::default().fg(accent_mid)
        };
        let line = Line::from(vec![
            Span::styled(prefix, lbl_style),
            Span::styled(ip.clone(), ip_style),
            Span::styled(hint, Style::default().fg(accent_dim)),
        ]);
        f.render_widget(Paragraph::new(line), rows[8 + i]);
    }

    render_toggle(
        f,
        rows[row_colors],
        "Disable auto colors",
        modal.disable_auto_colors,
        modal.field_kind(modal.selected_field) == FieldKind::ToggleColors,
    );

    // Image protocol selector: < protocol >
    {
        let is_selected = modal.field_kind(modal.selected_field) == FieldKind::SelectorProtocol;
        let proto_name = IMAGE_PROTOCOLS[modal.image_protocol_idx];
        let val_style = if is_selected {
            Style::default()
                .fg(accent_bright)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let lbl_style = if is_selected {
            Style::default().fg(accent_bright)
        } else {
            Style::default().fg(accent_mid)
        };
        let line = Line::from(vec![
            Span::styled(
                format!("  {:>w$}: ", "Image protocol", w = CONFIG_LABEL_W),
                lbl_style,
            ),
            Span::styled(if is_selected { "< " } else { "  " }, lbl_style),
            Span::styled(proto_name, val_style),
            Span::styled(if is_selected { " >" } else { "  " }, lbl_style),
        ]);
        f.render_widget(Paragraph::new(line), rows[row_proto]);
    }

    if let Some(err) = &modal.error {
        let p = Paragraph::new(err.as_str())
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Red));
        f.render_widget(p, rows[row_error]);
    }

    let btn_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[row_buttons]);

    let ok_style = if modal.field_kind(modal.selected_field) == FieldKind::OkButton {
        Style::default().fg(Color::Black).bg(accent_bright).bold()
    } else {
        Style::default().fg(Color::White)
    };
    let cancel_style = if modal.field_kind(modal.selected_field) == FieldKind::CancelButton {
        Style::default().fg(Color::Black).bg(accent_bright).bold()
    } else {
        Style::default().fg(Color::White)
    };

    f.render_widget(
        Paragraph::new("[ OK ]")
            .alignment(Alignment::Center)
            .style(ok_style),
        btn_cols[0],
    );
    f.render_widget(
        Paragraph::new("[ Cancel ]")
            .alignment(Alignment::Center)
            .style(cancel_style),
        btn_cols[1],
    );

    // Place the terminal's native blinking cursor at the edit position. Every label shares
    // the same prefix width ("  " + CONFIG_LABEL_W + ": "), so the value always starts there.
    if modal.editing {
        let prefix_w = (2 + CONFIG_LABEL_W + 2) as u16;
        let row_y = match modal.selected_field {
            f @ 0..=3 => Some(rows[f + 1].y), // host/port/username/password
            f if modal.field_kind(f) == FieldKind::TextMask => Some(rows[row_mask].y),
            _ => None,
        };
        if let Some(row_y) = row_y {
            f.set_cursor_position((inner.x + prefix_w + modal.cursor_pos as u16, row_y));
        }
    }
}

/// Returns (popup_rect, [ok_button_rect, cancel_button_rect]). The buttons are pushed to the
/// last inner row by the layout's flexible spacer, so their position is `popup.height - 2`
/// regardless of how many discovered-server rows are present.
pub fn compute_config_modal_button_rects(area: Rect, n_servers: usize) -> (Rect, [Rect; 2]) {
    let popup = centered_rect_abs(54, 20 + n_servers as u16, area);
    let inner_x = popup.x + 1;
    let inner_w = popup.width.saturating_sub(2);
    let btn_y = popup.y + popup.height.saturating_sub(2);
    let half_w = inner_w / 2;
    let ok_rect = Rect::new(inner_x, btn_y, half_w, 1);
    let cancel_rect = Rect::new(inner_x + half_w, btn_y, inner_w - half_w, 1);
    (popup, [ok_rect, cancel_rect])
}
