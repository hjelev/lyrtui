//! Local panel filter (`/`).
//!
//! Narrows the current right-side list *in memory* — no network. The view's backing Vec is
//! replaced with the matching subset while the full list is stashed in [`FilterBackup`] for an
//! instant restore. Because the Vec *is* the filtered list, every existing read (selection,
//! `main_list_len`, thumbnails, the per-view `make_row` closures) keeps working untouched.
//!
//! All the per-view `match`ing lives here so the rest of the codebase stays oblivious to the
//! filter. Lifecycle:
//! - [`open`]   — snapshot the current view's Vec, start editing.
//! - [`recompute`] — re-derive the filtered Vec from the backup (called live on each keystroke).
//! - [`clear`]  — restore the full Vec, drop the filter.
//! - [`reapply_if_active`] — a background load replaced an owned Vec (e.g. the periodic queue
//!   refresh); refresh the backup and re-filter so the load doesn't blow the filter away.
//! - [`clear_if_view_changed`] — the user navigated elsewhere; drop the filter.

use crate::api::{Album, Artist, FolderItem, Playlist, RadioItem, Track};
use crate::app::{App, FilterBackup, LibraryView, LocalFilter, MainView};

/// Case-insensitive substring test. An empty query matches everything (so an open-but-empty
/// filter box shows the full list).
fn text_matches(query_lc: &str, haystack: &str) -> bool {
    query_lc.is_empty() || haystack.to_lowercase().contains(query_lc)
}

/// Views whose list can be filtered. Excludes MyMusic (static menu), Players, Help, and the two
/// search views (which own their own `/`-style input).
pub fn is_filterable(view: &MainView) -> bool {
    matches!(
        view,
        MainView::Library(
            LibraryView::Artists
                | LibraryView::AlbumArtists
                | LibraryView::RecentlyPlayedArtists
                | LibraryView::Albums { .. }
                | LibraryView::NewMusic
                | LibraryView::Tracks { .. }
                | LibraryView::Folder { .. }
                | LibraryView::Playlists
        ) | MainView::Queue
            | MainView::Radio
            | MainView::Apps
            | MainView::Favourites
    )
}

/// Clone the full Vec backing `view` into a [`FilterBackup`]. `None` for non-filterable views.
fn snapshot_view(app: &App, view: &MainView) -> Option<FilterBackup> {
    Some(match view {
        MainView::Library(LibraryView::Artists) => FilterBackup::Artists(app.artists.clone()),
        MainView::Library(LibraryView::AlbumArtists) => {
            FilterBackup::Artists(app.album_artists.clone())
        }
        MainView::Library(LibraryView::RecentlyPlayedArtists) => {
            FilterBackup::Artists(app.recent_artists.clone())
        }
        MainView::Library(LibraryView::Albums { .. }) => FilterBackup::Albums(app.albums.clone()),
        MainView::Library(LibraryView::NewMusic) => {
            FilterBackup::Albums(app.new_music.clone())
        }
        MainView::Library(LibraryView::Tracks { .. }) => FilterBackup::Tracks(app.tracks.clone()),
        MainView::Queue => FilterBackup::Tracks(app.queue.clone()),
        MainView::Library(LibraryView::Folder { .. }) => {
            FilterBackup::Folder(app.folder_items.clone())
        }
        MainView::Library(LibraryView::Playlists) => {
            FilterBackup::Playlists(app.playlists.clone())
        }
        MainView::Radio => FilterBackup::Items(app.radio_items.clone()),
        MainView::Apps => FilterBackup::Items(app.app_items.clone()),
        MainView::Favourites => FilterBackup::Items(app.fav_items.clone()),
        _ => return None,
    })
}

// --- per-type retain helpers (title + subtitle matching) ---

fn retain_artists(full: &[Artist], q: &str) -> Vec<Artist> {
    full.iter().filter(|a| text_matches(q, &a.artist)).cloned().collect()
}

fn retain_albums(full: &[Album], q: &str) -> Vec<Album> {
    full.iter()
        .filter(|a| {
            text_matches(q, &a.album)
                || a.artist.as_deref().is_some_and(|s| text_matches(q, s))
        })
        .cloned()
        .collect()
}

fn retain_tracks(full: &[Track], q: &str) -> Vec<Track> {
    full.iter()
        .filter(|t| {
            text_matches(q, &t.title)
                || t.artist.as_deref().is_some_and(|s| text_matches(q, s))
        })
        .cloned()
        .collect()
}

fn retain_playlists(full: &[Playlist], q: &str) -> Vec<Playlist> {
    full.iter().filter(|p| text_matches(q, &p.name)).cloned().collect()
}

fn retain_folders(full: &[FolderItem], q: &str) -> Vec<FolderItem> {
    full.iter().filter(|i| text_matches(q, &i.filename)).cloned().collect()
}

fn retain_items(full: &[RadioItem], q: &str) -> Vec<RadioItem> {
    full.iter().filter(|i| text_matches(q, &i.name)).cloned().collect()
}

