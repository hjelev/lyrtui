use crate::app::{App, ConfigModal, ConnectionState, LibraryView, MainView};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph,
    },
    Frame,
};
use ratatui_image::{Resize, StatefulImage, protocol::StatefulProtocol};
use std::collections::HashMap;

const THUMB_W: u16 = 4; // image column width in cells
const THUMB_SEP: u16 = 1; // gap between image and text

struct RowItem {
    thumb_url: Option<String>,
    line1: Line<'static>,
    line2: Line<'static>,
}

/// Returns (sidebar_area, main_area) for a given terminal area.
pub fn compute_areas(area: Rect) -> (Rect, Rect) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(10), Constraint::Length(1)])
        .split(area);
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(20), Constraint::Min(1)])
        .split(outer[0]);
    (panes[0], panes[1])
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

    // Outer layout: main content | status bar (10 rows) | notification line
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(10), Constraint::Length(1)])
        .split(area);

    let main_area = outer[0];
    let status_area = outer[1];
    let notif_area = outer[2];

    // Split main into sidebar | content
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(20), Constraint::Min(1)])
        .split(main_area);

    // Split sidebar column into navigation (shrinks) + server status (fixed 5 rows)
    let sidebar_split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(5)])
        .split(panes[0]);

    draw_sidebar(f, app, sidebar_split[0], sidebar_state);
    draw_server_status(f, app, sidebar_split[1], server_host, server_port);
    draw_main(f, app, panes[1], main_state, thumbnails);
    draw_statusbar(f, app, status_area, album_art);

    if let Some(msg) = &app.status_message {
        let p = Paragraph::new(msg.as_str())
            .style(Style::default().fg(Color::Green));
        f.render_widget(p, notif_area);
    } else {
        let footer = if matches!(app.main_view, MainView::Players) {
            hint_line(&[
                ("j/k", "navigate"), ("Enter", "select"), ("Esc", "back"),
                ("t", "power"), ("Spc", "play/pause"), ("n/p", "next/prev"),
                ("+/-", "vol"), ("c", "config"), ("q", "quit"),
            ])
        } else {
            hint_line(&[
                ("j/k", "navigate"), ("Enter", "select"), ("Esc", "back"),
                ("a", "add to queue"), ("Spc", "play/pause"), ("n/p", "next/prev"),
                ("+/-", "vol"), ("c", "config"), ("q", "quit"),
            ])
        };
        f.render_widget(Paragraph::new(footer), notif_area);
    }

    if app.connection != ConnectionState::Connected {
        draw_disconnected_overlay(f, area, &app.connection);
    }

    if let Some(modal) = &app.config_modal {
        draw_config_modal(f, modal);
    }

    if app.context_menu.is_some() {
        draw_context_menu(f, app, area);
    }
}

fn draw_server_status(f: &mut Frame, app: &App, area: Rect, server_host: &str, server_port: u16) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" Status ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1)])
        .split(inner);

    // Volume
    let vol = app.now_playing.as_ref().map(|np| np.volume).unwrap_or(0);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Vol ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{vol}%"), Style::default().fg(Color::White)),
        ])),
        rows[0],
    );

    // Connection state
    let (dot, dot_color, label) = match &app.connection {
        ConnectionState::Connected    => ("●", Color::Green,  "Connected"),
        ConnectionState::Disconnected => ("●", Color::Red,    "Offline"),
        ConnectionState::Reconnecting => ("◌", Color::Yellow, "Reconnecting"),
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(dot, Style::default().fg(dot_color)),
            Span::styled(format!(" {label}"), Style::default().fg(Color::DarkGray)),
        ])),
        rows[1],
    );

    // Server address
    f.render_widget(
        Paragraph::new(Span::styled(
            format!("{server_host}:{server_port}"),
            Style::default().fg(Color::DarkGray),
        )),
        rows[2],
    );
}

