use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::api::{Album, Artist, LmsClient, RadioItem};
use crate::app::{App, AppMsg, SearchResultItem, SearchScope};
use crate::utils;

/// Now-playing poll cadence: fast while playing (smooth progress bar), backed off otherwise.
const POLL_PLAYING_MS: u64 = 500;
const POLL_IDLE_MS: u64 = 2000;
/// `color_thief` palette extraction tuning (quality, max colors) for accent detection.
const PALETTE_QUALITY: u8 = 10;
const PALETTE_MAX_COLORS: u8 = 5;
/// Luma window for picking an accent that is neither too dark nor too bright.
const ACCENT_LUMA_MIN: u32 = 70;
const ACCENT_LUMA_MAX: u32 = 210;

/// Fetch image bytes for `url` and decode them; `None` on any network or decode error.
pub async fn fetch_and_decode(client: &LmsClient, url: &str) -> Option<image::DynamicImage> {
    let bytes = client.fetch_image_bytes(url).await.ok()?;
    image::load_from_memory(&bytes).ok()
}

pub fn spawn_artwork_fetch(url: String, client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    tokio::spawn(async move {
        let ok = async {
            let img = fetch_and_decode(&client, &url).await?;
            let rgb = img.to_rgb8();
            let accent = color_thief::get_palette(
                rgb.as_raw(),
                color_thief::ColorFormat::Rgb,
                PALETTE_QUALITY,
                PALETTE_MAX_COLORS,
            )
            .ok()
            .and_then(|colors| {
                colors
                    .iter()
                    .find(|c| {
                        let luma = (c.r as u32 * 299 + c.g as u32 * 587 + c.b as u32 * 114)
                            / 1000;
                        (ACCENT_LUMA_MIN..=ACCENT_LUMA_MAX).contains(&luma)
                    })
                    .or_else(|| colors.first())
                    .map(|c| [c.r, c.g, c.b])
            });
            let dimensions = (img.width(), img.height());
            let art_normal = crate::artwork::with_rounded_corners(img.clone(), crate::artwork::ART_RADIUS_NORMAL);
            let art_full = crate::artwork::with_rounded_corners(img.clone(), crate::artwork::ART_RADIUS_FULL);
            let _ = tx
                .send(AppMsg::ArtworkDecoded {
                    img,
                    art_normal,
                    art_full,
                    accent,
                    dimensions,
                })
                .await;
            Some(())
        }
        .await;
        if ok.is_none() {
            let _ = tx.send(AppMsg::ArtworkFetchFailed(url)).await;
        }
    });
}

fn spawn_if_ok<F, Fut, T>(
    client: Arc<LmsClient>,
    tx: mpsc::Sender<AppMsg>,
    op: F,
    wrap: fn(T) -> AppMsg,
) where
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

pub fn start_now_playing_loop(
    pid: String,
    client: Arc<LmsClient>,
    tx: mpsc::Sender<AppMsg>,
) -> tokio::task::AbortHandle {
    tokio::spawn(async move {
        let mut last_queue_timestamp: f64 = -1.0;
        loop {
            let mut playing = false;
            if let Ok(np) = client.get_now_playing(&pid).await {
                let ts = np.playlist_timestamp;
                playing = np.is_playing;
                let _ = tx.send(AppMsg::NowPlayingUpdated(pid.clone(), np)).await;

                if ts != last_queue_timestamp
                    && let Ok(q) = client.get_queue(&pid).await
                {
                    last_queue_timestamp = ts;
                    let _ = tx.send(AppMsg::QueueLoaded(pid.clone(), q)).await;
                }
            }
            // Poll fast while playing (keeps the progress bar smooth); back off when paused,
            // stopped, or unreachable to cut idle network/CPU.
            let delay = if playing { POLL_PLAYING_MS } else { POLL_IDLE_MS };
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
    })
    .abort_handle()
}

/// Browse an app's menu tree to find the item_id of its "New Search" node.
/// Checks the top level first, then one level deeper into any "Search"-named folder.
async fn find_app_search_item_id(
    client: &Arc<LmsClient>,
    player_id: &str,
    cmd: &str,
) -> Option<String> {
    let Ok(items) = client.browse_radio(player_id, cmd, None).await else {
        return None;
    };
    for item in &items {
        if item.item_type == "search" {
            return item.item_id.clone();
        }
    }
    // Check one level into any "Search"-named folder
    for item in &items {
        if item.name.to_lowercase().contains("search")
            && let Some(sub_id) = &item.item_id
            && let Ok(sub_items) = client.browse_radio(player_id, cmd, Some(sub_id)).await
        {
            for sub_item in &sub_items {
                if sub_item.item_type == "search" {
                    return sub_item.item_id.clone();
                }
            }
        }
    }
    None
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
        let do_radios = matches!(scope, SearchScope::Radios | SearchScope::All);
        let do_apps = matches!(scope, SearchScope::Apps | SearchScope::All);

        let lib_handle = if do_library {
            let c = client.clone();
            let q = query.clone();
            Some(tokio::spawn(async move { c.search_library(&q).await }))
        } else {
            None
        };

        let mut radio_handles: Vec<tokio::task::JoinHandle<anyhow::Result<Vec<RadioItem>>>> =
            Vec::new();
        let mut app_handles: Vec<tokio::task::JoinHandle<anyhow::Result<Vec<RadioItem>>>> =
            Vec::new();

        if do_radios {
            for svc in &radio_services {
                if let Some(cmd) = svc.cmd.clone() {
                    let c = client.clone();
                    let q = query.clone();
                    let pid = player_id.clone();
                    radio_handles.push(tokio::spawn(
                        async move { c.search_app(&pid, &cmd, &q).await },
                    ));
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
                        let item_id = find_app_search_item_id(&c, &pid, &cmd).await;
                        c.search_app_via_item(&pid, &cmd, item_id.as_deref(), &q)
                            .await
                    }));
                }
            }
        }

        if let Some(h) = lib_handle
            && let Ok(Ok((artists, albums, tracks, playlists))) = h.await
        {
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
                    results.push(SearchResultItem::AppItem(item));
                }
            }
        }

        let _ = tx.send(AppMsg::SearchResultsLoaded(results)).await;
    });
}

