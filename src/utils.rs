use crate::api::FolderItemType;
use crate::app::{App, LibraryView, MainView, MyMusicEntry, SearchResultItem};
use ratatui::widgets::ListState;

/// Extracts a string id from an `Option<&Value>`, falling back to an empty string for `None`/`Null`.
pub fn extract_id(id: Option<&serde_json::Value>) -> String {
    json_id_to_string(id.unwrap_or(&serde_json::Value::Null))
}

pub fn json_id_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Extract a numeric/string LMS id as `Some(String)`, returning `None` for null/absent
/// values. Numbers (including floats) are normalized to integer strings.
pub fn json_id_to_opt_string(v: &serde_json::Value) -> Option<String> {
    v.as_str()
        .map(String::from)
        .or_else(|| v.as_u64().map(|n| n.to_string()))
        .or_else(|| v.as_i64().map(|n| n.to_string()))
        .or_else(|| v.as_f64().map(|f| (f as i64).to_string()))
}

/// Build an LMS artwork URL of the form `{base}/music/{id}/{file}`, e.g.
/// `music_image_url(base, id, "cover.jpg")` or `music_image_url(base, id, "artist.jpg")`.
/// `id` accepts anything `Display` (a stringified JSON id, a `u32`, etc.).
pub fn music_image_url(base: &str, id: impl std::fmt::Display, file: &str) -> String {
    format!("{}/music/{}/{}", base, id, file)
}

/// Resolved cover URL for an artist from the lazy [`App::artist_artwork`] cache. Returns `None`
/// when the artist is unresolved or known to have no art — the prefetch loop kicks off resolution
/// (see `resolve_artist_artwork` in main.rs) and a later frame will have the URL.
pub fn artist_artwork_url(app: &App, artist_id: &serde_json::Value) -> Option<String> {
    app.artist_artwork
        .get(&json_id_to_string(artist_id))
        .cloned()
        .flatten()
}

pub fn thumbnail_url_for(app: &App, idx: usize, base: &str) -> Option<String> {
    if app.full_art_mode {
        return app.queue.get(idx).and_then(|t| t.artwork_url.clone());
    }
    if let Some(artists) = app.view_artists() {
        return artists.get(idx).and_then(|a| artist_artwork_url(app, &a.id));
    }
    if let Some(albums) = app.view_albums() {
        return albums.get(idx).map(|a| a.cover_url(base));
    }
    match &app.main_view {
        MainView::Library(LibraryView::Tracks { .. }) => app.tracks.get(idx).and_then(|t| {
            t.id.as_ref()
                .map(|id| music_image_url(base, json_id_to_string(id), "cover.jpg"))
        }),
        MainView::Library(LibraryView::Playlists) => app
            .playlists
            .get(idx)
            .map(|p| music_image_url(base, json_id_to_string(&p.id), "cover.jpg")),
        MainView::Library(LibraryView::Folder { .. }) => {
            app.folder_items.get(idx).and_then(|item| {
                if item.item_type == FolderItemType::Track {
                    // A track id resolves directly to its embedded/folder art.
                    Some(music_image_url(base, item.id, "cover.jpg"))
                } else {
                    // Directory rows have no art of their own; use the lazily resolved cover.
                    app.folder_artwork.get(&item.id).cloned().flatten()
                }
            })
        }
        MainView::Queue => app.queue.get(idx).and_then(|t| t.artwork_url.clone()),
        MainView::Radio => app.radio_items.get(idx).and_then(|i| i.artwork_url.clone()),
        MainView::Apps => app.app_items.get(idx).and_then(|i| i.artwork_url.clone()),
        MainView::AppSearch { .. } => app
            .app_search_results
            .get(idx)
            .and_then(|i| i.artwork_url.clone()),
        MainView::Favourites => app.fav_items.get(idx).and_then(|i| i.artwork_url.clone()),
        MainView::Search => match app.search_results.get(idx) {
            Some(SearchResultItem::Artist(a)) => artist_artwork_url(app, &a.id),
            Some(SearchResultItem::Album(alb)) => Some(alb.cover_url(base)),
            Some(SearchResultItem::Track(t)) => {
                t.id.as_ref()
                    .map(|id| music_image_url(base, json_id_to_string(id), "cover.jpg"))
            }
            Some(SearchResultItem::AppItem(item)) => item.artwork_url.clone(),
            _ => None,
        },
        _ => None,
    }
}

/// The artist id at `idx` for views that list artists (so the prefetch loop can lazily resolve
/// their cover art). Returns `None` for non-artist views or out-of-range indices.
pub fn artist_id_at(app: &App, idx: usize) -> Option<String> {
    if let Some(artists) = app.view_artists() {
        return artists.get(idx).map(|a| json_id_to_string(&a.id));
    }
    match (&app.main_view, app.search_results.get(idx)) {
        (MainView::Search, Some(SearchResultItem::Artist(a))) => Some(json_id_to_string(&a.id)),
        _ => None,
    }
}