fn draw_sidebar(f: &mut Frame, app: &App, area: Rect, state: &mut ListState) {
    let border_style = if app.focus_sidebar {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(" Navigation ");

    let items: Vec<ListItem> = app
        .sidebar_items
        .iter()
        .map(|item| ListItem::new(app.sidebar_label(item)))
        .collect();

    state.select(Some(app.sidebar_selected));

    let (hl_style, hl_symbol) = if app.focus_sidebar {
        (Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD), "▶ ")
    } else {
        (Style::default(), "  ")
    };

    let list = List::new(items)
        .block(block)
        .highlight_style(hl_style)
        .highlight_symbol(hl_symbol);

    f.render_stateful_widget(list, area, state);
}

fn draw_main(f: &mut Frame, app: &App, area: Rect, state: &mut ListState, thumbnails: &mut HashMap<String, StatefulProtocol>) {
    match &app.main_view {
        MainView::Library(lib) => draw_library(f, app, area, lib, state, thumbnails),
        MainView::Queue => draw_queue(f, app, area, state, thumbnails),
        MainView::Players => draw_players(f, app, area, state),
        MainView::Radio => draw_radio(f, app, area, state, thumbnails),
        MainView::Apps => draw_apps(f, app, area, state, thumbnails),
        MainView::Favourites => draw_favourites(f, app, area, state, thumbnails),
        MainView::Help => draw_help(f, area),
    }
}

fn draw_library(f: &mut Frame, app: &App, area: Rect, view: &LibraryView, state: &mut ListState, thumbnails: &mut HashMap<String, StatefulProtocol>) {
    let focused = !app.focus_sidebar;
    match view {
        LibraryView::Artists => {
            let items = app.artists.iter().map(|a| RowItem {
                thumb_url: None, // artist thumbnails fetched by main.rs
                line1: Line::from(Span::raw(format!("  {}", a.artist))),
                line2: Line::from(Span::styled("  artist", Style::default().fg(Color::DarkGray))),
            }).collect();
            draw_two_row_list(f, area, " Artists ", items, app.main_selected, focused, state, thumbnails);
        }
        LibraryView::Albums { .. } => {
            let items = app.albums.iter().map(|a| {
                let sub = a.artist.as_deref().unwrap_or("Unknown Artist");
                RowItem {
                    thumb_url: None,
                    line1: Line::from(Span::raw(format!("  {}", a.album))),
                    line2: Line::from(Span::styled(format!("  {}", sub), Style::default().fg(Color::DarkGray))),
                }
            }).collect();
            draw_two_row_list(f, area, " Albums ", items, app.main_selected, focused, state, thumbnails);
        }
        LibraryView::Tracks { album_id } => {
            let title = if album_id.is_some() { " Tracks " } else { " All Tracks " };
            let items = app.tracks.iter().enumerate().map(|(i, t)| {
                let dur = t.duration.map(format_duration).unwrap_or_default();
                let artist = t.artist.as_deref().unwrap_or("");
                RowItem {
                    thumb_url: None,
                    line1: Line::from(Span::raw(format!("  {:>3}. {}", i + 1, t.title))),
                    line2: Line::from(Span::styled(
                        format!("  {}  {}", artist, dur),
                        Style::default().fg(Color::DarkGray),
                    )),
                }
            }).collect();
            draw_two_row_list(f, area, title, items, app.main_selected, focused, state, thumbnails);
        }
    }
}

