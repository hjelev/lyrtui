use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::api::{LmsClient, RadioItem};
use crate::app::{AppMsg, SearchResultItem};

pub fn start_now_playing_loop(pid: String, client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
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

pub fn trigger_search(
    query: String,
    app_items: Vec<RadioItem>,
    client: Arc<LmsClient>,
    tx: mpsc::Sender<AppMsg>,
) {
    let q_lower = query.to_lowercase();
    tokio::spawn(async move {
        let mut results = Vec::new();

        if let Ok((artists, albums, tracks, playlists)) = client.search_library(&query).await {
            for a in artists {
                results.push(SearchResultItem::Artist(a));
            }
            for a in albums {
                results.push(SearchResultItem::Album(a));
            }
            for t in tracks {
                results.push(SearchResultItem::Track(t));
            }
            for p in playlists {
                results.push(SearchResultItem::Playlist(p));
            }
        }

        // Client-side filter on loaded app items (streaming services by name)
        for item in app_items {
            if item.name.to_lowercase().contains(&q_lower) {
                results.push(SearchResultItem::AppItem(item));
            }
        }

        let _ = tx.send(AppMsg::SearchResultsLoaded(results)).await;
    });
}

pub fn load_albums(artist_id: Option<String>, client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    tokio::spawn(async move {
        let id_ref = artist_id.as_deref();
        if let Ok(albums) = client.get_albums(id_ref).await {
            let _ = tx.send(AppMsg::AlbumsLoaded(albums)).await;
        }
    });
}

pub fn load_radio_services(client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    tokio::spawn(async move {
        if let Ok(items) = client.get_radio_services().await {
            let _ = tx.send(AppMsg::RadioItemsLoaded(items)).await;
        }
    });
}

pub fn load_radio_items(
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

pub fn load_fav_items(
    player_id: String,
    item_id: Option<String>,
    client: Arc<LmsClient>,
    tx: mpsc::Sender<AppMsg>,
) {
    tokio::spawn(async move {
        if let Ok(items) = client
            .browse_radio(&player_id, "favorites", item_id.as_deref())
            .await
        {
            let _ = tx.send(AppMsg::FavItemsLoaded(items)).await;
        }
    });
}

pub fn load_app_services(client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    tokio::spawn(async move {
        if let Ok(items) = client.get_apps().await {
            let _ = tx.send(AppMsg::AppItemsLoaded(items)).await;
        }
    });
}

pub fn load_app_items(
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

pub fn load_folder_items(folder_id: Option<u32>, client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    tokio::spawn(async move {
        if let Ok(items) = client.browse_music_folder(folder_id).await {
            let _ = tx.send(AppMsg::FolderItemsLoaded(items)).await;
        }
    });
}

pub fn load_all_tracks(client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    tokio::spawn(async move {
        if let Ok(tracks) = client.get_all_tracks().await {
            let _ = tx.send(AppMsg::TracksLoaded(tracks)).await;
        }
    });
}

pub fn load_tracks(album_id: String, client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    tokio::spawn(async move {
        if let Ok(tracks) = client.get_tracks(&album_id).await {
            let _ = tx.send(AppMsg::TracksLoaded(tracks)).await;
        }
    });
}
