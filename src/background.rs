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
    app_services: Vec<RadioItem>,
    player_id: String,
    client: Arc<LmsClient>,
    tx: mpsc::Sender<AppMsg>,
) {
    tokio::spawn(async move {
        let mut results = Vec::new();

        // Spawn library search and all app searches concurrently
        let lib_handle = {
            let client = client.clone();
            let query = query.clone();
            tokio::spawn(async move { client.search_library(&query).await })
        };

        let mut app_handles = Vec::new();
        for svc in &app_services {
            if let Some(cmd) = svc.cmd.clone() {
                let client = client.clone();
                let query = query.clone();
                let player_id = player_id.clone();
                app_handles.push(tokio::spawn(async move {
                    client.search_app(&player_id, &cmd, &query).await
                }));
            }
        }

        if let Ok(Ok((artists, albums, tracks, playlists))) = lib_handle.await {
            for a in artists   { results.push(SearchResultItem::Artist(a)); }
            for a in albums    { results.push(SearchResultItem::Album(a)); }
            for t in tracks    { results.push(SearchResultItem::Track(t)); }
            for p in playlists { results.push(SearchResultItem::Playlist(p)); }
        }

        for handle in app_handles {
            if let Ok(Ok(items)) = handle.await {
                for item in items {
                    results.push(SearchResultItem::AppItem(item));
                }
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