fn draw_queue(f: &mut Frame, app: &App, area: Rect, state: &mut ListState, thumbnails: &mut HashMap<String, StatefulProtocol>) {
    let focused = !app.focus_sidebar;
    let playing_title = app.now_playing.as_ref().map(|n| n.title.as_str()).unwrap_or("");

    let items = app.queue.iter().enumerate().map(|(i, t)| {
        let is_current = t.title == playing_title && !playing_title.is_empty();
        let (l1_style, l2_style) = if is_current {
            (Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
             Style::default().fg(Color::Green))
        } else {
            (Style::default().fg(Color::White),
             Style::default().fg(Color::DarkGray))
        };
        let marker = if is_current { "▶ " } else { "  " };
        let artist_album = match (t.artist.as_deref(), t.album.as_deref()) {
            (Some(ar), Some(al)) => format!("{} — {}", ar, al),
            (Some(ar), None) => ar.to_string(),
            _ => String::new(),
        };
        RowItem {
            thumb_url: None,
            line1: Line::from(Span::styled(format!("{}{:>3}. {}", marker, i + 1, t.title), l1_style)),
            line2: Line::from(Span::styled(format!("    {}", artist_album), l2_style)),
        }
    }).collect();

    draw_two_row_list(f, area, " Queue ", items, app.main_selected, focused, state, thumbnails);
}

