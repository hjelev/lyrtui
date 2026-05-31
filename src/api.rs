use anyhow::{Result, anyhow};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::RwLock;

#[derive(Debug, Clone, Deserialize)]
pub struct Player {
    pub playerid: String,
    pub name: String,
    #[serde(default)]
    pub power: u8,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlayerDetail {
    pub playerid: String,
    pub name: String,
    pub model: Option<String>,
    pub modelname: Option<String>,
    #[serde(default)]
    pub connected: u8,
    pub ip: Option<String>,
    #[serde(default)]
    pub power: u8,
    #[serde(rename = "isplaying", default)]
    pub is_playing: u8,
    pub firmware: Option<String>,
    pub uuid: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ServerInfo {
    pub version: Option<String>,
    pub uuid: Option<String>,
    pub mac: Option<String>,
    pub ip: Option<String>,
    #[serde(rename = "player count")]
    pub player_count: Option<u64>,
    pub name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: Option<u32>,
    pub duration: f64,
    pub elapsed: f64,
    pub volume: u8,
    pub is_playing: bool,
    pub shuffle: u8,
    pub repeat: u8,
    pub artwork_url: Option<String>,
    pub playlist_cur_index: Option<usize>,
    pub playlist_tracks: Option<usize>,
    pub playlist_timestamp: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Track {
    pub id: Option<Value>,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<f64>,
    #[serde(default)]
    pub artwork_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Artist {
    pub id: Value,
    pub artist: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FolderItemType {
    Folder,
    Track,
}

#[derive(Debug, Clone)]
pub struct FolderItem {
    pub id: u32,
    pub filename: String,
    pub item_type: FolderItemType,
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Album {
    pub id: Value,
    pub album: String,
    pub artist: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Playlist {
    pub id: Value,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct RadioItem {
    pub name: String,
    /// "audio", "playlist", "link", "outline", "text", …
    pub item_type: String,
    /// Stream URL, present on playable items when `want_url:1` is requested.
    pub url: Option<String>,
    /// The XMLBrowser command for this subtree (e.g. "tunein", "picks").
    pub cmd: Option<String>,
    /// Opaque item id used to navigate into this item.
    pub item_id: Option<String>,
    /// Thumbnail / cover art URL for this item, if provided by the server.
    pub artwork_url: Option<String>,
    /// True when the server reports hasitems:1 — this item contains sub-items to browse.
    pub has_items: bool,
    /// True when the server reports isaudio:1 — this item is audio content (track or stream).
    pub is_audio: bool,
}

impl RadioItem {
    /// An item is playable when it carries a stream URL and is identified as audio content.
    /// "link" type items may carry browse URLs (not streams), so we require isaudio:1 or
    /// an explicit "audio"/"playlist" type to distinguish streams from browse links.
    pub fn is_playable(&self) -> bool {
        self.url.is_some()
            && (self.is_audio || matches!(self.item_type.as_str(), "audio" | "playlist"))
    }
    /// An item is navigable when it has sub-items AND is not a direct audio leaf.
    /// Spotty (and similar plugins) set hasitems:1 on tracks but also set isaudio:1 and
    /// provide a stream URL — those are leaves, not folders. TuneIn folders have isaudio:0
    /// even when they carry a browse URL, so they remain navigable.
    pub fn is_navigable(&self) -> bool {
        self.has_items && !(self.url.is_some() && self.is_audio)
    }
}

pub struct LmsClient {
    client: Client,
    base_url: RwLock<String>,
    credentials: RwLock<Option<(String, String)>>,
}

impl LmsClient {
    pub fn new(base_url: String, credentials: Option<(String, String)>) -> Self {
        Self {
            client: Client::new(),
            base_url: RwLock::new(base_url),
            credentials: RwLock::new(credentials),
        }
    }

    /// Update the server URL in-place; background tasks pick it up on their next iteration.
    pub fn update_base_url(&self, url: String) {
        *self.base_url.write().unwrap() = url;
    }

    /// Update credentials in-place; background tasks pick them up on their next iteration.
    pub fn update_credentials(&self, credentials: Option<(String, String)>) {
        *self.credentials.write().unwrap() = credentials;
    }

    pub fn server_base_url(&self) -> String {
        let url = self.base_url.read().unwrap().clone();
        url.trim_end_matches("/jsonrpc.js").to_string()
    }

    async fn rpc(&self, player_id: &str, params: &[Value]) -> Result<Value> {
        // Clone URL and credentials before any await so we don't hold locks across await points.
        let url = self.base_url.read().unwrap().clone();
        let creds = self.credentials.read().unwrap().clone();
        let body = json!({
            "id": 1,
            "method": "slim.request",
            "params": [player_id, params]
        });
        let mut req = self.client.post(&url).json(&body);
        if let Some((user, pass)) = creds {
            req = req.basic_auth(user, Some(pass));
        }
        let resp = req.send().await?.json::<Value>().await?;
        Ok(resp["result"].clone())
    }

    pub async fn get_players(&self) -> Result<Vec<Player>> {
        let mut result = self
            .rpc("", &[json!("players"), json!(0), json!(100)])
            .await?;
        let count = result["count"].as_u64().unwrap_or(0);
        if count == 0 {
            return Ok(vec![]);
        }
        let players: Vec<Player> = serde_json::from_value(result["players_loop"].take())?;
        Ok(players)
    }

    pub async fn get_now_playing(&self, player_id: &str) -> Result<NowPlaying> {
        let result = self
            .rpc(
                player_id,
                &[json!("status"), json!("-"), json!(1), json!("tags:adltuKy")],
            )
            .await?;

        let track = result["playlist_loop"]
            .as_array()
            .and_then(|a| a.first())
            .cloned()
            .unwrap_or(json!({}));

        let base = self.server_base_url();
        let artwork_url = cover_url(&track, &base);

        Ok(NowPlaying {
            title: track["title"].as_str().unwrap_or("").to_string(),
            artist: track["artist"].as_str().unwrap_or("").to_string(),
            album: track["album"].as_str().unwrap_or("").to_string(),
            year: as_u64_loose(&track["year"])
                .filter(|&y| y > 0)
                .map(|y| y as u32),
            duration: track["duration"].as_f64().unwrap_or(0.0),
            elapsed: result["time"].as_f64().unwrap_or(0.0),
            volume: result["mixer volume"].as_f64().unwrap_or(0.0) as u8,
            is_playing: result["mode"].as_str() == Some("play"),
            shuffle: result["playlist shuffle"].as_u64().unwrap_or(0) as u8,
            repeat: result["playlist repeat"].as_u64().unwrap_or(0) as u8,
            artwork_url,
            playlist_cur_index: as_u64_loose(&result["playlist_cur_index"]).map(|i| i as usize),
            playlist_tracks: as_u64_loose(&result["playlist_tracks"]).map(|i| i as usize),
            playlist_timestamp: result["playlist_timestamp"].as_f64().unwrap_or(0.0),
        })
    }

    pub async fn get_queue(&self, player_id: &str) -> Result<Vec<Track>> {
        let mut result = self
            .rpc(
                player_id,
                &[json!("status"), json!(0), json!(500), json!("tags:adltK")],
            )
            .await?;
        let tracks = result["playlist_loop"]
            .as_array_mut()
            .map(std::mem::take)
            .unwrap_or_default();
        let base = self.server_base_url();
        Ok(tracks
            .into_iter()
            .map(|v| {
                let id: Option<Value> = if v["id"].is_null() {
                    None
                } else {
                    Some(v["id"].clone())
                };
                let artwork_url = cover_url(&v, &base);
                Track {
                    id,
                    title: v["title"].as_str().unwrap_or("Unknown").to_string(),
                    artist: v["artist"].as_str().map(String::from),
                    album: v["album"].as_str().map(String::from),
                    duration: v["duration"].as_f64(),
                    artwork_url,
                }
            })
            .collect())
    }

    pub async fn get_playlists(&self) -> Result<Vec<Playlist>> {
        let mut result = self
            .rpc("", &[json!("playlists"), json!(0), json!(10000)])
            .await?;
        let playlists: Vec<Playlist> = result["playlists_loop"]
            .as_array_mut()
            .map(std::mem::take)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| {
                let name = v["playlist"].as_str()?.to_string();
                let id = json!(v["id"].as_u64()?);
                Some(Playlist { id, name })
            })
            .collect();
        Ok(playlists)
    }

    pub async fn get_artists(&self) -> Result<Vec<Artist>> {
        let mut result = self
            .rpc("", &[json!("artists"), json!(0), json!(10000)])
            .await?;
        let artists: Vec<Artist> =
            serde_json::from_value(result["artists_loop"].take()).unwrap_or_default();
        Ok(artists)
    }

    pub async fn get_album_artists(&self) -> Result<Vec<Artist>> {
        let mut result = self
            .rpc(
                "",
                &[
                    json!("artists"),
                    json!(0),
                    json!(10000),
                    json!("role_id:ALBUMARTIST"),
                ],
            )
            .await?;
        let artists: Vec<Artist> =
            serde_json::from_value(result["artists_loop"].take()).unwrap_or_default();
        Ok(artists)
    }

    pub async fn get_albums(&self, artist_id: Option<&str>) -> Result<Vec<Album>> {
        let mut params = vec![json!("albums"), json!(0), json!(10000), json!("tags:al")];
        if let Some(id) = artist_id {
            params.push(json!(format!("artist_id:{}", id)));
        }
        let mut result = self.rpc("", &params).await?;
        let albums: Vec<Album> =
            serde_json::from_value(result["albums_loop"].take()).unwrap_or_default();
        Ok(albums)
    }

    pub async fn get_all_tracks(&self) -> Result<Vec<Track>> {
        let mut result = self
            .rpc(
                "",
                &[json!("tracks"), json!(0), json!(10000), json!("tags:adlt")],
            )
            .await?;
        let tracks: Vec<Track> =
            serde_json::from_value(result["titles_loop"].take()).unwrap_or_default();
        Ok(tracks)
    }

    pub async fn get_tracks(&self, album_id: &str) -> Result<Vec<Track>> {
        let mut result = self
            .rpc(
                "",
                &[
                    json!("tracks"),
                    json!(0),
                    json!(10000),
                    json!(format!("album_id:{}", album_id)),
                    json!("tags:adlt"),
                ],
            )
            .await?;
        let tracks: Vec<Track> =
            serde_json::from_value(result["titles_loop"].take()).unwrap_or_default();
        Ok(tracks)
    }

    // Playback controls
    pub async fn play(&self, player_id: &str) -> Result<()> {
        self.rpc(player_id, &[json!("play")]).await?;
        Ok(())
    }

    pub async fn pause(&self, player_id: &str) -> Result<()> {
        self.rpc(player_id, &[json!("pause")]).await?;
        Ok(())
    }

    pub async fn stop(&self, player_id: &str) -> Result<()> {
        self.rpc(player_id, &[json!("stop")]).await?;
        Ok(())
    }

    pub async fn next(&self, player_id: &str) -> Result<()> {
        self.rpc(player_id, &[json!("playlist"), json!("index"), json!("+1")])
            .await?;
        Ok(())
    }

    pub async fn prev(&self, player_id: &str) -> Result<()> {
        self.rpc(player_id, &[json!("playlist"), json!("index"), json!("-1")])
            .await?;
        Ok(())
    }

    pub async fn seek(&self, player_id: &str, seconds: f64) -> Result<()> {
        self.rpc(player_id, &[json!("time"), json!(seconds.max(0.0).floor())])
            .await?;
        Ok(())
    }

    pub async fn set_volume(&self, player_id: &str, volume: u8) -> Result<()> {
        self.rpc(player_id, &[json!("mixer"), json!("volume"), json!(volume)])
            .await?;
        Ok(())
    }

    /// Fetch volume and sync-group info for a player in a single RPC call.
    pub async fn get_player_status_info(&self, player_id: &str) -> Result<(u8, Vec<String>)> {
        let result = self
            .rpc(player_id, &[json!("status"), json!("-"), json!(0)])
            .await?;
        let volume = result["mixer volume"].as_f64().unwrap_or(0.0) as u8;
        let mut ids = vec![];
        if let Some(master) = result["sync_master"]
            .as_str()
            .filter(|s| !s.is_empty() && *s != player_id)
        {
            ids.push(master.to_string());
        }
        if let Some(slaves) = result["sync_slaves"].as_str().filter(|s| !s.is_empty()) {
            ids.extend(
                slaves
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty() && s != player_id),
            );
        }
        ids.sort();
        ids.dedup();
        Ok((volume, ids))
    }

    pub async fn sync_with(&self, player_id: &str, target_id: &str) -> Result<()> {
        self.rpc(player_id, &[json!("sync"), json!(target_id)])
            .await?;
        Ok(())
    }

    pub async fn unsync(&self, player_id: &str) -> Result<()> {
        self.rpc(player_id, &[json!("sync"), json!("-")]).await?;
        Ok(())
    }

    pub async fn set_power(&self, player_id: &str, on: bool) -> Result<()> {
        self.rpc(player_id, &[json!("power"), json!(if on { 1 } else { 0 })])
            .await?;
        Ok(())
    }

    pub async fn set_shuffle(&self, player_id: &str, value: u8) -> Result<()> {
        self.rpc(
            player_id,
            &[json!("playlist"), json!("shuffle"), json!(value)],
        )
        .await?;
        Ok(())
    }

    pub async fn set_repeat(&self, player_id: &str, value: u8) -> Result<()> {
        self.rpc(
            player_id,
            &[json!("playlist"), json!("repeat"), json!(value)],
        )
        .await?;
        Ok(())
    }

    async fn playlistcontrol(
        &self,
        player_id: &str,
        cmd: &str,
        item_type: &str,
        id: &str,
    ) -> Result<()> {
        self.rpc(
            player_id,
            &[
                json!("playlistcontrol"),
                json!(format!("cmd:{}", cmd)),
                json!(format!("{}:{}", item_type, id)),
            ],
        )
        .await?;
        Ok(())
    }

    pub async fn play_track(&self, player_id: &str, track_id: &str) -> Result<()> {
        self.playlistcontrol(player_id, "load", "track_id", track_id)
            .await
    }

    pub async fn play_album(&self, player_id: &str, album_id: &str) -> Result<()> {
        self.playlistcontrol(player_id, "load", "album_id", album_id)
            .await
    }

    pub async fn play_artist(&self, player_id: &str, artist_id: &str) -> Result<()> {
        self.playlistcontrol(player_id, "load", "artist_id", artist_id)
            .await
    }

    pub async fn play_folder(&self, player_id: &str, folder_id: u32) -> Result<()> {
        self.playlistcontrol(player_id, "load", "folder_id", &folder_id.to_string())
            .await
    }

    pub async fn play_track_index(&self, player_id: &str, index: usize) -> Result<()> {
        self.rpc(
            player_id,
            &[json!("playlist"), json!("index"), json!(index)],
        )
        .await?;
        Ok(())
    }

    /// Returns the list of installed apps (Spotify, Deezer, etc.).
    pub async fn get_apps(&self) -> Result<Vec<RadioItem>> {
        let result = self.rpc("", &[json!("apps"), json!(0), json!(100)]).await?;
        let items = result["appss_loop"].as_array().cloned().unwrap_or_default();
        let base = self.server_base_url();
        Ok(items
            .into_iter()
            .map(|v| RadioItem {
                name: v["name"].as_str().unwrap_or("").to_string(),
                item_type: "link".to_string(),
                url: None,
                cmd: v["cmd"].as_str().map(String::from),
                item_id: None,
                artwork_url: resolve_image_url(&v, &base),
                has_items: true,
                is_audio: false,
            })
            .collect())
    }

    /// Returns the list of available radio services/plugins (TuneIn, etc.).
    pub async fn get_radio_services(&self) -> Result<Vec<RadioItem>> {
        let result = self
            .rpc("", &[json!("radios"), json!(0), json!(100)])
            .await?;
        let items = result["radioss_loop"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let base = self.server_base_url();
        Ok(items
            .into_iter()
            .map(|v| RadioItem {
                name: v["name"].as_str().unwrap_or("").to_string(),
                item_type: "link".to_string(),
                url: None,
                cmd: v["cmd"].as_str().map(String::from),
                item_id: None,
                artwork_url: resolve_image_url(&v, &base),
                has_items: true,
                is_audio: false,
            })
            .collect())
    }

    /// Browse into a radio service or subfolder.
    /// `cmd` is the XMLBrowser plugin command (e.g. "tunein").
    /// `item_id` is the opaque id to navigate into, or None for the top of that service.
    pub async fn browse_radio(
        &self,
        player_id: &str,
        cmd: &str,
        item_id: Option<&str>,
    ) -> Result<Vec<RadioItem>> {
        let mut params = vec![
            json!(cmd),
            json!("items"),
            json!(0),
            json!(200),
            json!("want_url:1"),
        ];
        if let Some(id) = item_id {
            params.push(json!(format!("item_id:{}", id)));
        }
        let result = self.rpc(player_id, &params).await?;
        let items = browse_loop(&result);
        let base = self.server_base_url();
        Ok(items
            .into_iter()
            .map(|v| parse_browse_item(&v, &base, cmd))
            .collect())
    }

    pub async fn play_url_with_title(&self, player_id: &str, url: &str, title: &str) -> Result<()> {
        self.rpc(
            player_id,
            &[json!("playlist"), json!("play"), json!(url), json!(title)],
        )
        .await?;
        Ok(())
    }

    pub async fn add_track_to_queue(&self, player_id: &str, track_id: &str) -> Result<()> {
        self.playlistcontrol(player_id, "add", "track_id", track_id)
            .await
    }

    pub async fn add_album_to_queue(&self, player_id: &str, album_id: &str) -> Result<()> {
        self.playlistcontrol(player_id, "add", "album_id", album_id)
            .await
    }

    pub async fn add_artist_to_queue(&self, player_id: &str, artist_id: &str) -> Result<()> {
        self.playlistcontrol(player_id, "add", "artist_id", artist_id)
            .await
    }

    pub async fn add_folder_to_queue(&self, player_id: &str, folder_id: u32) -> Result<()> {
        self.playlistcontrol(player_id, "add", "folder_id", &folder_id.to_string())
            .await
    }

    pub async fn clear_queue(&self, player_id: &str) -> Result<()> {
        self.rpc(player_id, &[json!("playlist"), json!("clear")])
            .await?;
        Ok(())
    }

    pub async fn delete_queue_item(&self, player_id: &str, index: usize) -> Result<()> {
        self.rpc(
            player_id,
            &[json!("playlist"), json!("delete"), json!(index)],
        )
        .await?;
        Ok(())
    }

    pub async fn add_url_with_title_to_queue(
        &self,
        player_id: &str,
        url: &str,
        title: &str,
    ) -> Result<()> {
        self.rpc(
            player_id,
            &[json!("playlist"), json!("add"), json!(url), json!(title)],
        )
        .await?;
        Ok(())
    }

    pub async fn browse_music_folder(&self, folder_id: Option<u32>) -> Result<Vec<FolderItem>> {
        let mut params = vec![
            json!("musicfolder"),
            json!(0),
            json!(10000),
            json!("tags:dlt"),
        ];
        if let Some(id) = folder_id {
            params.push(json!(format!("folder_id:{}", id)));
        }
        let result = self.rpc("", &params).await?;
        let items = result["folder_loop"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        Ok(items
            .into_iter()
            .filter_map(|v| {
                let id = v["id"].as_u64()? as u32;
                let filename = v["filename"].as_str()?.to_string();
                let item_type = match v["type"].as_str() {
                    Some("track") => FolderItemType::Track,
                    _ => FolderItemType::Folder,
                };
                let duration = v["duration"].as_f64().filter(|&d| d > 0.0);
                Some(FolderItem {
                    id,
                    filename,
                    item_type,
                    duration,
                })
            })
            .collect())
    }

    pub async fn insert_track_next(&self, player_id: &str, track_id: &str) -> Result<()> {
        self.playlistcontrol(player_id, "insert", "track_id", track_id)
            .await
    }

    pub async fn insert_url_next_with_title(
        &self,
        player_id: &str,
        url: &str,
        title: &str,
    ) -> Result<()> {
        self.rpc(
            player_id,
            &[json!("playlist"), json!("insert"), json!(url), json!(title)],
        )
        .await?;
        Ok(())
    }

    pub async fn add_to_favorites(&self, player_id: &str, url: &str, title: &str) -> Result<()> {
        self.rpc(
            player_id,
            &[
                json!("favorites"),
                json!("add"),
                json!(format!("url:{}", url)),
                json!(format!("title:{}", title)),
            ],
        )
        .await?;
        Ok(())
    }

    /// Search within a single LMS app/plugin using the XMLBrowser items API with a search filter.
    pub async fn search_app(
        &self,
        player_id: &str,
        cmd: &str,
        query: &str,
    ) -> Result<Vec<RadioItem>> {
        self.search_app_via_item(player_id, cmd, None, query).await
    }

    /// Search via a plugin's dedicated "New Search" item.
    /// Plugins like Spotty require `item_id:{search_item_id} search:{query}` rather
    /// than a bare `search:query` at the top level.
    pub async fn search_app_via_item(
        &self,
        player_id: &str,
        cmd: &str,
        item_id: Option<&str>,
        query: &str,
    ) -> Result<Vec<RadioItem>> {
        let mut params = vec![
            json!(cmd),
            json!("items"),
            json!(0),
            json!(200),
            json!(format!("search:{}", query)),
            json!("want_url:1"),
        ];
        if let Some(id) = item_id {
            params.push(json!(format!("item_id:{}", id)));
        }
        let result = self.rpc(player_id, &params).await?;
        let base = self.server_base_url();
        Ok(browse_loop(&result)
            .into_iter()
            .map(|v| parse_browse_item(&v, &base, cmd))
            .filter(|item| !item.name.is_empty())
            .collect())
    }

    pub async fn search_library(
        &self,
        query: &str,
    ) -> Result<(Vec<Artist>, Vec<Album>, Vec<Track>, Vec<Playlist>)> {
        // Try FTS (full-text search) command first — fast when the index exists.
        if let Ok(result) = self
            .rpc(
                "",
                &[
                    json!("search"),
                    json!(0),
                    json!(200),
                    json!(format!("term:{}", query)),
                ],
            )
            .await
            && !result.is_null()
        {
            let raw_artists = result["contributors_loop"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let artists: Vec<Artist> = raw_artists
                .into_iter()
                .filter_map(|v| {
                    let name = v["contributor"].as_str()?.to_string();
                    let id = v["contributor_id"].clone();
                    if id.is_null() {
                        return None;
                    }
                    Some(Artist { id, artist: name })
                })
                .collect();

            let raw_albums = result["albums_loop"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let albums: Vec<Album> = raw_albums
                .into_iter()
                .filter_map(|v| {
                    let name = v["album"].as_str()?.to_string();
                    let id = v["album_id"].clone();
                    if id.is_null() {
                        return None;
                    }
                    Some(Album {
                        id,
                        album: name,
                        artist: v["artist"].as_str().map(String::from),
                    })
                })
                .collect();

            let raw_tracks = result["tracks_loop"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let tracks: Vec<Track> = raw_tracks
                .into_iter()
                .filter_map(|v| {
                    let title = v["track"].as_str()?.to_string();
                    let id = v["track_id"].clone();
                    if id.is_null() {
                        return None;
                    }
                    Some(Track {
                        id: Some(id),
                        title,
                        artist: v["artist"].as_str().map(String::from),
                        album: v["album"].as_str().map(String::from),
                        duration: v["duration"].as_f64(),
                        artwork_url: None,
                    })
                })
                .collect();

            let raw_playlists = result["playlists_loop"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let playlists: Vec<Playlist> = raw_playlists
                .into_iter()
                .filter_map(|v| {
                    let name = v["playlist"].as_str()?.to_string();
                    let id = v["playlist_id"].clone();
                    if id.is_null() {
                        return None;
                    }
                    Some(Playlist { id, name })
                })
                .collect();

            return Ok((artists, albums, tracks, playlists));
        }

        // FTS unavailable or returned null — fall back to fetching all items and
        // filtering by name client-side.  All four fetches run concurrently.
        let lower = query.to_lowercase();
        let (art_res, alb_res, trk_res, pls_res) = tokio::join!(
            self.get_artists(),
            self.get_albums(None),
            self.get_all_tracks(),
            self.get_playlists(),
        );

        const LIMIT: usize = 200;

        let artists: Vec<Artist> = art_res
            .unwrap_or_default()
            .into_iter()
            .filter(|a| a.artist.to_lowercase().contains(&lower))
            .take(LIMIT)
            .collect();

        let albums: Vec<Album> = alb_res
            .unwrap_or_default()
            .into_iter()
            .filter(|a| {
                a.album.to_lowercase().contains(&lower)
                    || a.artist
                        .as_deref()
                        .is_some_and(|ar| ar.to_lowercase().contains(&lower))
            })
            .take(LIMIT)
            .collect();

        let tracks: Vec<Track> = trk_res
            .unwrap_or_default()
            .into_iter()
            .filter(|t| {
                t.title.to_lowercase().contains(&lower)
                    || t.artist
                        .as_deref()
                        .is_some_and(|ar| ar.to_lowercase().contains(&lower))
                    || t.album
                        .as_deref()
                        .is_some_and(|al| al.to_lowercase().contains(&lower))
            })
            .take(LIMIT)
            .collect();

        let playlists: Vec<Playlist> = pls_res
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.name.to_lowercase().contains(&lower))
            .take(LIMIT)
            .collect();

        Ok((artists, albums, tracks, playlists))
    }

    pub async fn play_playlist(&self, player_id: &str, playlist_id: &str) -> Result<()> {
        self.playlistcontrol(player_id, "load", "playlist_id", playlist_id)
            .await
    }

    pub async fn add_playlist_to_queue(&self, player_id: &str, playlist_id: &str) -> Result<()> {
        self.playlistcontrol(player_id, "add", "playlist_id", playlist_id)
            .await
    }

    pub async fn insert_playlist_next(&self, player_id: &str, playlist_id: &str) -> Result<()> {
        self.playlistcontrol(player_id, "insert", "playlist_id", playlist_id)
            .await
    }

    pub async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let creds = self.credentials.read().unwrap().clone();
        let mut req = self.client.get(url);
        if let Some((user, pass)) = creds {
            req = req.basic_auth(user, Some(pass));
        }
        let bytes = req.send().await?.bytes().await?;
        Ok(bytes.to_vec())
    }

    pub async fn server_status(&self) -> Result<()> {
        let result = self
            .rpc("", &[json!("serverstatus"), json!(0), json!(0)])
            .await?;
        if result.is_null() {
            return Err(anyhow!("no response from server"));
        }
        Ok(())
    }

    pub async fn get_server_info(&self) -> Result<ServerInfo> {
        let result = self
            .rpc("", &[json!("serverstatus"), json!(0), json!(0)])
            .await?;
        if result.is_null() {
            return Err(anyhow!("no response from server"));
        }
        Ok(serde_json::from_value(result)?)
    }

    pub async fn get_players_detailed(&self) -> Result<Vec<PlayerDetail>> {
        let mut result = self
            .rpc("", &[json!("players"), json!(0), json!(100)])
            .await?;
        let count = result["count"].as_u64().unwrap_or(0);
        if count == 0 {
            return Ok(vec![]);
        }
        let players: Vec<PlayerDetail> = serde_json::from_value(result["players_loop"].take())?;
        Ok(players)
    }

    /// Fetch the currently-playing cover art URL for a Radio Paradise channel.
    /// `chan`: 0=Main Mix, 1=Mellow Mix, 2=Rock Mix, 3=World Mix.
    pub async fn fetch_radio_paradise_art_url(&self, chan: u8) -> Result<String> {
        let url = format!(
            "https://api.radioparadise.com/api/now_playing?chan={}",
            chan
        );
        let resp: Value = self.client.get(&url).send().await?.json().await?;
        resp["cover"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(String::from)
            .ok_or_else(|| anyhow::anyhow!("no cover in RP response"))
    }
}

/// Build a `RadioItem` from a browse/search result JSON object.
/// `default_cmd` is used as the fallback when the server doesn't include a `cmd` field.
fn parse_browse_item(v: &Value, base: &str, default_cmd: &str) -> RadioItem {
    RadioItem {
        name: v["name"].as_str().unwrap_or("").to_string(),
        item_type: v["type"].as_str().unwrap_or("link").to_string(),
        url: v["url"].as_str().map(String::from),
        cmd: v["cmd"]
            .as_str()
            .map(String::from)
            .or_else(|| Some(default_cmd.to_string())),
        item_id: crate::utils::json_id_to_opt_string(&v["id"]),
        artwork_url: resolve_image_url(v, base),
        has_items: v["hasitems"].as_u64().unwrap_or(0) > 0,
        is_audio: v["isaudio"].as_u64().unwrap_or(0) > 0,
    }
}

/// Coerce a JSON value that may be a number or a numeric string into `u64`.
fn as_u64_loose(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// Extract the item array from an XMLBrowser response, returned by the server under either
/// `loop_loop` or `item_loop` depending on the plugin.
fn browse_loop(result: &Value) -> Vec<Value> {
    result["loop_loop"]
        .as_array()
        .or_else(|| result["item_loop"].as_array())
        .cloned()
        .unwrap_or_default()
}

/// Resolve a track/cover artwork URL from an LMS item JSON object: prefer an explicit
/// `artwork_url` (resolving a leading-`/` relative path against `base`), otherwise fall
/// back to `artwork_track_id`/`id` as `{base}/music/{id}/cover.jpg`.
fn cover_url(v: &Value, base: &str) -> Option<String> {
    if let Some(url) = v["artwork_url"].as_str().filter(|s| !s.is_empty()) {
        return Some(if url.starts_with('/') {
            format!("{}{}", base, url)
        } else {
            url.to_string()
        });
    }
    let cover_id = crate::utils::json_id_to_opt_string(&v["artwork_track_id"])
        .or_else(|| crate::utils::json_id_to_opt_string(&v["id"]));
    cover_id.map(|id| crate::utils::music_image_url(base, id, "cover.jpg"))
}

/// Extract an image URL from a JSON item value, resolving relative paths against `base`.
fn resolve_image_url(v: &Value, base: &str) -> Option<String> {
    let raw = v["image"]
        .as_str()
        .or_else(|| v["icon"].as_str())
        .or_else(|| v["artwork_url"].as_str())?;
    if raw.is_empty() {
        return None;
    }
    if raw.starts_with("http://") || raw.starts_with("https://") {
        Some(raw.to_string())
    } else if raw.starts_with('/') {
        Some(format!("{}{}", base, raw))
    } else {
        // relative path without leading slash (e.g. plugin icons like "plugins/Foo/icon.png")
        Some(format!("{}/{}", base, raw))
    }
}
