use crate::api::{Album, Artist, NowPlaying, Player, RadioItem, Track};

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionState {
    Connected,
    Disconnected,
    Reconnecting,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SidebarItem {
    Artists,
    Albums,
    Tracks,
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
    Albums { artist_id: Option<String> },
    Tracks { album_id: Option<String> }, // None = all tracks
}

#[derive(Debug, Clone)]
pub enum MainView {
    Library(LibraryView),
    Queue,
    Players,
    Radio,
    Apps,
    Favourites,
    Help,
}

#[derive(Debug, Clone)]
pub struct ConfigModal {
    pub host: String,
    pub port: String,
    pub selected_field: usize, // 0 = host, 1 = port
    pub editing: bool,
    pub error: Option<String>,
}

impl ConfigModal {
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            host: host.to_string(),
            port: port.to_string(),
            selected_field: 0,
            editing: false,
            error: None,
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
pub struct ContextMenu {
    pub selected: usize,
}

impl ContextMenu {
    pub fn new() -> Self {
        Self { selected: 0 }
    }

    pub fn option_count() -> usize {
        4
    }

    pub fn options() -> [&'static str; 4] {
        ["Play now", "Play next", "Add to end of queue", "Add to favourites"]
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
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,

    // Radio data
    pub radio_items: Vec<RadioItem>,
    pub radio_nav_stack: Vec<RadioNav>,
    pub radio_title: String,

    // Apps data
    pub app_items: Vec<RadioItem>,
    pub app_nav_stack: Vec<RadioNav>,
    pub app_title: String,

    // Favourites data
    pub fav_items: Vec<RadioItem>,
    pub fav_nav_stack: Vec<RadioNav>,
    pub fav_title: String,

    // UI state
    pub sidebar_selected: usize,
    pub main_selected: usize,
    pub sidebar_items: Vec<SidebarItem>,
    pub main_view: MainView,
    pub focus_sidebar: bool,

    pub status_message: Option<String>,
    pub config_modal: Option<ConfigModal>,
    pub context_menu: Option<ContextMenu>,
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
            albums: vec![],
            tracks: vec![],
            radio_items: vec![],
            radio_nav_stack: vec![],
            radio_title: "Radio".to_string(),
            app_items: vec![],
            app_nav_stack: vec![],
            app_title: "Apps".to_string(),
            fav_items: vec![],
            fav_nav_stack: vec![],
            fav_title: "Favourites".to_string(),
            sidebar_selected: 0,
            main_selected: 0,
            sidebar_items: vec![
                SidebarItem::Artists,
                SidebarItem::Albums,
                SidebarItem::Tracks,
                SidebarItem::Radio,
                SidebarItem::Apps,
                SidebarItem::Favourites,
                SidebarItem::Queue,
                SidebarItem::Players,
                SidebarItem::Help,
            ],
            main_view: MainView::Library(LibraryView::Artists),
            focus_sidebar: true,
            status_message: None,
            config_modal: None,
            context_menu: None,
        }
    }

    pub fn sidebar_label(&self, item: &SidebarItem) -> &'static str {
        match item {
            SidebarItem::Artists => "Artists",
            SidebarItem::Albums => "Albums",
            SidebarItem::Tracks => "Tracks",
            SidebarItem::Radio => "Radio",
            SidebarItem::Apps => "Apps",
            SidebarItem::Favourites => "Favourites",
            SidebarItem::Queue => "Queue",
            SidebarItem::Players => "Players",
            SidebarItem::Help => "Help  ?",
        }
    }

    pub fn is_playing(&self) -> bool {
        self.now_playing.as_ref().map(|n| n.is_playing).unwrap_or(false)
    }
}

// Messages sent from background tasks to the app
pub enum AppMsg {
    Connected,
    Disconnected,
    PlayersLoaded(Vec<Player>),
    NowPlayingUpdated(NowPlaying),
    QueueLoaded(Vec<Track>),
    ArtistsLoaded(Vec<Artist>),
    AlbumsLoaded(Vec<Album>),
    TracksLoaded(Vec<Track>),
    RadioItemsLoaded(Vec<RadioItem>),
    AppItemsLoaded(Vec<RadioItem>),
    FavItemsLoaded(Vec<RadioItem>),
    ArtworkLoaded(Vec<u8>),
    StatusMsg(String),
    #[allow(dead_code)]
    Error(String),
}