fn draw_players(f: &mut Frame, app: &App, area: Rect, state: &mut ListState) {
    let border_style = if !app.focus_sidebar {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let items: Vec<ListItem> = app
        .players
        .iter()
        .map(|p| {
            let active = app.active_player.as_deref() == Some(p.playerid.as_str());
            let powered = p.power > 0;
            let marker = if active { "● " } else { "○ " };
            let power_tag = if powered { "" } else { " [off]" };
            let style = if active {
                Style::default().fg(Color::Green)
            } else if powered {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            ListItem::new(format!("{}{}{}", marker, p.name, power_tag)).style(style)
        })
        .collect();

    render_list(f, area, " Players  t:power ", items, app.main_selected, !app.focus_sidebar, border_style, state);
}

fn draw_radio(f: &mut Frame, app: &App, area: Rect, state: &mut ListState, thumbnails: &mut HashMap<String, StatefulProtocol>) {
    let focused = !app.focus_sidebar;
    let breadcrumb = breadcrumb_str(app.radio_nav_stack.iter().map(|n| n.title.as_str()), &app.radio_title);
    let title = format!(" Radio — {} ", breadcrumb);
    let items = app.radio_items.iter().map(|item| {
        let (icon, fg) = if item.is_playable() { ("▶ ", Color::Cyan) } else { ("▸ ", Color::White) };
        RowItem {
            thumb_url: item.artwork_url.clone(),
            line1: Line::from(Span::styled(format!("  {}{}", icon, item.name), Style::default().fg(fg))),
            line2: Line::from(Span::styled(
                format!("  {}", if item.is_playable() { "stream" } else { "folder" }),
                Style::default().fg(Color::DarkGray),
            )),
        }
    }).collect();
    draw_two_row_list(f, area, &title, items, app.main_selected, focused, state, thumbnails);
}

fn draw_apps(f: &mut Frame, app: &App, area: Rect, state: &mut ListState, thumbnails: &mut HashMap<String, StatefulProtocol>) {
    let focused = !app.focus_sidebar;
    let breadcrumb = breadcrumb_str(app.app_nav_stack.iter().map(|n| n.title.as_str()), &app.app_title);
    let title = format!(" Apps — {} ", breadcrumb);
    let items = app.app_items.iter().map(|item| {
        let (icon, fg) = if item.is_playable() { ("▶ ", Color::Cyan) } else { ("▸ ", Color::White) };
        RowItem {
            thumb_url: item.artwork_url.clone(),
            line1: Line::from(Span::styled(format!("  {}{}", icon, item.name), Style::default().fg(fg))),
            line2: Line::from(Span::styled(
                format!("  {}", if item.is_playable() { "stream" } else { "folder" }),
                Style::default().fg(Color::DarkGray),
            )),
        }
    }).collect();
    draw_two_row_list(f, area, &title, items, app.main_selected, focused, state, thumbnails);
}

fn draw_favourites(f: &mut Frame, app: &App, area: Rect, state: &mut ListState, thumbnails: &mut HashMap<String, StatefulProtocol>) {
    let focused = !app.focus_sidebar;
    let breadcrumb = breadcrumb_str(app.fav_nav_stack.iter().map(|n| n.title.as_str()), &app.fav_title);
    let title = format!(" ★ {} ", breadcrumb);
    let items = app.fav_items.iter().map(|item| {
        let (icon, fg) = if item.is_playable() { ("▶ ", Color::Cyan) } else { ("▸ ", Color::White) };
        RowItem {
            thumb_url: item.artwork_url.clone(),
            line1: Line::from(Span::styled(format!("  {}{}", icon, item.name), Style::default().fg(fg))),
            line2: Line::from(Span::styled(
                format!("  {}", if item.is_playable() { "stream" } else { "folder" }),
                Style::default().fg(Color::DarkGray),
            )),
        }
    }).collect();
    draw_two_row_list(f, area, &title, items, app.main_selected, focused, state, thumbnails);
}

fn draw_help(f: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" Keyboard Shortcuts ");

    let inner = block.inner(area);
    f.render_widget(block, area);

    let col_w = inner.width / 2;
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(col_w), Constraint::Min(1)])
        .split(inner);

    let left: Vec<Line> = vec![
        Line::from(Span::styled("Navigation", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        shortcut("j / ↓",        "Move down"),
        shortcut("k / ↑",        "Move up"),
        shortcut("Enter / l / →", "Select / enter / focus main"),
        shortcut("Esc / h / ←",  "Back / focus sidebar"),
        Line::from(""),
        Line::from(Span::styled("Playback", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        shortcut("Space",  "Play / pause"),
        shortcut("n",      "Next track"),
        shortcut("p",      "Previous track"),
        shortcut("+ / =",  "Volume up"),
        shortcut("-",      "Volume down"),
    ];

    let right: Vec<Line> = vec![
        Line::from(Span::styled("Library & Queue", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        shortcut("a",  "Add selected item to queue"),
        Line::from(""),
        Line::from(Span::styled("Players", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        shortcut("t",  "Toggle player power"),
        Line::from(""),
        Line::from(Span::styled("App", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        shortcut("c",         "Open server configuration"),
        shortcut("q / Ctrl-c", "Quit"),
    ];

    f.render_widget(Paragraph::new(left), cols[0]);
    f.render_widget(Paragraph::new(right), cols[1]);
}

fn shortcut<'a>(key: &'a str, desc: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("  {:<16}", key), Style::default().fg(Color::White)),
        Span::styled(desc, Style::default().fg(Color::DarkGray)),
    ])
}

/// Builds a styled hint line: keys are White, separators and descriptions are DarkGray.
fn hint_line(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, (key, action)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", Style::default().fg(Color::DarkGray)));
        }
        spans.push(Span::styled(key.to_string(), Style::default().fg(Color::White)));
        spans.push(Span::styled(format!(":{action}"), Style::default().fg(Color::DarkGray)));
    }
    Line::from(spans)
}

fn draw_statusbar(f: &mut Frame, app: &App, area: Rect, album_art: Option<&mut StatefulProtocol>) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" Now Playing ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(np) = &app.now_playing else {
        let msg = Paragraph::new("No player selected — press → then navigate to Players")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(msg, inner);
        return;
    };

    // Split: art column (18 cols) | info column
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(18), Constraint::Min(1)])
        .split(inner);

    if let Some(proto) = album_art {
        let img = StatefulImage::<StatefulProtocol>::default().resize(Resize::Fit(None));
        f.render_stateful_widget(img, cols[0], proto);
    }

    // Info panel: title / artist / album / [spacer] / progress / time
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title
            Constraint::Length(1), // artist
            Constraint::Length(1), // album
            Constraint::Min(0),    // filler
            Constraint::Length(1), // progress gauge
            Constraint::Length(1), // time + volume
        ])
        .split(cols[1]);

    let play_icon = if np.is_playing { "▶" } else { "⏸" };
    let shuffle_icon = if np.shuffle > 0 { " ⇌" } else { "" };
    let repeat_icon = match np.repeat {
        1 => " ↺",
        2 => " ↺1",
        _ => "",
    };

    let title_line = Line::from(vec![
        Span::styled(format!("{} ", play_icon), Style::default().fg(Color::Green)),
        Span::styled(np.title.clone(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{}{}", shuffle_icon, repeat_icon), Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(title_line), rows[0]);

    let artist_line = Line::from(Span::styled(
        format!("  {}", np.artist),
        Style::default().fg(Color::Gray),
    ));
    f.render_widget(Paragraph::new(artist_line), rows[1]);

    let album_line = Line::from(Span::styled(
        format!("  {}", np.album),
        Style::default().fg(Color::DarkGray),
    ));
    f.render_widget(Paragraph::new(album_line), rows[2]);

    let pct = if np.duration > 0.0 {
        ((np.elapsed / np.duration) * 100.0) as u16
    } else {
        0
    };
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(Color::Yellow))
        .percent(pct.min(100));
    f.render_widget(gauge, rows[4]);

    let time = format!(
        "{} / {}  vol:{}",
        format_duration(np.elapsed),
        format_duration(np.duration),
        np.volume
    );
    f.render_widget(
        Paragraph::new(time)
            .alignment(Alignment::Right)
            .style(Style::default().fg(Color::DarkGray)),
        rows[5],
    );
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

fn draw_thumb_placeholder(f: &mut Frame, rect: Rect) {
    f.render_widget(
        Paragraph::new(if rect.height >= 2 { "\n ♪" } else { " ♪" })
            .style(Style::default().fg(Color::Rgb(60, 60, 80)).bg(Color::Rgb(25, 25, 35))),
        rect,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_two_row_list(
    f: &mut Frame,
    area: Rect,
    title: &str,
    items: Vec<RowItem>,
    selected: usize,
    focused: bool,
    state: &mut ListState,
    thumbnails: &mut HashMap<String, StatefulProtocol>,
) {
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(title);

    if items.is_empty() {
        state.select(None);
        f.render_widget(
            Paragraph::new("(empty)").block(block).style(Style::default().fg(Color::DarkGray)),
            area,
        );
        return;
    }

    let inner = block.inner(area);
    f.render_widget(block, area);

    let visible = ((inner.height / 2) as usize).max(1);

    // Sync scroll so selected is always visible
    let offset = {
        let o = *state.offset_mut();
        let new_o = if selected < o {
            selected
        } else if selected >= o + visible {
            selected + 1 - visible
        } else {
            o
        };
        *state.offset_mut() = new_o;
        new_o
    };

    let text_x = inner.x + THUMB_W + THUMB_SEP;
    let text_w = inner.width.saturating_sub(THUMB_W + THUMB_SEP);

    for (vis_i, item_i) in (offset..).zip(0usize..) {
        if vis_i >= items.len() { break; }
        let y = inner.y + (item_i as u16) * 2;
        if y + 1 >= inner.y + inner.height { break; }

        let item = &items[vis_i];
        let is_sel = vis_i == selected;

        // Thumbnail
        let thumb_rect = Rect::new(inner.x, y, THUMB_W, 2);
        match item.thumb_url.as_ref().and_then(|u| thumbnails.get_mut(u)) {
            Some(proto) => {
                let img = StatefulImage::default().resize(Resize::Fit(None));
                f.render_stateful_widget(img, thumb_rect, proto);
            }
            None => draw_thumb_placeholder(f, thumb_rect),
        }

        // Text: two lines
        let (fg1, fg2) = if is_sel && focused {
            (Color::Yellow, Color::DarkGray)
        } else if is_sel {
            (Color::White, Color::Gray)
        } else {
            (Color::White, Color::DarkGray)
        };
        let mod1 = if is_sel && focused { Modifier::BOLD } else { Modifier::empty() };

        // Prefix the first line with a selection arrow
        let line1 = if is_sel && focused {
            let mut spans = vec![Span::styled("▶ ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))];
            // strip leading "  " from item line1 if present
            for span in item.line1.spans.iter() {
                let content = span.content.trim_start_matches("  ");
                spans.push(Span::styled(content.to_string(), span.style.fg(fg1).add_modifier(mod1)));
            }
            Line::from(spans)
        } else {
            item.line1.clone().patch_style(Style::default().fg(fg1))
        };

        f.render_widget(
            Paragraph::new(line1),
            Rect::new(text_x, y, text_w, 1),
        );
        f.render_widget(
            Paragraph::new(item.line2.clone().patch_style(Style::default().fg(fg2))),
            Rect::new(text_x, y + 1, text_w, 1),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_list(
    f: &mut Frame,
    area: Rect,
    title: &str,
    items: Vec<ListItem>,
    selected: usize,
    focused: bool,
    border_style: Style,
    state: &mut ListState,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(title);

    if items.is_empty() {
        // Reset scroll when empty so offset doesn't stick from a previous view
        state.select(None);
        let p = Paragraph::new("(empty)")
            .block(block)
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(p, area);
        return;
    }

    state.select(Some(selected));

    let (hl_style, hl_symbol) = if focused {
        (Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD), "▶ ")
    } else {
        (Style::default(), "  ")
    };

    let list = List::new(items)
        .block(block)
        .highlight_style(hl_style)
        .highlight_symbol(hl_symbol);

    f.render_stateful_widget(list, area, state);
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

pub fn compute_context_menu_rect(area: Rect, option_count: usize) -> Rect {
    centered_rect_abs(44, (option_count + 3) as u16, area)
}

fn draw_context_menu(f: &mut Frame, app: &App, area: Rect) {
    let Some(menu) = &app.context_menu else { return };

    let popup = compute_context_menu_rect(area, menu.option_count());
    f.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" What do you want to do? ");

    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let options = menu.options();
    let items: Vec<ListItem> = options.iter().map(|o| ListItem::new(o.as_str())).collect();

    let mut state = ListState::default();
    state.select(Some(menu.selected));

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list, rows[0], &mut state);

    let hint = Paragraph::new(hint_line(&[("↑/↓", "move"), ("Enter", "confirm"), ("Esc", "cancel")]));
    f.render_widget(hint, rows[1]);
}

fn centered_rect_abs(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width.min(area.width), height.min(area.height))
}

fn draw_config_modal(f: &mut Frame, modal: &ConfigModal) {
    let area = f.area();
    let popup = centered_rect_abs(54, 10, area);

    f.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Server Configuration ");

    let inner = block.inner(popup);
    f.render_widget(block, popup);

    // rows: top-pad | host | port | error | spacer | help
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);

    let fields: &[(&str, &str, usize)] = &[("Host", &modal.host, 0), ("Port", &modal.port, 1)];
    for (i, (label, value, idx)) in fields.iter().enumerate() {
        let is_selected = modal.selected_field == *idx;
        let is_editing = is_selected && modal.editing;

        let cursor = if is_editing { "█" } else { "" };
        let display = format!("{}{}", value, cursor);

        let val_style = if is_editing {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else if is_selected {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let line = Line::from(vec![
            Span::styled(format!("  {:>4}: ", label), Style::default().fg(Color::DarkGray)),
            Span::styled(display, val_style),
        ]);
        f.render_widget(Paragraph::new(line), rows[i + 1]);
    }

    if let Some(err) = &modal.error {
        let p = Paragraph::new(err.as_str())
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Red));
        f.render_widget(p, rows[3]);
    }

    let help = Paragraph::new(hint_line(&[
        ("Enter/i", "edit"), ("j/k", "switch field"), ("s", "save"), ("Esc", "close"),
    ]));
    f.render_widget(help, rows[5]);
}
