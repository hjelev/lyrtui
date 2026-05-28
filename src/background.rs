use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::api::{LmsClient, RadioItem};
use crate::app::{AppMsg, SearchResultItem, SearchScope};

fn spawn_if_ok<F, Fut, T>(client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>, op: F, wrap: fn(T) -> AppMsg)
where
    F: FnOnce(Arc<LmsClient>) -> Fut + Send + 'static,
    Fut: Future<Output = anyhow::Result<T>> + Send + 'static,
    T: Send + 'static,
{
    tokio::spawn(async move {
        if let Ok(result) = op(client).await {
            let _ = tx.send(wrap(result)).await;
        }
    });
}

pub fn start_now_playing_loop(pid: String, client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) -> tokio::task::AbortHandle {
    tokio::spawn(async move {
        let mut last_queue_timestamp: f64 = -1.0;
        loop {
            if let Ok(np) = client.get_now_playing(&pid).await {
                let ts = np.playlist_timestamp;
                let _ = tx.send(AppMsg::NowPlayingUpdated(pid.clone(), np)).await;

                if ts != last_queue_timestamp
                    && let Ok(q) = client.get_queue(&pid).await
                {
                    last_queue_timestamp = ts;
                    let _ = tx.send(AppMsg::QueueLoaded(pid.clone(), q)).await;
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .abort_handle()
}

pub fn trigger_search(
    query: String,
    scope: SearchScope,
    app_services: Vec<RadioItem>,
    radio_services: Vec<RadioItem>,
    player_id: String,
    client: Arc<LmsClient>,
    tx: mpsc::Sender<AppMsg>,
) {
    tokio::spawn(async move {
        let mut results = Vec::new();

        let do_library = matches!(scope, SearchScope::MyMusic | SearchScope::All);
        let do_radios  = matches!(scope, SearchScope::Radios  | SearchScope::All);
        let do_apps    = matches!(scope, SearchScope::Apps    | SearchScope::All);

        let lib_handle = if do_library {
            let c = client.clone();
            let q = query.clone();
            Some(tokio::spawn(async move { c.search_library(&q).await }))
        } else {
            None
        };

        let mut radio_handles: Vec<tokio::task::JoinHandle<anyhow::Result<Vec<RadioItem>>>> = Vec::new();
        let mut app_handles: Vec<tokio::task::JoinHandle<anyhow::Result<Vec<RadioItem>>>> = Vec::new();

        if do_radios {
            for svc in &radio_services {
                if let Some(cmd) = svc.cmd.clone() {
                    let c = client.clone();
                    let q = query.clone();
                    let pid = player_id.clone();
                    radio_handles.push(tokio::spawn(async move {
                        c.search_app(&pid, &cmd, &q).await
                    }));
                }
            }
        }

        if do_apps {
            for svc in &app_services {
                if let Some(cmd) = svc.cmd.clone() {
                    let c = client.clone();
                    let q = query.clone();
                    let pid = player_id.clone();
                    app_handles.push(tokio::spawn(async move {
                        c.search_app(&pid, &cmd, &q).await
                    }));
                }
            }
        }

        if let Some(h) = lib_handle
            && let Ok(Ok((artists, albums, tracks, playlists))) = h.await
        {
            for a in artists   { results.push(SearchResultItem::Artist(a)); }
            for a in albums    { results.push(SearchResultItem::Album(a)); }
            for t in tracks    { results.push(SearchResultItem::Track(t)); }
            for p in playlists { results.push(SearchResultItem::Playlist(p)); }
        }

        let q_lower = query.to_lowercase();
        for handle in radio_handles {
            if let Ok(Ok(items)) = handle.await {
                for item in items {
                    if item.name.to_lowercase().contains(&q_lower) {
                        results.push(SearchResultItem::RadioItem(item));
                    }
                }
            }
        }
        for handle in app_handles {
            if let Ok(Ok(items)) = handle.await {
                for item in items {
                    if item.name.to_lowercase().contains(&q_lower) {
                        results.push(SearchResultItem::AppItem(item));
                    }
                }
            }
        }

        let _ = tx.send(AppMsg::SearchResultsLoaded(results)).await;
    });
}

pub fn load_albums(artist_id: Option<String>, client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    spawn_if_ok(client, tx, |c| async move { c.get_albums(artist_id.as_deref()).await }, AppMsg::AlbumsLoaded);
}

pub fn load_radio_services(client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    spawn_if_ok(client, tx, |c| async move { c.get_radio_services().await }, AppMsg::RadioItemsLoaded);
}

pub fn load_radio_items(
    player_id: String,
    cmd: String,
    item_id: Option<String>,
    client: Arc<LmsClient>,
    tx: mpsc::Sender<AppMsg>,
) {
    spawn_if_ok(client, tx, |c| async move {
        c.browse_radio(&player_id, &cmd, item_id.as_deref()).await
    }, AppMsg::RadioItemsLoaded);
}

pub fn load_fav_items(
    player_id: String,
    item_id: Option<String>,
    client: Arc<LmsClient>,
    tx: mpsc::Sender<AppMsg>,
) {
    spawn_if_ok(client, tx, |c| async move {
        c.browse_radio(&player_id, "favorites", item_id.as_deref()).await
    }, AppMsg::FavItemsLoaded);
}

pub fn load_app_services(client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    spawn_if_ok(client, tx, |c| async move { c.get_apps().await }, AppMsg::AppItemsLoaded);
}

pub fn load_app_items(
    player_id: String,
    cmd: String,
    item_id: Option<String>,
    client: Arc<LmsClient>,
    tx: mpsc::Sender<AppMsg>,
) {
    tokio::spawn(async move {
        let Ok(mut items) = client.browse_radio(&player_id, &cmd, item_id.as_deref()).await else { return; };
        if cmd == "radioparadise" {
            let mut set = tokio::task::JoinSet::new();
            for (i, item) in items.iter().enumerate() {
                if item.artwork_url.is_none()
                    && let Some(chan) = rp_channel_num(item.item_id.as_deref())
                {
                    let c = Arc::clone(&client);
                    set.spawn(async move { (i, c.fetch_radio_paradise_art_url(chan).await) });
                }
            }
            while let Some(Ok((i, Ok(url)))) = set.join_next().await {
                items[i].artwork_url = Some(url);
            }
        }
        let _ = tx.send(AppMsg::AppItemsLoaded(items)).await;
    });
}

/// Extract the Radio Paradise API channel number from an item_id like "b44f4573.N" or "b44f4573.N.M".
fn rp_channel_num(item_id: Option<&str>) -> Option<u8> {
    item_id?.split('.').nth(1)?.parse().ok()
}

pub fn load_folder_items(folder_id: Option<u32>, client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    spawn_if_ok(client, tx, move |c| async move { c.browse_music_folder(folder_id).await }, AppMsg::FolderItemsLoaded);
}

pub fn load_playlists(client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    spawn_if_ok(client, tx, |c| async move { c.get_playlists().await }, AppMsg::PlaylistsLoaded);
}

pub fn load_all_tracks(client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    spawn_if_ok(client, tx, |c| async move { c.get_all_tracks().await }, AppMsg::TracksLoaded);
}

pub fn load_tracks(album_id: String, client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    spawn_if_ok(client, tx, |c| async move { c.get_tracks(&album_id).await }, AppMsg::TracksLoaded);
}