/// Open the filter on the current view (no-op if one is already active or the view isn't
/// filterable). Starts in editing mode with an empty query (full list shown).
pub fn open(app: &mut App) {
    if app.local_filter.is_some() {
        return;
    }
    let owner = app.main_view.clone();
    let Some(backup) = snapshot_view(app, &owner) else {
        return;
    };
    app.local_filter = Some(LocalFilter {
        query: String::new(),
        cursor: 0,
        editing: true,
        owner,
        backup,
    });
}

/// Re-derive the filtered Vec from the backup using the current query. Called live after each
/// keystroke. Clamps `main_selected` into the new (possibly shorter) list.
pub fn recompute(app: &mut App) {
    let Some(filter) = app.local_filter.take() else {
        return;
    };
    let q = filter.query.to_lowercase();
    match (&filter.owner, &filter.backup) {
        (MainView::Library(LibraryView::Artists), FilterBackup::Artists(full)) => {
            app.artists = retain_artists(full, &q);
        }
        (MainView::Library(LibraryView::AlbumArtists), FilterBackup::Artists(full)) => {
            app.album_artists = retain_artists(full, &q);
        }
        (MainView::Library(LibraryView::RecentlyPlayedArtists), FilterBackup::Artists(full)) => {
            app.recent_artists = retain_artists(full, &q);
        }
        (MainView::Library(LibraryView::Albums { .. }), FilterBackup::Albums(full)) => {
            app.albums = retain_albums(full, &q);
        }
        (MainView::Library(LibraryView::NewMusic), FilterBackup::Albums(full)) => {
            app.new_music = retain_albums(full, &q);
        }
        (MainView::Library(LibraryView::Tracks { .. }), FilterBackup::Tracks(full)) => {
            app.tracks = retain_tracks(full, &q);
        }
        (MainView::Queue, FilterBackup::Tracks(full)) => {
            app.queue = retain_tracks(full, &q);
        }
        (MainView::Library(LibraryView::Folder { .. }), FilterBackup::Folder(full)) => {
            app.folder_items = retain_folders(full, &q);
        }
        (MainView::Library(LibraryView::Playlists), FilterBackup::Playlists(full)) => {
            app.playlists = retain_playlists(full, &q);
        }
        (MainView::Radio, FilterBackup::Items(full)) => {
            app.radio_items = retain_items(full, &q);
        }
        (MainView::Apps, FilterBackup::Items(full)) => {
            app.app_items = retain_items(full, &q);
        }
        (MainView::Favourites, FilterBackup::Items(full)) => {
            app.fav_items = retain_items(full, &q);
        }
        _ => {}
    }
    app.local_filter = Some(filter);
    let len = crate::utils::main_list_len(app);
    if app.main_selected >= len {
        app.main_selected = len.saturating_sub(1);
    }
}

/// Restore the full list (matched on the filter's `owner`, not the current view) and drop the
/// filter. No-op when no filter is active.
pub fn clear(app: &mut App) {
    let Some(filter) = app.local_filter.take() else {
        return;
    };
    match (filter.owner, filter.backup) {
        (MainView::Library(LibraryView::Artists), FilterBackup::Artists(v)) => app.artists = v,
        (MainView::Library(LibraryView::AlbumArtists), FilterBackup::Artists(v)) => {
            app.album_artists = v
        }
        (MainView::Library(LibraryView::RecentlyPlayedArtists), FilterBackup::Artists(v)) => {
            app.recent_artists = v
        }
        (MainView::Library(LibraryView::Albums { .. }), FilterBackup::Albums(v)) => app.albums = v,
        (MainView::Library(LibraryView::NewMusic), FilterBackup::Albums(v)) => {
            app.new_music = v
        }
        (MainView::Library(LibraryView::Tracks { .. }), FilterBackup::Tracks(v)) => app.tracks = v,
        (MainView::Queue, FilterBackup::Tracks(v)) => app.queue = v,
        (MainView::Library(LibraryView::Folder { .. }), FilterBackup::Folder(v)) => {
            app.folder_items = v
        }
        (MainView::Library(LibraryView::Playlists), FilterBackup::Playlists(v)) => {
            app.playlists = v
        }
        (MainView::Radio, FilterBackup::Items(v)) => app.radio_items = v,
        (MainView::Apps, FilterBackup::Items(v)) => app.app_items = v,
        (MainView::Favourites, FilterBackup::Items(v)) => app.fav_items = v,
        _ => {}
    }
}

/// A background load just replaced the Vec backing `view`; if the active filter owns that view
/// (notably the periodic `QueueLoaded`), refresh the backup from the new full list and re-filter
/// so the reload doesn't discard the filter.
pub fn reapply_if_active(app: &mut App, view: &MainView) {
    match &app.local_filter {
        Some(f) if &f.owner == view => {}
        _ => return,
    }
    if let Some(new_backup) = snapshot_view(app, view) {
        if let Some(f) = &mut app.local_filter {
            f.backup = new_backup;
        }
        recompute(app);
    }
}

/// Drop the filter if the user has navigated to a different view than the one it was opened on.
/// Called once per event-loop iteration, catching every navigation path from one place.
pub fn clear_if_view_changed(app: &mut App) {
    if let Some(f) = &app.local_filter
        && f.owner != app.main_view
    {
        clear(app);
    }
}