pub fn trigger_app_specific_search(
    query: String,
    cmd: String,
    item_id: Option<String>,
    player_id: String,
    client: Arc<LmsClient>,
    tx: mpsc::Sender<AppMsg>,
) {
    tokio::spawn(async move {
        if let Ok(items) = client
            .search_app_via_item(&player_id, &cmd, item_id.as_deref(), &query)
            .await
        {
            let _ = tx.send(AppMsg::AppSearchResultsLoaded(items)).await;
        }
    });
}

pub fn load_albums(artist_id: Option<String>, client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    spawn_if_ok(
        client,
        tx,
        |c| async move { c.get_albums(artist_id.as_deref()).await },
        AppMsg::AlbumsLoaded,
    );
}

pub fn load_radio_services(client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    spawn_if_ok(
        client,
        tx,
        |c| async move { c.get_radio_services().await },
        AppMsg::RadioItemsLoaded,
    );
}

pub fn load_radio_items(
    player_id: String,
    cmd: String,
    item_id: Option<String>,
    client: Arc<LmsClient>,
    tx: mpsc::Sender<AppMsg>,
) {
    spawn_if_ok(
        client,
        tx,
        |c| async move { c.browse_radio(&player_id, &cmd, item_id.as_deref()).await },
        AppMsg::RadioItemsLoaded,
    );
}

pub fn load_fav_items(
    player_id: String,
    item_id: Option<String>,
    client: Arc<LmsClient>,
    tx: mpsc::Sender<AppMsg>,
) {
    spawn_if_ok(
        client,
        tx,
        |c| async move {
            c.browse_radio(&player_id, "favorites", item_id.as_deref())
                .await
        },
        AppMsg::FavItemsLoaded,
    );
}

pub fn load_app_services(client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    spawn_if_ok(
        client,
        tx,
        |c| async move { c.get_apps().await },
        AppMsg::AppItemsLoaded,
    );
}

pub fn load_app_items(
    player_id: String,
    cmd: String,
    item_id: Option<String>,
    client: Arc<LmsClient>,
    tx: mpsc::Sender<AppMsg>,
) {
    tokio::spawn(async move {
        let Ok(mut items) = client
            .browse_radio(&player_id, &cmd, item_id.as_deref())
            .await
        else {
            return;
        };
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
    spawn_if_ok(
        client,
        tx,
        move |c| async move { c.browse_music_folder(folder_id).await },
        AppMsg::FolderItemsLoaded,
    );
}

pub fn load_playlists(client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    spawn_if_ok(
        client,
        tx,
        |c| async move { c.get_playlists().await },
        AppMsg::PlaylistsLoaded,
    );
}

pub fn load_all_tracks(client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    spawn_if_ok(
        client,
        tx,
        |c| async move { c.get_all_tracks().await },
        AppMsg::TracksLoaded,
    );
}

pub fn load_recent_artists(limit: usize, client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    spawn_if_ok(
        client,
        tx,
        move |c| async move { c.get_recently_played_artists(limit).await },
        |v: Vec<Artist>| AppMsg::RecentArtistsLoaded(v),
    );
}

pub fn load_new_music(limit: usize, client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    spawn_if_ok(
        client,
        tx,
        move |c| async move { c.get_new_music(limit).await },
        |v: Vec<Album>| AppMsg::NewMusicLoaded(v),
    );
}

pub fn load_tracks(album_id: String, client: Arc<LmsClient>, tx: mpsc::Sender<AppMsg>) {
    spawn_if_ok(
        client,
        tx,
        |c| async move { c.get_tracks(&album_id).await },
        AppMsg::TracksLoaded,
    );
}

pub fn set_timed_status(app: &mut App, msg: String, tx: &mpsc::Sender<AppMsg>) {
    app.status_message_gen += 1;
    let seq = app.status_message_gen;
    app.status_message = Some(msg);
    let t = tx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(4)).await;
        let _ = t.send(AppMsg::ClearStatusMsg(seq)).await;
    });
}

pub fn resolve_artist_art(
    app: &App,
    idx: usize,
    pending: &mut HashSet<String>,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    if let Some(artist_id) = utils::artist_id_at(app, idx)
        && !app.artist_artwork.contains_key(&artist_id)
        && !pending.contains(&artist_id)
    {
        pending.insert(artist_id.clone());
        let c = client.clone();
        let t = tx.clone();
        tokio::spawn(async move {
            let url = c.get_artist_artwork(&artist_id).await;
            let _ = t.send(AppMsg::ArtistArtworkResolved(artist_id, url)).await;
        });
    }
}

pub fn resolve_folder_art(
    app: &App,
    idx: usize,
    pending: &mut HashSet<u32>,
    client: &Arc<LmsClient>,
    tx: &mpsc::Sender<AppMsg>,
) {
    if let Some(folder_id) = utils::folder_id_at(app, idx)
        && !app.folder_artwork.contains_key(&folder_id)
        && !pending.contains(&folder_id)
    {
        pending.insert(folder_id);
        let c = client.clone();
        let t = tx.clone();
        tokio::spawn(async move {
            let url = c.get_folder_artwork(folder_id).await;
            let _ = t.send(AppMsg::FolderArtworkResolved(folder_id, url)).await;
        });
    }
}