/// The folder id at `idx` for directory (non-track) rows in the Folders view, so the prefetch
/// loop can lazily resolve their cover art. Returns `None` for tracks and non-folder views.
pub fn folder_id_at(app: &App, idx: usize) -> Option<u32> {
    match &app.main_view {
        MainView::Library(LibraryView::Folder { .. }) => app
            .folder_items
            .get(idx)
            .filter(|item| item.item_type != FolderItemType::Track)
            .map(|item| item.id),
        _ => None,
    }
}

fn queue_labels(name: &str, folder: bool) -> (Option<String>, Option<String>) {
    if folder {
        (
            Some(format!("Add \"{}\" folder to queue", name)),
            Some(format!("Replace queue with \"{}\" folder", name)),
        )
    } else {
        (
            Some(format!("Add \"{}\" to queue", name)),
            Some(format!("Replace queue with \"{}\"", name)),
        )
    }
}

pub fn compute_parent_labels(app: &App) -> (Option<String>, Option<String>) {
    match &app.main_view {
        MainView::Search | MainView::AppSearch { .. } => (None, None),
        MainView::Library(LibraryView::Tracks { album_id: Some(id) }) => {
            let name = app
                .albums
                .iter()
                .find(|a| json_id_to_string(&a.id) == *id)
                .map(|a| a.album.clone())
                .unwrap_or_else(|| "album".to_string());
            queue_labels(&name, false)
        }
        MainView::Radio if !app.radio_items.is_empty() => {
            queue_labels(&app.radio_title, true)
        }
        MainView::Apps if !app.app_items.is_empty() => {
            queue_labels(&app.app_title, true)
        }
        MainView::Favourites if !app.fav_items.is_empty() => {
            queue_labels(&app.fav_title, true)
        }
        MainView::Library(LibraryView::Folder { folder_id: Some(_) }) => {
            queue_labels(&app.folder_title, false)
        }
        _ => (None, None),
    }
}

pub fn uses_two_row_layout(view: &MainView) -> bool {
    !matches!(
        view,
        MainView::Players | MainView::Help | MainView::MyMusic | MainView::AppSearch { .. }
    )
}

pub fn is_main_item_playable(app: &App) -> bool {
    match &app.main_view {
        MainView::Library(LibraryView::Tracks { .. }) => !app.tracks.is_empty(),
        MainView::Library(LibraryView::Folder { .. }) => app
            .folder_items
            .get(app.main_selected)
            .map(|i| i.item_type == FolderItemType::Track)
            .unwrap_or(false),
        MainView::Radio => app
            .radio_items
            .get(app.main_selected)
            .map(|i| i.is_playable() && !i.is_navigable())
            .unwrap_or(false),
        MainView::Apps => app
            .app_items
            .get(app.main_selected)
            .map(|i| i.is_playable() && !i.is_navigable())
            .unwrap_or(false),
        MainView::Favourites => app
            .fav_items
            .get(app.main_selected)
            .map(|i| i.is_playable() && !i.is_navigable())
            .unwrap_or(false),
        MainView::Search => app
            .search_results
            .get(app.main_selected)
            .map(|r| {
                matches!(
                    r,
                    SearchResultItem::Track(_) | SearchResultItem::Playlist(_)
                )
            })
            .unwrap_or(false),
        MainView::AppSearch { .. } => app
            .app_search_results
            .get(app.main_selected)
            .map(|i| i.is_playable())
            .unwrap_or(false),
        _ => false,
    }
}

pub fn thumb_range(term_h: u16, state: &ListState, app: &App) -> std::ops::Range<usize> {
    let inner_h = term_h.saturating_sub(13);
    let visible = ((inner_h / 2) as usize).max(1);
    let offset = state.offset();
    let end = (offset + visible + 5).min(main_list_len(app));
    offset..end
}

pub fn has_overlay(app: &App) -> bool {
    app.confirm_delete_queue_item.is_some()
        || app.confirm_clear_queue
        || app.confirm_quit
        || app.config_modal.is_some()
        || app.context_menu.is_some()
        || app.sync_modal.is_some()
}

