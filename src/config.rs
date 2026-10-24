use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

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
    #[serde(default)]
    pub full_art_mode: bool,
    #[serde(default)]
    pub disable_auto_colors: bool,
    #[serde(default = "default_accent_lightness")]
    pub accent_lightness: u8,
    #[serde(default = "default_image_protocol")]
    pub image_protocol: String,
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

fn default_accent_lightness() -> u8 {
    65
}

fn default_image_protocol() -> String {
    "auto".to_string()
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
            full_art_mode: false,
            disable_auto_colors: false,
            accent_lightness: default_accent_lightness(),
            image_protocol: default_image_protocol(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        Self::load_from(&config_path())
    }

    fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&config_path())
    }

    fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating config dir {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).with_context(|| "serializing config")?;
        fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
        // The file may hold a plaintext password — keep it owner-only.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                .with_context(|| format!("setting permissions on {}", path.display()))?;
        }
        Ok(())
    }

    pub fn base_url(&self) -> String {
        format!("http://{}:{}/jsonrpc.js", self.host, self.port)
    }

    /// Owned `(username, password)` pair when both are set, for `LmsClient` basic auth.
    pub fn credentials(&self) -> Option<(String, String)> {
        self.username
            .as_ref()
            .zip(self.password.as_ref())
            .map(|(u, p)| (u.clone(), p.clone()))
    }
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lyrtui")
        .join("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("lyrtui-test-{}-{}", std::process::id(), name))
            .join("config.toml")
    }

    #[test]
    fn missing_file_yields_defaults() {
        let cfg = Config::load_from(&temp_path("missing")).unwrap();
        assert_eq!(cfg.host, default_host());
        assert_eq!(cfg.port, default_port());
    }

    #[test]
    fn partial_file_fills_defaults() {
        let path = temp_path("partial");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "host = \"lms.local\"\n").unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.host, "lms.local");
        assert_eq!(cfg.port, default_port());
        assert_eq!(cfg.image_protocol, default_image_protocol());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn save_load_round_trip_and_private_perms() {
        let path = temp_path("roundtrip");
        let cfg = Config {
            host: "10.0.0.5".into(),
            port: 9001,
            username: Some("u".into()),
            password: Some("p".into()),
            ..Config::default()
        };
        cfg.save_to(&path).unwrap();
        let back = Config::load_from(&path).unwrap();
        assert_eq!(back.base_url(), "http://10.0.0.5:9001/jsonrpc.js");
        assert_eq!(back.credentials(), Some(("u".into(), "p".into())));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn credentials_need_both_fields() {
        let cfg = Config { username: Some("u".into()), ..Config::default() };
        assert_eq!(cfg.credentials(), None);
    }
}
