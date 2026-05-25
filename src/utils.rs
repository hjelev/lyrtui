use crate::api::FolderItemType;
use crate::app::{App, LibraryView, MainView, SearchResultItem};

pub fn json_id_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub fn thumbnail_url_for(app: &App, idx: usize, base: &str) -> Option<String> {
    if app.full_art_mode {
        return app.queue.get(idx).and_then(|t| t.artwork_url.clone());
    }
    match &app.main_view {
        MainView::Library(LibraryView::Artists) => app
            .artists
            .get(idx)
            .map(|a| format!("{}/music/{}/artist.jpg", base, json_id_to_string(&a.id))),
        MainView::Library(LibraryView::Albums { .. }) => app
            .albums
            .get(idx)
            .map(|a| format!("{}/music/{}/cover.jpg", base, json_id_to_string(&a.id))),
        MainView::Library(LibraryView::Tracks { .. }) => app.tracks.get(idx).and_then(|t| {
            t.id.as_ref()
                .map(|id| format!("{}/music/{}/cover.jpg", base, json_id_to_string(id)))
        }),
        MainView::Library(LibraryView::Folder { .. }) => {
            app.folder_items.get(idx).and_then(|item| {
                if item.item_type == FolderItemType::Track {
                    Some(format!("{}/music/{}/cover.jpg", base, item.id))
                } else {
                    None
                }
            })
        }
        MainView::Queue => app.queue.get(idx).and_then(|t| t.artwork_url.clone()),
        MainView::Radio => app.radio_items.get(idx).and_then(|i| i.artwork_url.clone()),
        MainView::Apps => app.app_items.get(idx).and_then(|i| i.artwork_url.clone()),
        MainView::Favourites => app.fav_items.get(idx).and_then(|i| i.artwork_url.clone()),
        MainView::Search => match app.search_results.get(idx) {
            Some(SearchResultItem::Artist(a)) => Some(format!(
                "{}/music/{}/artist.jpg",
                base,
                json_id_to_string(&a.id)
            )),
            Some(SearchResultItem::Album(alb)) => Some(format!(
                "{}/music/{}/cover.jpg",
                base,
                json_id_to_string(&alb.id)
            )),
            Some(SearchResultItem::Track(t)) => t
                .id
                .as_ref()
                .map(|id| format!("{}/music/{}/cover.jpg", base, json_id_to_string(id))),
            Some(SearchResultItem::AppItem(item)) => item.artwork_url.clone(),
            _ => None,
        },
        _ => None,
    }
}

pub fn compute_parent_label(app: &App) -> Option<String> {
    match &app.main_view {
        MainView::Search => None,
        MainView::Library(LibraryView::Tracks { album_id: Some(id) }) => {
            let name = app
                .albums
                .iter()
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
        MainView::Library(LibraryView::Folder { folder_id: Some(_) }) => {
            Some(format!("Add \"{}\" to queue", app.folder_title))
        }
        _ => None,
    }
}

pub fn uses_two_row_layout(view: &MainView) -> bool {
    !matches!(view, MainView::Players | MainView::Help | MainView::MyMusic)
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
            .map(|r| matches!(r, SearchResultItem::Track(_) | SearchResultItem::Playlist(_)))
            .unwrap_or(false),
        _ => false,
    }
}

pub fn main_list_len(app: &App) -> usize {
    if app.full_art_mode {
        return app.queue.len();
    }
    match &app.main_view {
        MainView::MyMusic => 4,
        MainView::Library(LibraryView::Artists) => app.artists.len(),
        MainView::Library(LibraryView::Albums { .. }) => app.albums.len(),
        MainView::Library(LibraryView::Tracks { .. }) => app.tracks.len(),
        MainView::Library(LibraryView::Folder { .. }) => app.folder_items.len(),
        MainView::Queue => app.queue.len(),
        MainView::Players => app.players.len(),
        MainView::Radio => app.radio_items.len(),
        MainView::Apps => app.app_items.len(),
        MainView::Favourites => app.fav_items.len(),
        MainView::Help => 0,
        MainView::Search => app.search_results.len(),
    }
}
