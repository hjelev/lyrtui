use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub default_player: Option<String>,
    #[serde(default)]
    pub use_nerd_icons: bool,
    pub username: Option<String>,
    pub password: Option<String>,
    #[serde(default = "default_auto_discover")]
    pub auto_discover: bool,
    #[serde(default = "default_broadcast_mask")]
    pub broadcast_mask: String,
    #[serde(default)]
    pub global_volume_control: bool,
}

fn default_host() -> String {
    "localhost".to_string()
}

fn default_port() -> u16 {
    9000
}

fn default_auto_discover() -> bool {
    true
}

fn default_broadcast_mask() -> String {
    "255.255.255.255".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            default_player: None,
            use_nerd_icons: false,
            username: None,
            password: None,
            auto_discover: default_auto_discover(),
            broadcast_mask: default_broadcast_mask(),
            global_volume_control: false,
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating config dir {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self)
            .with_context(|| "serializing config")?;
        fs::write(&path, text)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    pub fn base_url(&self) -> String {
        format!("http://{}:{}/jsonrpc.js", self.host, self.port)
    }
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lyrtui")
        .join("config.toml")
}
