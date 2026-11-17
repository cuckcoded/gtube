use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SbMode {
    Skip,
    Show,
    Off,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub max_quality: u32,
    pub autoplay: bool,
    pub hwdec: bool,
    pub mark_watched: bool,
    pub hide_shorts: bool,
    pub volume: f64,
    pub audio_only: bool,
    pub sponsorblock: bool,
    pub sb_modes: HashMap<String, SbMode>,
    pub dislikes: bool,
    pub download_dir: String,
    pub download_sb_remove: bool,
    pub download_embed_metadata: bool,
    pub audio_format: String,
    pub language: String,
    pub region: String,
    /// "video" (YouTube) or "music" (YouTube Music).
    pub ui_mode: String,
    /// Music mode plays the video instead of just the song.
    pub music_video: bool,
    pub music_shuffle: bool,
    /// "off", "all" or "one".
    pub music_repeat: String,
    /// Tint Music mode with the colors of the album art.
    pub music_ambient: bool,
    /// The spectrum visualizer behind the Music player bar.
    pub music_visualizer: bool,
    /// Continue videos from where they were left.
    pub resume_playback: bool,
    /// Private SponsorBlock user ID for submissions (made on first use).
    pub sb_user_id: String,
    /// Size of the compact player window.
    pub compact_width: i32,
    pub compact_height: i32,
    /// The main window's size (when not maximized) and maximized state.
    pub window_width: i32,
    pub window_height: i32,
    pub window_maximized: bool,
    /// What the immersive view shows beside the artwork: "lyrics", "queue"
    /// or "" (nothing).
    pub immersive_panel: String,
    /// Ask LRCLIB for timed lyrics when YouTube Music has none.
    pub lyrics_lrclib: bool,
    /// Show subtitles for videos that have them.
    pub subtitles: bool,
    /// The subtitle language wanted (the last one picked); "" for `language`.
    pub subtitle_lang: String,
    /// Without subtitles in that language, show YouTube's translation.
    pub subtitle_translate: bool,
    /// Subtitle size (mpv's `sub-scale`).
    pub subtitle_scale: f64,
    /// The watch page's player spans the whole width (theater mode).
    pub theater: bool,
}

fn system_locale() -> (String, String) {
    let lang = std::env::var("LC_ALL")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("LC_MESSAGES").ok().filter(|s| !s.is_empty()))
        .or_else(|| std::env::var("LANG").ok())
        .unwrap_or_else(|| "en_US".into());
    let base = lang.split(['.', '@']).next().unwrap_or("en_US");
    let mut parts = base.split('_');
    let hl = parts.next().filter(|s| s.len() == 2).unwrap_or("en").to_lowercase();
    let gl = parts.next().filter(|s| s.len() == 2).unwrap_or("US").to_uppercase();
    if hl == "c" || hl == "posix" { ("en".into(), "US".into()) } else { (hl, gl) }
}

impl Default for Settings {
    fn default() -> Self {
        let (hl, gl) = system_locale();
        let mut sb_modes = HashMap::new();
        for c in ["sponsor", "selfpromo", "interaction"] {
            sb_modes.insert(c.to_string(), SbMode::Skip);
        }
        for c in ["intro", "outro", "preview", "hook", "music_offtopic", "filler", "poi_highlight", "exclusive_access"] {
            sb_modes.insert(c.to_string(), SbMode::Show);
        }
        let dl = dirs::video_dir()
            .or_else(dirs::download_dir)
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
            .join("GTube");
        Self {
            max_quality: 1080,
            autoplay: true,
            hwdec: true,
            mark_watched: true,
            hide_shorts: true,
            volume: 100.0,
            audio_only: false,
            sponsorblock: true,
            sb_modes,
            dislikes: true,
            download_dir: dl.to_string_lossy().into_owned(),
            download_sb_remove: false,
            download_embed_metadata: true,
            audio_format: "mp3".into(),
            language: hl,
            region: gl,
            ui_mode: "video".into(),
            music_video: false,
            music_shuffle: false,
            music_repeat: "off".into(),
            music_ambient: true,
            music_visualizer: true,
            resume_playback: true,
            sb_user_id: String::new(),
            compact_width: 480,
            compact_height: 270,
            window_width: 1440,
            window_height: 900,
            window_maximized: false,
            immersive_panel: "lyrics".into(),
            lyrics_lrclib: true,
            subtitles: false,
            subtitle_lang: String::new(),
            subtitle_translate: false,
            subtitle_scale: 1.0,
            theater: false,
        }
    }
}

impl Settings {
    pub fn sb_mode(&self, cat: &str) -> SbMode {
        self.sb_modes.get(cat).copied().unwrap_or(SbMode::Show)
    }

    /// The subtitle language wanted.
    pub fn subtitle_language(&self) -> &str {
        if self.subtitle_lang.is_empty() { &self.language } else { &self.subtitle_lang }
    }
}

pub fn config_dir() -> PathBuf {
    let d = dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("gtube");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn data_dir() -> PathBuf {
    let d = dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("gtube");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn cache_dir() -> PathBuf {
    let d = dirs::cache_dir().unwrap_or_else(|| PathBuf::from(".")).join("gtube");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn cookies_path() -> PathBuf {
    config_dir().join("cookies.txt")
}

static SETTINGS: LazyLock<RwLock<Settings>> = LazyLock::new(|| {
    let path = config_dir().join("settings.json");
    let s = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    RwLock::new(s)
});

pub fn settings() -> Settings {
    SETTINGS.read().unwrap().clone()
}

pub fn update(f: impl FnOnce(&mut Settings)) {
    let mut s = SETTINGS.write().unwrap();
    f(&mut s);
    if let Ok(t) = serde_json::to_string_pretty(&*s) {
        let _ = std::fs::write(config_dir().join("settings.json"), t);
    }
}

pub fn load_cookies() -> crate::api::cookies::CookieJar {
    std::fs::read_to_string(cookies_path())
        .map(|t| crate::api::cookies::CookieJar::parse_netscape(&t))
        .unwrap_or_default()
}

pub fn save_cookies(jar: &crate::api::cookies::CookieJar) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    // Only readable by you (Windows and macOS keep the profile private anyway).
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts.open(cookies_path())?;
    f.write_all(jar.to_netscape().as_bytes())
}

pub fn delete_cookies() {
    let _ = std::fs::remove_file(cookies_path());
}
