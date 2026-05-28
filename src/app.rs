use crate::api::{Album, Artist, FolderItem, NowPlaying, Player, Playlist, RadioItem, Track};
use std::cell::Cell;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionState {
    Connected,
    Disconnected,
    Reconnecting,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SidebarItem {
    MyMusic,
    Search,
    Radio,
    Apps,
    Favourites,
    Queue,
    Players,
    Help,
}

#[derive(Debug, Clone)]
pub enum LibraryView {
    Artists,
    AlbumArtists,
    Albums { artist_id: Option<String> },
    Tracks { album_id: Option<String> }, // None = all tracks
    Folder { folder_id: Option<u32> },
    Playlists,
}

#[derive(Debug, Clone)]
pub enum MainView {
    Library(LibraryView),
    MyMusic,
    Queue,
    Players,
    Radio,
    Apps,
    Favourites,
    Help,
    Search,
    /// In-app search within a specific plugin (e.g. Spotty).
    /// `item_id` is the opaque ID of the "New Search" entry — used to
    /// build the correct LMS XMLBrowser request (item_id:X search:query).
    AppSearch { cmd: String, item_id: Option<String> },
}

#[derive(Debug, Clone)]
pub enum SearchResultItem {
    Artist(Artist),
    Album(Album),
    Track(Track),
    Playlist(Playlist),
    AppItem(RadioItem),
    RadioItem(RadioItem),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SearchScope {
    MyMusic,
    Radios,
    Apps,
    All,
}

#[derive(Debug, Clone)]
pub struct ConfigModal {
    pub host: String,
    pub port: String,
    pub username: String,
    pub password: String,
    pub use_nerd_icons: bool,
    pub auto_discover: bool,
    pub broadcast_mask: String,
    pub disable_auto_colors: bool,
    // 0=host, 1=port, 2=username, 3=password, 4=nerd_icons, 5=auto_discover, 6=broadcast_mask, 7=disable_auto_colors, 8=OK, 9=Cancel
    pub selected_field: usize,
    pub editing: bool,
    pub cursor_pos: usize,
    pub error: Option<String>,
}

impl ConfigModal {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        host: &str,
        port: u16,
        username: Option<&str>,
        password: Option<&str>,
        use_nerd_icons: bool,
        auto_discover: bool,
        broadcast_mask: &str,
        disable_auto_colors: bool,
    ) -> Self {
        Self {
            host: host.to_string(),
            port: port.to_string(),
            username: username.unwrap_or("").to_string(),
            password: password.unwrap_or("").to_string(),
            use_nerd_icons,
            auto_discover,
            broadcast_mask: broadcast_mask.to_string(),
            disable_auto_colors,
            selected_field: 0,
            editing: false,
            cursor_pos: 0,
            error: None,
        }
    }

    pub fn current_field_char_count(&self) -> usize {
        match self.selected_field {
            0 => self.host.chars().count(),
            1 => self.port.chars().count(),
            2 => self.username.chars().count(),
            3 => self.password.chars().count(),
            6 => self.broadcast_mask.chars().count(),
            _ => 0,
        }
    }

    pub fn current_field_str_mut(&mut self) -> Option<&mut String> {
        match self.selected_field {
            0 => Some(&mut self.host),
            1 => Some(&mut self.port),
            2 => Some(&mut self.username),
            3 => Some(&mut self.password),
            6 => Some(&mut self.broadcast_mask),
            _ => None,
        }
    }
}

/// One level of radio navigation history, saved so Back is instant (no refetch).
#[derive(Debug, Clone)]
pub struct RadioNav {
    /// Title of the level we're leaving (for restoring on Back).
    pub title: String,
    /// Items at this level, moved out and stored here while browsing deeper.
    pub items: Vec<RadioItem>,
    /// Cursor position to restore on Back.
    pub selected: usize,
}

#[derive(Debug, Clone)]
pub struct FolderNav {
    pub folder_id: Option<u32>,
    pub title: String,
    pub items: Vec<FolderItem>,
    pub selected: usize,
}

#[derive(Debug, Clone)]
pub struct ContextMenu {
    pub selected: usize,
    pub parent_add_label: Option<String>,
    pub parent_replace_label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SyncModal {
    pub player_id: String,
    pub player_name: String,
    pub other_players: Vec<Player>,
    pub checked: Vec<bool>,
    pub initial_synced_ids: Vec<String>,
    pub list_selected: usize,
    pub focus_buttons: bool,
    pub selected_button: u8, // 0 = Synchronize, 1 = Cancel
}

impl ContextMenu {
    pub fn new(parent_add_label: Option<String>, parent_replace_label: Option<String>) -> Self {
        Self { selected: 0, parent_add_label, parent_replace_label }
    }

    pub fn option_count(&self) -> usize {
        let parent_count = self.parent_add_label.is_some() as usize
            + self.parent_replace_label.is_some() as usize;
        6 + parent_count
    }

    pub fn options(&self) -> Vec<String> {
        let mut opts = vec![
            "Play now".to_string(),
            "Play next".to_string(),
            "Add to end of queue".to_string(),
            "Replace queue".to_string(),
            "Add to favourites".to_string(),
        ];
        if let Some(label) = &self.parent_add_label {
            opts.push(label.clone());
        }
        if let Some(label) = &self.parent_replace_label {
            opts.push(label.clone());
        }
        opts.push("Cancel".to_string());
        opts
    }
}

pub struct App {
    pub connection: ConnectionState,
    pub players: Vec<Player>,
    pub active_player: Option<String>,
    pub now_playing: Option<NowPlaying>,
    pub queue: Vec<Track>,

    // Library data
    pub artists: Vec<Artist>,
    pub album_artists: Vec<Artist>,
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
    pub playlists: Vec<Playlist>,

    // Radio data
    pub radio_items: Vec<RadioItem>,
    pub radio_nav_stack: Vec<RadioNav>,
    pub radio_title: String,

    // Apps data
    pub app_services: Vec<RadioItem>,  // top-level app list, never mutated during navigation
    pub app_items: Vec<RadioItem>,
    pub app_nav_stack: Vec<RadioNav>,
    pub app_title: String,

    // Favourites data
    pub fav_items: Vec<RadioItem>,
    pub fav_nav_stack: Vec<RadioNav>,
    pub fav_title: String,

    // Folder view data
    pub folder_items: Vec<FolderItem>,
    pub folder_nav_stack: Vec<FolderNav>,
    pub folder_title: String,

    // Per-player volumes (updated by background polling)
    pub player_volumes: HashMap<String, u8>,
    // Per-player sync group members (updated by background polling)
    pub player_sync_groups: HashMap<String, Vec<String>>,

    // UI state
    pub sidebar_selected: usize,
    pub main_selected: usize,
    pub sidebar_items: Vec<SidebarItem>,
    pub main_view: MainView,
    pub previous_view: Option<MainView>,
    pub focus_sidebar: bool,
    pub players_focus_global: bool,
    pub global_volume_control: bool,

    pub status_message: Option<String>,
    pub status_message_gen: u64,
    pub config_modal: Option<ConfigModal>,
    pub context_menu: Option<ContextMenu>,
    pub sync_modal: Option<SyncModal>,
    pub confirm_clear_queue: bool,
    pub clear_queue_selected_button: u8, // 0 = OK, 1 = Cancel
    pub confirm_delete_queue_item: Option<usize>, // Some(idx) when pending confirmation
    pub delete_queue_selected_button: u8, // 0 = OK, 1 = Cancel
    /// Height (in terminal rows) of the Now Playing panel, computed from font metrics.
    pub status_height: u16,
    /// Width (in terminal columns) of the album-art cell in the Now Playing panel.
    pub art_col_w: u16,

    // Search state
    pub search_query: String,
    pub search_results: Vec<SearchResultItem>,
    pub search_input_active: bool,
    pub search_scope: SearchScope,
    pub radio_services: Vec<RadioItem>,

    // In-app search state (e.g. Spotty search)
    pub app_search_query: String,
    pub app_search_results: Vec<RadioItem>,
    pub app_search_input_active: bool,

    pub use_nerd_icons: bool,
    pub full_art_mode: bool,
    pub accent_color: Option<[u8; 3]>,
    pub disable_auto_colors: bool,

    /// True while a background navigation fetch is in-flight (triggers loading indicator in UI).
    pub is_loading: bool,

    pub help_scroll: u16,
    /// Updated each frame by draw_help; used by handlers to clamp scrolling.
    pub help_visible_lines: Cell<u16>,

    /// Pixel dimensions of the current album art image (width, height).
    pub art_image_size: Option<(u32, u32)>,
    /// Terminal font size in pixels (width, height), set once from the picker.
    pub font_size: (u16, u16),

    pub now_playing_handle: Option<tokio::task::AbortHandle>,
}

impl App {
    pub fn new(default_player: Option<String>) -> Self {
        Self {
            connection: ConnectionState::Reconnecting,
            players: vec![],
            active_player: default_player,
            now_playing: None,
            queue: vec![],
            artists: vec![],
            album_artists: vec![],
            albums: vec![],
            tracks: vec![],
            playlists: vec![],
            radio_items: vec![],
            radio_nav_stack: vec![],
            radio_title: "Radio".to_string(),
            app_services: vec![],
            app_items: vec![],
            app_nav_stack: vec![],
            app_title: "Apps".to_string(),
            fav_items: vec![],
            fav_nav_stack: vec![],
            fav_title: "Favourites".to_string(),
            folder_items: vec![],
            folder_nav_stack: vec![],
            folder_title: "Folders".to_string(),
            player_volumes: HashMap::new(),
            player_sync_groups: HashMap::new(),
            sidebar_selected: 0,
            main_selected: 0,
            sidebar_items: vec![
                SidebarItem::MyMusic,
                SidebarItem::Search,
                SidebarItem::Radio,
                SidebarItem::Apps,
                SidebarItem::Favourites,
                SidebarItem::Queue,
                SidebarItem::Players,
                SidebarItem::Help,
            ],
            main_view: MainView::MyMusic,
            previous_view: None,
            focus_sidebar: true,
            players_focus_global: false,
            global_volume_control: false,
            status_message: None,
            status_message_gen: 0,
            config_modal: None,
            context_menu: None,
            sync_modal: None,
            confirm_clear_queue: false,
            clear_queue_selected_button: 0,
            confirm_delete_queue_item: None,
            delete_queue_selected_button: 0,
            status_height: 11, // overwritten in run() from picker font metrics
            art_col_w: 16,     // overwritten in run() from picker font metrics
            search_query: String::new(),
            search_results: vec![],
            search_input_active: false,
            search_scope: SearchScope::MyMusic,
            radio_services: vec![],
            app_search_query: String::new(),
            app_search_results: vec![],
            app_search_input_active: false,
            is_loading: false,
            use_nerd_icons: false,
            full_art_mode: false,
            accent_color: None,
            disable_auto_colors: false,
            help_scroll: 0,
            help_visible_lines: Cell::new(u16::MAX),
            art_image_size: None,
            font_size: (8, 16),
            now_playing_handle: None,
        }
    }

    pub fn sidebar_label(&self, item: &SidebarItem) -> &'static str {
        match item {
            SidebarItem::MyMusic => " My Music",
            SidebarItem::Search => " Search",
            SidebarItem::Radio => " Radio",
            SidebarItem::Apps => " Apps",
            SidebarItem::Favourites => " Favourites",
            SidebarItem::Queue => " Queue",
            SidebarItem::Players => " Players",
            SidebarItem::Help => " Help",
        }
    }

    pub fn is_playing(&self) -> bool {
        self.now_playing.as_ref().map(|n| n.is_playing).unwrap_or(false)
    }

    pub fn effective_accent(&self) -> Option<[u8; 3]> {
        if self.disable_auto_colors { None } else { self.accent_color }
    }
}

// Messages sent from background tasks to the app
pub enum AppMsg {
    Connected,
    Disconnected,
    PlayersLoaded(Vec<Player>),
    NowPlayingUpdated(String, NowPlaying), // player_id, data
    QueueLoaded(String, Vec<Track>),       // player_id, data
    ArtistsLoaded(Vec<Artist>),
    AlbumArtistsLoaded(Vec<Artist>),
    AlbumsLoaded(Vec<Album>),
    TracksLoaded(Vec<Track>),
    RadioItemsLoaded(Vec<RadioItem>),
    AppItemsLoaded(Vec<RadioItem>),
    FavItemsLoaded(Vec<RadioItem>),
    FolderItemsLoaded(Vec<FolderItem>),
    PlaylistsLoaded(Vec<Playlist>),
    ArtworkLoaded(Vec<u8>),
    ThumbnailLoaded(String, image::DynamicImage), // url, decoded image
    ThumbnailFailed(String),          // url
    PlayerVolumesLoaded(HashMap<String, u8>),
    PlayerSyncGroupsLoaded(HashMap<String, Vec<String>>),
    StatusMsg(String),
    ClearStatusMsg(u64),
    SearchResultsLoaded(Vec<SearchResultItem>),
    AppSearchResultsLoaded(Vec<RadioItem>),
    #[allow(dead_code)]
    Error(String),
}
