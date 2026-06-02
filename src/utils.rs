use crate::api::FolderItemType;
use crate::app::{App, LibraryView, MainView, SearchResultItem};

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
    match &app.main_view {
        MainView::Library(LibraryView::Artists) => app
            .artists
            .get(idx)
            .and_then(|a| artist_artwork_url(app, &a.id)),
        MainView::Library(LibraryView::AlbumArtists) => app
            .album_artists
            .get(idx)
            .and_then(|a| artist_artwork_url(app, &a.id)),
        MainView::Library(LibraryView::Albums { .. }) => {
            app.albums.get(idx).map(|a| a.cover_url(base))
        }
        MainView::Library(LibraryView::PopularAlbums) => {
            app.popular_albums.get(idx).map(|a| a.cover_url(base))
        }
        MainView::Library(LibraryView::RecentlyPlayedArtists) => app
            .recent_artists
            .get(idx)
            .and_then(|a| artist_artwork_url(app, &a.id)),
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
    let list = match &app.main_view {
        MainView::Library(LibraryView::Artists) => &app.artists,
        MainView::Library(LibraryView::AlbumArtists) => &app.album_artists,
        MainView::Library(LibraryView::RecentlyPlayedArtists) => &app.recent_artists,
        MainView::Search => {
            return match app.search_results.get(idx) {
                Some(SearchResultItem::Artist(a)) => Some(json_id_to_string(&a.id)),
                _ => None,
            };
        }
        _ => return None,
    };
    list.get(idx).map(|a| json_id_to_string(&a.id))
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

pub fn main_list_len(app: &App) -> usize {
    if app.full_art_mode {
        return app.queue.len();
    }
    match &app.main_view {
        MainView::MyMusic => 8,
        MainView::Library(LibraryView::Artists) => app.artists.len(),
        MainView::Library(LibraryView::AlbumArtists) => app.album_artists.len(),
        MainView::Library(LibraryView::Albums { .. }) => app.albums.len(),
        MainView::Library(LibraryView::Tracks { .. }) => app.tracks.len(),
        MainView::Library(LibraryView::Folder { .. }) => app.folder_items.len(),
        MainView::Library(LibraryView::Playlists) => app.playlists.len(),
        MainView::Library(LibraryView::RecentlyPlayedArtists) => app.recent_artists.len(),
        MainView::Library(LibraryView::PopularAlbums) => app.popular_albums.len(),
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