pub fn update_status_height(app: &mut App, term_height: u16, base_height: u16) {
    let fw = app.font_size.0.max(1) as u32;
    let fh = app.font_size.1.max(1) as u32;
    let dyn_sh = (term_height / 3).max(base_height);
    app.status_height = dyn_sh;
    let inner_h = dyn_sh.saturating_sub(2);
    app.art_col_w = ((inner_h as u32 * fh) / fw).max(4) as u16;
}

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let r = r as f32 / 255.0;
    let g = g as f32 / 255.0;
    let b = b as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < 1e-6 {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if (max - r).abs() < 1e-6 {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if (max - g).abs() < 1e-6 {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    if s < 1e-6 {
        let v = (l * 255.0).round() as u8;
        return (v, v, v);
    }
    fn hue_to_rgb(p: f32, q: f32, mut t: f32) -> f32 {
        if t < 0.0 { t += 1.0; }
        if t > 1.0 { t -= 1.0; }
        if t < 1.0 / 6.0 { return p + (q - p) * 6.0 * t; }
        if t < 0.5 { return q; }
        if t < 2.0 / 3.0 { return p + (q - p) * (2.0 / 3.0 - t) * 6.0; }
        p
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let r = (hue_to_rgb(p, q, h + 1.0 / 3.0) * 255.0).round() as u8;
    let g = (hue_to_rgb(p, q, h) * 255.0).round() as u8;
    let b = (hue_to_rgb(p, q, h - 1.0 / 3.0) * 255.0).round() as u8;
    (r, g, b)
}

/// Normalize an accent color to the given target lightness (0–100), preserving hue and
/// saturation. Desaturated colors are given a minimum saturation of 0.15 so they don't
/// become indistinguishable gray blobs at any lightness.
pub fn normalize_accent_lightness(color: [u8; 3], target_l: u8) -> [u8; 3] {
    let (h, s, _) = rgb_to_hsl(color[0], color[1], color[2]);
    let s = s.max(0.15);
    let l = (target_l as f32 / 100.0).clamp(0.0, 1.0);
    let (r, g, b) = hsl_to_rgb(h, s, l);
    [r, g, b]
}

pub fn main_list_len(app: &App) -> usize {
    if app.full_art_mode {
        return app.queue.len();
    }
    match &app.main_view {
        MainView::MyMusic => MyMusicEntry::ALL.len(),
        MainView::Library(
            LibraryView::Artists | LibraryView::AlbumArtists | LibraryView::RecentlyPlayedArtists,
        ) => app.view_artists().map_or(0, <[_]>::len),
        MainView::Library(LibraryView::Albums { .. } | LibraryView::NewMusic) => {
            app.view_albums().map_or(0, <[_]>::len)
        }
        MainView::Library(LibraryView::Tracks { .. }) => app.tracks.len(),
        MainView::Library(LibraryView::Folder { .. }) => app.folder_items.len(),
        MainView::Library(LibraryView::Playlists) => app.playlists.len(),
        MainView::Queue => app.queue.len(),
        MainView::Players => app.players.len(),
        MainView::Radio => app.radio_items.len(),
        MainView::Apps => app.app_items.len(),
        MainView::Favourites => app.fav_items.len(),
        MainView::Help => 0,
        MainView::Search => app.search_results.len(),
        MainView::AppSearch { .. } => app.app_search_results.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::FolderItem;
    use serde_json::json;

    fn folder(id: u32, t: FolderItemType) -> FolderItem {
        FolderItem { id, filename: format!("f{id}"), item_type: t, duration: None, url: None }
    }

    #[test]
    fn json_ids_normalize() {
        assert_eq!(json_id_to_string(&json!("ab")), "ab");
        assert_eq!(json_id_to_string(&json!(12)), "12");
        assert_eq!(json_id_to_opt_string(&json!(3.0)), Some("3".into()));
        assert_eq!(json_id_to_opt_string(&json!(null)), None);
        assert_eq!(extract_id(None), "null");
    }

    #[test]
    fn music_image_url_format() {
        let url = music_image_url("http://h:9000", 42, "cover.jpg");
        assert_eq!(url, "http://h:9000/music/42/cover.jpg");
    }

    #[test]
    fn folder_id_only_for_non_track_rows() {
        let mut app = App::new(None);
        app.main_view = MainView::Library(LibraryView::Folder { folder_id: None });
        app.folder_items =
            vec![folder(1, FolderItemType::Folder), folder(2, FolderItemType::Track)];
        assert_eq!(folder_id_at(&app, 0), Some(1));
        assert_eq!(folder_id_at(&app, 1), None);
        assert_eq!(folder_id_at(&app, 9), None);
        assert_eq!(main_list_len(&app), 2);
    }

    #[test]
    fn main_list_len_follows_view_and_full_art_mode() {
        let mut app = App::new(None);
        app.main_view = MainView::MyMusic;
        assert_eq!(main_list_len(&app), 8);
        app.main_view = MainView::Help;
        assert_eq!(main_list_len(&app), 0);
        app.queue = vec![serde_json::from_value(json!({"title": "t"})).unwrap()];
        app.full_art_mode = true;
        assert_eq!(main_list_len(&app), 1);
    }

    #[test]
    fn artist_and_album_views_share_accessors() {
        let mut app = App::new(None);
        app.recent_artists = vec![serde_json::from_value(json!({"id": 9, "artist": "X"})).unwrap()];
        app.new_music = vec![serde_json::from_value(json!({"id": 4, "album": "Y"})).unwrap()];
        app.main_view = MainView::Library(LibraryView::RecentlyPlayedArtists);
        assert_eq!(main_list_len(&app), 1);
        assert_eq!(artist_id_at(&app, 0).as_deref(), Some("9"));
        app.main_view = MainView::Library(LibraryView::NewMusic);
        assert_eq!(main_list_len(&app), 1);
        let thumb = thumbnail_url_for(&app, 0, "http://h");
        assert_eq!(thumb.as_deref(), Some("http://h/music/4/cover.jpg"));
        assert_eq!(artist_id_at(&app, 0), None);
    }

    #[test]
    fn accent_normalization_hits_target_lightness() {
        let [r, g, b] = normalize_accent_lightness([200, 30, 30], 50);
        let (_, _, l) = rgb_to_hsl(r, g, b);
        assert!((l - 0.5).abs() < 0.02);
    }
}
