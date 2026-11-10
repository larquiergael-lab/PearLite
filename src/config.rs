use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const HOME_URL: &str = "https://music.youtube.com/";

/// Persistent settings, stored as JSON in `%APPDATA%\PearLite\config.json`.
/// Unknown/missing keys fall back to defaults so the file can be hand-edited safely.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// Hide to the tray instead of quitting when the window is closed.
    pub close_to_tray: bool,
    /// Strip ads from player responses and skip any that slip through.
    pub ad_block: bool,
    /// Show "Listening to ..." in Discord.
    pub discord_rpc: bool,
    pub discord_client_id: String,
    /// Reopen the last visited YouTube Music page on start.
    pub resume_last_url: bool,
    pub last_url: Option<String>,
    /// Ask WebView2 to trim memory while the window is hidden or minimized.
    pub low_memory_when_hidden: bool,
    /// GPU rendering and video decoding. Off by default: in testing that halved memory
    /// (~580 -> ~255 MB committed while playing) at similar CPU use.
    pub hardware_acceleration: bool,
    /// Extra Chromium switches appended verbatim, e.g. "--disable-gpu-vsync".
    pub extra_browser_args: String,
    pub devtools: bool,
    pub window: WindowState,
    /// Where the mini player was last left (physical pixels); `None` = next to the tray icon.
    pub mini_position: Option<(i32, i32)>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct WindowState {
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self { x: None, y: None, width: 1100, height: 760, maximized: false }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            close_to_tray: true,
            ad_block: true,
            discord_rpc: false,
            // Application registered by the pear-desktop team (shows as "YouTube Music").
            discord_client_id: "1177081335727267940".into(),
            resume_last_url: true,
            last_url: None,
            low_memory_when_hidden: true,
            hardware_acceleration: false,
            extra_browser_args: String::new(),
            devtools: false,
            window: WindowState::default(),
            mini_position: None,
        }
    }
}

/// `%APPDATA%\PearLite` — settings and optional user `custom.css` / `custom.js`.
pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| ".".into());
    base.join("PearLite")
}

/// `%LOCALAPPDATA%\PearLite` — WebView2 profile (cookies, cache, login).
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(config_dir);
    base.join("PearLite")
}

fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

impl Config {
    pub fn load() -> Self {
        let path = config_path();
        let Ok(text) = std::fs::read_to_string(&path) else { return Self::default() };
        // Notepad and PowerShell like to save UTF-8 with a BOM, which serde_json rejects.
        match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
            Ok(cfg) => cfg,
            Err(_) => {
                // Keep a hand-edited file with a typo instead of overwriting it on exit.
                let _ = std::fs::rename(&path, config_dir().join("config.invalid.json"));
                Self::default()
            }
        }
    }

    pub fn save(&self) {
        let dir = config_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(json) = serde_json::to_string_pretty(self) {
            // Write-then-rename so a crash mid-write never leaves a truncated config.
            let tmp = dir.join("config.json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, config_path());
            }
        }
    }

    /// URL to open at startup. A last `/watch` page is not restored: with autoplay allowed
    /// it would start blasting music on login.
    pub fn start_url(&self) -> String {
        match &self.last_url {
            Some(u) if self.resume_last_url && crate::win::is_ytm_url(u) && !u.contains("/watch") => u.clone(),
            _ => HOME_URL.into(),
        }
    }
}
