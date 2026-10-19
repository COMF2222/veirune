#![allow(dead_code)]

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

pub const APP_ID: &str = "fastcloud";
pub const INTERFACE_TEXT_SCALE_MIN: f32 = 0.9;
pub const INTERFACE_TEXT_SCALE_MAX: f32 = 1.25;
pub const INTERFACE_SCALE_MIN: f32 = 0.9;
pub const INTERFACE_SCALE_MAX: f32 = 1.15;

static SETTINGS_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn settings_write_lock() -> &'static Mutex<()> {
    SETTINGS_WRITE_LOCK.get_or_init(|| Mutex::new(()))
}

/// Last normal-size window rectangle plus its maximized state.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MainWindowBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

impl Default for MainWindowBounds {
    fn default() -> Self {
        Self {
            x: 100,
            y: 100,
            width: 1280,
            height: 800,
            maximized: false,
        }
    }
}

/// User-editable settings persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub settings_version: u8,
    pub music_taste: MusicTaste,
    #[serde(default)]
    pub wave_mode: WaveMode,
    #[serde(default)]
    pub wave_mood: WaveMood,
    #[serde(default = "default_true")]
    pub leaderboard_anonymous: bool,
    pub show_community: bool,
    pub theme_presets: Vec<serde_json::Value>,
    pub offline_limit_mb: u64,
    pub normalization: bool,
    pub crossfade_ms: u32,
    #[serde(default = "default_true")]
    pub gapless: bool,
    pub theme: ThemeMode,
    /// Language used by the application interface.
    #[serde(default)]
    pub language: Language,
    /// Artwork/network memory policy. Balanced is the migration-safe default;
    /// Eco favours small working sets, Quality favours high-DPI artwork.
    #[serde(default)]
    pub memory_profile: MemoryProfile,
    /// Stop optional continuous motion (spinners become static and idle
    /// playback UI uses the slowest useful cadence).
    #[serde(default)]
    pub reduced_motion: bool,
    /// Visual shell used when the main window switches into mini-player mode.
    #[serde(default)]
    pub mini_player_style: MiniPlayerStyle,
    /// Page shown after a normal application launch.
    #[serde(default)]
    pub startup_page: StartupPage,
    #[serde(default)]
    pub main_window_bounds: Option<MainWindowBounds>,
    /// Hide the main window in the system tray when its close button is used.
    #[serde(default = "default_true")]
    pub close_to_tray: bool,
    /// User-selected semantic accent. Kept as RGB so it stays renderer-agnostic.
    #[serde(default = "default_accent_rgb")]
    pub accent_rgb: [u8; 3],
    /// None follows the selected theme; custom colours also work without wallpaper.
    pub panel_rgb: Option<[u8; 3]>,
    pub panel_opacity: f32,
    pub panel_blur: u8,
    /// None follows the theme's page headings; zero removes their surfaces.
    #[serde(default)]
    pub heading_opacity: Option<f32>,
    pub text_rgb: Option<[u8; 3]>,
    pub muted_text_rgb: Option<[u8; 3]>,
    pub interface_text_scale: f32,
    pub interface_scale: f32,
    /// HTTP(S) or file URI painted behind the main content area.
    #[serde(default)]
    pub background_image: Option<String>,
    /// Strength of the donor's vignette and chrome-edge framing (0..=0.7).
    #[serde(default = "default_background_opacity")]
    pub background_opacity: f32,
    /// Uniform dark layer over the full image (0..=0.85).
    #[serde(default = "default_background_dim")]
    pub background_dim: f32,
    /// Blur radius applied to the wallpaper before it reaches the renderer.
    #[serde(default = "default_background_blur")]
    pub background_blur: u8,
    /// Opacity of the reading layer above a custom wallpaper, 0–1.
    #[serde(default = "default_background_overlay")]
    pub background_overlay: f32,
    /// Relative size of lyrics in the regular right panel and full-screen view.
    #[serde(default = "default_lyrics_scale")]
    pub lyrics_scale: f32,
    /// Soften inactive lyrics around the current line. The saved key is retained.
    #[serde(default = "default_true")]
    pub lyrics_blur_past: bool,
    /// Follow the current lyric line automatically.
    #[serde(default = "default_true")]
    pub lyrics_auto_scroll: bool,
    /// Wallpaper renderer semantics. Version zero was Fastcloud's old global
    /// opacity + forced-dim implementation; version one matches the donor.
    #[serde(default = "default_background_style_version")]
    pub background_style_version: u8,
    /// Discord application id. The desktop release provides a public default
    /// at build time; the owner may override it for development.
    #[serde(default)]
    pub discord_client_id: String,
    #[serde(default)]
    pub discord_presence: bool,
    /// Audio segment cache quota in MiB. Zero disables persistent audio caching.
    #[serde(default = "default_audio_cache_limit_mb")]
    pub audio_cache_limit_mb: u64,
    pub volume: f32,
    /// Stereo balance, -1 hard left to 1 hard right. Only the mini player's
    /// second slider sets it; the app's own interface has no control.
    #[serde(default)]
    pub balance: f32,
    /// Fold stereo output to the same signal in both channels.
    #[serde(default)]
    pub mono: bool,
    pub eq_gains_db: [f32; 10],
    pub eq_enabled: bool,
    /// The equaliser's preamp in dB, ±12 as Winamp's slider is.
    #[serde(default)]
    pub eq_preamp_db: f32,
    /// Winamp's AUTO: reload each track's own preset when it starts.
    #[serde(default)]
    pub eq_auto: bool,
    pub last_track_urn: Option<String>,
    pub last_position_ms: Option<u64>,
    /// Whether the saved queue belongs to an active My Wave session.
    #[serde(default)]
    pub last_wave_active: bool,
    /// Device-local radio context; never synchronized as a taste preference.
    #[serde(default)]
    pub last_station: Option<(String, Vec<crate::api::models::Track>)>,
    pub client_id: Option<String>,
    /// SoundCloud profile used for public library fallback when SoundCloud's
    /// user OAuth flow is unavailable. Public likes, tracks and playlists can
    /// still be read with the app token.
    #[serde(default)]
    pub soundcloud_profile_url: Option<String>,
    pub show_track_numbers: bool,
    /// Liked track ids (synced to SoundCloud when online).
    #[serde(default)]
    pub liked_ids: Vec<u64>,
    /// Followed user ids.
    #[serde(default)]
    pub followed_user_ids: Vec<u64>,
    /// Locally created playlists.
    #[serde(default)]
    pub custom_playlists: Vec<CustomPlaylist>,
    /// Per-track equaliser presets, by track id. Winamp kept these in
    /// `winamp.q1`; this is the same idea, and what AUTO reloads.
    #[serde(default)]
    pub eq_presets: Vec<EqPreset>,
    /// Opened SoundCloud links (local messages inbox, newest last).
    #[serde(default)]
    pub inbox: Vec<InboxItem>,
    /// Keep playing similar tracks when the queue runs out.
    #[serde(default = "default_true")]
    pub autoplay: bool,
    /// One-line track rows without artwork (Appearance → Compact rows).
    #[serde(default)]
    pub compact_rows: bool,
    /// Which trace the player bar's little display shows.
    #[serde(default)]
    pub visualiser: crate::ui::visualiser::Mode,
    /// Path of the `.wsz` the Winamp mini player wears; the built-in skin
    /// when unset or unreadable.
    #[serde(default)]
    pub winamp_skin: Option<PathBuf>,
    /// Whether the app is *in* the mini player.
    ///
    /// The mini player is not a second window: it is what the one window
    /// looks like, so this survives a restart the way Winamp's own mode did.
    #[serde(default)]
    pub winamp_window: bool,
    /// Whether the mini player is rolled up to its title bar ("windowshade").
    ///
    /// Winamp's own shade mode: 275×14 of title, time, transport and a seek
    /// bar. Still the mini player, just without the interface — so this is a
    /// second flag rather than a third window mode.
    #[serde(default)]
    pub winamp_shade: bool,
    /// Whether the equaliser window is open, and rolled up.
    #[serde(default)]
    pub winamp_eq_window: bool,
    #[serde(default)]
    pub winamp_eq_shade: bool,
    /// Whether the playlist window is open, rolled up, and how many rows it
    /// shows. Winamp's playlist is the one window that resizes.
    #[serde(default)]
    pub winamp_pl_window: bool,
    #[serde(default)]
    pub winamp_pl_shade: bool,
    #[serde(default = "default_pl_rows")]
    pub winamp_pl_rows: u32,
    /// Keep the mini player above other windows, as Winamp's "always on top".
    #[serde(default)]
    pub winamp_on_top: bool,
    /// A font file to use for the interface instead of the bundled Inter.
    ///
    /// soundcloud.com's own face (Söhne) is a commercial licence and cannot be
    /// shipped with an MIT project, so a user who owns it can point at the
    /// file here and get the site's exact letterforms. Unset, unreadable or
    /// unparsable falls back to Inter (see [`crate::fonts`]).
    #[serde(default)]
    pub interface_font: Option<PathBuf>,
    /// The mini player's whole-pixel scale, 1–4.
    #[serde(default = "default_scale")]
    pub winamp_scale: u32,
    /// Persisted queue (tracks are small; capped on save).
    #[serde(default)]
    pub last_queue: Vec<crate::api::models::Track>,
    /// Index into `last_queue` that was current.
    #[serde(default)]
    pub last_queue_idx: Option<usize>,
    #[serde(default)]
    pub last_queue_priority: Vec<usize>,
    /// Tracks and collections explicitly pinned by the listener.
    #[serde(default)]
    pub quick_access: Vec<QuickAccessShortcut>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MusicTaste {
    pub discovery: f32,
    pub diversity: f32,
    pub repeat_days: u8,
    pub genres: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaveMode { #[default] Balanced, Familiar, Discover, Mood, NoRepeats }

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaveMood { #[default] Calm, Sad, Happy, Workout, Road }

impl WaveMood {
    pub fn query(self) -> &'static str {
        match self { Self::Calm => "calm chill", Self::Sad => "sad melancholy", Self::Happy => "happy upbeat", Self::Workout => "workout energy", Self::Road => "road driving" }
    }
}

impl Default for MusicTaste {
    fn default() -> Self {
        Self { discovery: 0.5, diversity: 0.5, repeat_days: 2, genres: Vec::new() }
    }
}

impl MusicTaste {
    pub fn normalize(&mut self) {
        self.discovery = self.discovery.clamp(0.0, 1.0);
        self.diversity = self.diversity.clamp(0.0, 1.0);
        self.repeat_days = self.repeat_days.min(7);
        self.genres = self.genres.iter().map(|g| g.trim().chars().take(48).collect::<String>())
            .filter(|g| !g.is_empty()).take(12).collect();
        self.genres.sort();
        self.genres.dedup();
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            settings_version: 2,
            music_taste: MusicTaste::default(),
            wave_mode: WaveMode::Balanced, wave_mood: WaveMood::Calm, leaderboard_anonymous: true, show_community: true,
            theme_presets: Vec::new(),
            offline_limit_mb: 0,
            normalization: false,
            crossfade_ms: 0,
            gapless: true,
            theme: ThemeMode::Dark,
            language: Language::English,
            memory_profile: MemoryProfile::Balanced,
            reduced_motion: false,
            mini_player_style: MiniPlayerStyle::Airwave,
            startup_page: StartupPage::Home,
            main_window_bounds: None,
            close_to_tray: true,
            accent_rgb: default_accent_rgb(),
            panel_rgb: None,
            panel_opacity: 0.85,
            panel_blur: 12,
            heading_opacity: None,
            text_rgb: None,
            muted_text_rgb: None,
            interface_text_scale: 1.0,
            interface_scale: 1.0,
            background_image: None,
            background_opacity: default_background_opacity(),
            background_dim: default_background_dim(),
            background_blur: default_background_blur(),
            background_overlay: default_background_overlay(),
            lyrics_scale: default_lyrics_scale(),
            lyrics_blur_past: true,
            lyrics_auto_scroll: true,
            background_style_version: 1,
            discord_client_id: String::new(),
            discord_presence: false,
            audio_cache_limit_mb: default_audio_cache_limit_mb(),
            volume: 0.8,
            balance: 0.0,
            mono: false,
            eq_gains_db: [0.0; 10],
            eq_enabled: false,
            eq_preamp_db: 0.0,
            eq_auto: false,
            last_track_urn: None,
            last_position_ms: None,
            last_wave_active: false,
            last_station: None,
            client_id: None,
            soundcloud_profile_url: None,
            show_track_numbers: false,
            liked_ids: Vec::new(),
            followed_user_ids: Vec::new(),
            custom_playlists: Vec::new(),
            eq_presets: Vec::new(),
            inbox: Vec::new(),
            autoplay: true,
            compact_rows: false,
            visualiser: Default::default(),
            winamp_skin: None,
            winamp_window: false,
            winamp_shade: false,
            winamp_eq_window: false,
            winamp_eq_shade: false,
            winamp_pl_window: false,
            winamp_pl_shade: false,
            winamp_pl_rows: default_pl_rows(),
            winamp_on_top: false,
            interface_font: None,
            winamp_scale: default_scale(),
            last_queue: Vec::new(),
            last_queue_idx: None,
            last_queue_priority: Vec::new(),
            quick_access: Vec::new(),
        }
    }
}

impl Settings {
    /// Pin or unpin one media item. Returns whether it is pinned afterward.
    pub fn toggle_quick_access(&mut self, shortcut: QuickAccessShortcut) -> bool {
        if let Some(index) = self
            .quick_access
            .iter()
            .position(|candidate| candidate.same_target(&shortcut))
        {
            self.quick_access.remove(index);
            false
        } else {
            self.quick_access.insert(0, shortcut);
            true
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_accent_rgb() -> [u8; 3] {
    [0xFF, 0x5B, 0x24]
}

fn default_background_opacity() -> f32 {
    0.15
}

fn default_background_dim() -> f32 {
    0.0
}

fn default_background_blur() -> u8 {
    0
}

fn default_background_overlay() -> f32 {
    0.8
}

fn default_lyrics_scale() -> f32 {
    1.0
}

fn default_background_style_version() -> u8 {
    1
}

fn default_audio_cache_limit_mb() -> u64 {
    512
}

/// 2× is the classic window at a size a modern screen can read.
fn default_scale() -> u32 {
    2
}

/// Eight rows is enough for the playlist to be worth opening.
fn default_pl_rows() -> u32 {
    crate::skin::layout::pl::DEFAULT_ROWS
}

/// A user-created playlist persisted in settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomPlaylist {
    pub id: u64,
    pub title: String,
    pub track_ids: Vec<u64>,
}

/// One track's equaliser settings, which Winamp's AUTO reloads when that track
/// comes on.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct EqPreset {
    pub track_id: u64,
    pub preamp_db: f32,
    pub gains_db: [f32; 10],
}

/// A SoundCloud link opened/shared into the app (local messages inbox).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxItem {
    pub label: String,
    pub link: String,
    /// Unix seconds when it arrived.
    pub at: u64,
}

/// A listener-managed media item shown in Quick Access. The legacy unit
/// variants remain deserializable so older settings files still load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuickAccessShortcut {
    Track {
        id: u64,
        title: String,
        artist: String,
        artwork_url: Option<String>,
    },
    Playlist {
        id: u64,
        title: String,
        artist: String,
        artwork_url: Option<String>,
    },
    Album {
        id: u64,
        title: String,
        artist: String,
        artwork_url: Option<String>,
    },
    Likes,
    DailyMix,
    Fresh,
    Vibe,
    History,
    Station,
}

impl QuickAccessShortcut {
    pub fn same_target(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Track { id: left, .. }, Self::Track { id: right, .. })
            | (Self::Playlist { id: left, .. }, Self::Playlist { id: right, .. })
            | (Self::Album { id: left, .. }, Self::Album { id: right, .. }) => left == right,
            _ => self == other,
        }
    }

    pub fn is_media(&self) -> bool {
        matches!(
            self,
            Self::Track { .. } | Self::Playlist { .. } | Self::Album { .. }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
    System,
}

/// Language used for interface copy, independent of SoundCloud metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Language {
    #[default]
    English,
    Russian,
}

impl Language {
    pub const fn text<'a>(self, english: &'a str, russian: &'a str) -> &'a str {
        match self {
            Self::English => english,
            Self::Russian => russian,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MemoryProfile {
    Eco,
    #[default]
    Balanced,
    Quality,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MiniPlayerStyle {
    #[default]
    Airwave,
    Winamp,
}

impl MemoryProfile {
    pub const fn playback_refresh_ms(self) -> u64 {
        match self {
            Self::Eco => 250,
            Self::Balanced => 125,
            Self::Quality => 67,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StartupPage {
    #[default]
    Home,
    Search,
    Library,
    Settings,
}

impl Settings {
    pub fn load() -> Result<Self> {
        let path = settings_path()?;
        Self::load_from(&path)
    }

    pub fn load_from(path: &std::path::Path) -> Result<Self> {
        if !path.exists() {
            let backup = path.with_extension("json.bak");
            if backup.exists() {
                let raw = std::fs::read_to_string(backup)?;
                return Self::decode(&raw);
            }
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)?;
        match Self::decode(&raw) {
            Ok(settings) => Ok(settings),
            Err(primary) => {
                let backup = path.with_extension("json.bak");
                if !backup.exists() {
                    return Err(primary);
                }
                log::warn!("settings file is invalid; recovering {}", backup.display());
                let raw = std::fs::read_to_string(backup)?;
                Self::decode(&raw)
            }
        }
    }

    fn decode(raw: &str) -> Result<Self> {
        let mut value: serde_json::Value = serde_json::from_str(raw)?;
        anyhow::ensure!(value.is_object(), "Settings must be an object");
        let legacy_wallpaper = value.get("background_style_version").is_none();
        // Repair incompatible fields individually. Old installations retain all
        // valid preferences and their paused queue instead of reverting everything.
        let defaults = serde_json::to_value(Self::default())?;
        for (key, fallback) in defaults.as_object().expect("settings object") {
            if let Some(saved) = value.get(key) {
                let probe = serde_json::json!({key: saved});
                if serde_json::from_value::<Self>(probe).is_err() {
                    value[key] = fallback.clone();
                }
            }
        }
        let mut settings: Self = serde_json::from_value(value)?;
        settings.music_taste.normalize();
        settings.theme_presets.truncate(20);
        settings.offline_limit_mb = settings.offline_limit_mb.min(102_400);
        settings.crossfade_ms = settings.crossfade_ms.min(8000);
        settings.settings_version = 2;
        settings.interface_text_scale = settings.interface_text_scale
            .clamp(INTERFACE_TEXT_SCALE_MIN, INTERFACE_TEXT_SCALE_MAX);
        settings.interface_scale = settings.interface_scale
            .clamp(INTERFACE_SCALE_MIN, INTERFACE_SCALE_MAX);
        if legacy_wallpaper || settings.background_style_version == 0 {
            settings.background_opacity = default_background_opacity();
            settings.background_dim = default_background_dim();
            settings.background_blur = default_background_blur();
            settings.background_style_version = 1;
        }
        Ok(settings)
    }

    pub fn save(&self) -> Result<()> {
        let path = settings_path()?;
        self.save_to(&path)
    }

    /// Save UI preferences while preserving the player's latest session.
    pub fn save_to(&self, path: &std::path::Path) -> Result<()> {
        self.save_to_preserving_session(path)
    }

    fn save_to_preserving_session(&self, path: &std::path::Path) -> Result<()> {
        let _guard = settings_write_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut next = self.clone();
        if let Ok(current) = Self::load_from(path) {
            // Player and UI intentionally own different snapshots. A UI-only
            // save must not roll back the newer playback session on disk.
            next.last_track_urn = current.last_track_urn;
            next.last_position_ms = current.last_position_ms;
            next.last_wave_active = current.last_wave_active;
            next.last_station = current.last_station;
            next.last_queue = current.last_queue;
            next.last_queue_idx = current.last_queue_idx;
        }
        write_settings(path, &next)
    }
}

/// Update playback-owned settings under the same lock as UI saves.
pub(crate) fn update_settings(
    path: &std::path::Path,
    update: impl FnOnce(&mut Settings),
) -> Result<()> {
    let _guard = settings_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut settings = Settings::load_from(path).unwrap_or_default();
    update(&mut settings);
    write_settings(path, &settings)
}

fn write_settings(path: &std::path::Path, settings: &Settings) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_vec_pretty(settings)?;
    let temporary = path.with_extension("json.tmp");
    let backup = path.with_extension("json.bak");
    let mut file = std::fs::File::create(&temporary)?;
    file.write_all(&raw)?;
    file.sync_all()?;
    drop(file);

    if backup.exists() {
        std::fs::remove_file(&backup)?;
    }
    if path.exists() {
        std::fs::rename(path, &backup)?;
    }
    if let Err(error) = std::fs::rename(&temporary, path) {
        if backup.exists() {
            let _ = std::fs::rename(&backup, path);
        }
        return Err(error.into());
    }
    // Keep the previous valid version for recovery if the new primary is
    // damaged later (for example by an interrupted write outside this process).
    Ok(())
}

pub struct AppPaths {
    pub root: PathBuf,
    pub cache: PathBuf,
    pub audio_cache: PathBuf,
    pub cover_cache: PathBuf,
    pub skin_dir: PathBuf,
}

pub fn app_paths() -> Result<AppPaths> {
    let base = directories::ProjectDirs::from("", "", APP_ID).context("resolve app data dir")?;
    let cache = base.cache_dir().to_path_buf();
    let audio_cache = cache.join("audio");
    let cover_cache = cache.join("covers");
    let skin_dir = base.data_dir().join("skins");
    std::fs::create_dir_all(&audio_cache)?;
    std::fs::create_dir_all(&cover_cache)?;
    std::fs::create_dir_all(&skin_dir)?;
    Ok(AppPaths {
        root: base.data_dir().to_path_buf(),
        cache,
        audio_cache,
        cover_cache,
        skin_dir,
    })
}

pub fn settings_path() -> Result<PathBuf> {
    Ok(app_paths()?.root.join("settings.json"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn migration_repairs_one_invalid_field_without_resetting_valid_preferences() {
        let settings = super::Settings::decode(r#"{"language":"Russian","volume":0.37,"interface_scale":"legacy","liked_ids":[42],"last_position_ms":12345}"#).unwrap();
        assert_eq!(settings.language, super::Language::Russian);
        assert_eq!(settings.liked_ids, vec![42]);
        assert_eq!(settings.last_position_ms, Some(12345));
        assert_eq!(settings.volume, 0.37);
        assert_eq!(settings.interface_scale, 1.0);
        assert_eq!(settings.settings_version, 2);
    }
    #[test]
    fn saved_interface_sizes_are_limited_without_resetting_other_preferences() {
        let settings = Settings::decode(
            r#"{"interface_text_scale":1.6,"interface_scale":1.3,"lyrics_scale":1.5,"liked_ids":[42]}"#,
        ).unwrap();
        assert_eq!(settings.interface_text_scale, INTERFACE_TEXT_SCALE_MAX);
        assert_eq!(settings.interface_scale, INTERFACE_SCALE_MAX);
        assert_eq!(settings.lyrics_scale, 1.5);
        assert_eq!(settings.liked_ids, vec![42]);
        let small = Settings::decode(
            r#"{"interface_text_scale":0.1,"interface_scale":-1}"#,
        ).unwrap();
        assert_eq!(small.interface_text_scale, INTERFACE_TEXT_SCALE_MIN);
        assert_eq!(small.interface_scale, INTERFACE_SCALE_MIN);
    }
    #[test]
    fn custom_appearance_survives_disk_reload_without_changing_library_or_lyrics() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let settings = Settings {
            panel_rgb: Some([41, 62, 83]),
            panel_opacity: 0.35,
            panel_blur: 24,
            heading_opacity: Some(0.0),
            text_rgb: Some([240, 224, 207]),
            muted_text_rgb: Some([181, 165, 151]),
            interface_text_scale: 1.25,
            interface_scale: 1.15,
            lyrics_scale: 0.9,
            liked_ids: vec![42, 73],
            ..Settings::default()
        };
        write_settings(&path, &settings).unwrap();
        let restored = Settings::load_from(&path).unwrap();
        assert_eq!(restored.panel_rgb, settings.panel_rgb);
        assert_eq!(restored.panel_opacity, settings.panel_opacity);
        assert_eq!(restored.heading_opacity, Some(0.0));
        assert_eq!(restored.panel_blur, settings.panel_blur);
        assert_eq!(restored.text_rgb, settings.text_rgb);
        assert_eq!(restored.muted_text_rgb, settings.muted_text_rgb);
        assert_eq!(restored.interface_text_scale, 1.25);
        assert_eq!(restored.interface_scale, 1.15);
        assert_eq!(restored.lyrics_scale, 0.9);
        assert_eq!(restored.liked_ids, vec![42, 73]);
    }

    #[test]
    fn older_settings_keep_their_theme_and_gain_neutral_customization_defaults() {
        let settings: Settings = serde_json::from_str(
            r#"{"theme":"Light","lyrics_scale":1.5,"liked_ids":[42]}"#,
        ).unwrap();
        assert_eq!(settings.theme, ThemeMode::Light);
        assert_eq!(settings.lyrics_scale, 1.5);
        assert_eq!(settings.liked_ids, vec![42]);
        assert!(settings.panel_rgb.is_none());
        assert!(settings.heading_opacity.is_none());
        assert!(settings.text_rgb.is_none());
        assert!(settings.muted_text_rgb.is_none());
        assert_eq!(settings.interface_text_scale, 1.0);
        assert_eq!(settings.interface_scale, 1.0);
        assert_eq!(settings.panel_opacity, 0.85);
        assert_eq!(settings.panel_blur, 12);
    }
    use super::*;

    #[test]
    fn a_fresh_install_is_audible_and_matches_missing_settings_fields() {
        let fresh = Settings::default();
        let old: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(fresh.volume, 0.8);
        assert_eq!(
            serde_json::to_value(&fresh).unwrap(),
            serde_json::to_value(old).unwrap()
        );
        let muted: Settings = serde_json::from_str(r#"{"volume":0}"#).unwrap();
        assert_eq!(muted.volume, 0.0);
    }

    #[test]
    fn defaults_roundtrip() {
        let s = Settings::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.volume, s.volume);
        assert_eq!(back.eq_gains_db.len(), 10);
        assert!(!back.eq_enabled);
    }

    #[test]
    fn quick_access_only_changes_when_explicitly_toggled() {
        let mut settings = Settings::default();
        assert!(settings.quick_access.is_empty());
        let track = QuickAccessShortcut::Track {
            id: 42,
            title: "First title".into(),
            artist: "Artist".into(),
            artwork_url: None,
        };
        assert!(settings.toggle_quick_access(track.clone()));
        assert_eq!(settings.quick_access, [track]);
        assert!(!settings.toggle_quick_access(QuickAccessShortcut::Track {
            id: 42,
            title: "Updated title".into(),
            artist: "Artist".into(),
            artwork_url: None,
        }));
        assert!(settings.quick_access.is_empty());
    }

    #[test]
    fn newest_quick_access_shortcut_appears_first() {
        let mut settings = Settings::default();
        let first = QuickAccessShortcut::Track {
            id: 1,
            title: "First".into(),
            artist: "Artist".into(),
            artwork_url: None,
        };
        let second = QuickAccessShortcut::Track {
            id: 2,
            title: "Second".into(),
            artist: "Artist".into(),
            artwork_url: None,
        };
        settings.toggle_quick_access(first.clone());
        settings.toggle_quick_access(second.clone());
        assert_eq!(settings.quick_access, [second, first]);
    }

    #[test]
    fn quick_access_reads_legacy_entries_without_showing_them_as_media() {
        let old: QuickAccessShortcut = serde_json::from_str("\"daily_mix\"").unwrap();
        let track: QuickAccessShortcut = serde_json::from_str(
            r#"{"track":{"id":42,"title":"Song","artist":"Artist","artwork_url":null}}"#,
        )
        .unwrap();
        assert!(!old.is_media());
        assert!(track.is_media());
    }

    #[test]
    fn parse_theme() {
        let json = r#"{"theme":"Light"}"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(s.theme, ThemeMode::Light);
    }

    #[test]
    fn language_defaults_to_english_and_roundtrips_russian() {
        let old: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(old.language, Language::English);

        let russian: Settings = serde_json::from_str(r#"{"language":"Russian"}"#).unwrap();
        assert_eq!(russian.language, Language::Russian);
        assert_eq!(russian.language.text("Settings", "Настройки"), "Настройки");
        let encoded = serde_json::to_string(&russian).unwrap();
        let restored: Settings = serde_json::from_str(&encoded).unwrap();
        assert_eq!(restored.language, Language::Russian);
    }

    #[test]
    fn older_settings_receive_new_personalisation_defaults() {
        let settings: Settings = serde_json::from_str(r#"{"theme":"Dark"}"#).unwrap();

        assert_eq!(settings.startup_page, StartupPage::Home);
        assert_eq!(settings.accent_rgb, [0xFF, 0x5B, 0x24]);
        assert_eq!(settings.background_blur, 0);
        assert_eq!(settings.audio_cache_limit_mb, 512);
        assert_eq!(settings.memory_profile, MemoryProfile::Balanced);
    }

    #[test]
    fn older_settings_receive_the_donor_wallpaper_edge_darkening() {
        let settings: Settings = serde_json::from_str(r#"{"theme":"Dark"}"#).unwrap();

        assert!((settings.background_opacity - 0.15).abs() < f32::EPSILON);
    }

    #[test]
    fn older_settings_do_not_dim_the_wallpaper() {
        let settings: Settings = serde_json::from_str(r#"{"theme":"Dark"}"#).unwrap();

        assert!(settings.background_dim.abs() < f32::EPSILON);
    }

    #[test]
    fn loading_the_old_wallpaper_renderer_migrates_to_donor_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"theme":"Dark","background_image":"file:///wall.jpg","background_opacity":0.62,"background_dim":0.42,"background_blur":12}"#,
        )
        .unwrap();

        let settings = Settings::load_from(&path).unwrap();

        assert_eq!(
            (
                settings.background_opacity,
                settings.background_dim,
                settings.background_blur,
                settings.background_style_version,
            ),
            (0.15, 0.0, 0, 1)
        );
    }

    /// A settings file written before the mini player grew its three states
    /// must still load: every field it does not name takes its default, and
    /// the defaults are the old behaviour.
    #[test]
    fn an_older_settings_file_still_loads() {
        let json = r#"{"theme":"Dark","volume":0.5,"winamp_window":true}"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(s.volume, 0.5);
        assert!(s.winamp_window, "it was left in the mini player");
        assert!(!s.winamp_shade, "…and not rolled up");
        assert!(!s.winamp_on_top);
        assert_eq!(s.balance, 0.0, "centred");
        assert_eq!(s.winamp_scale, 2, "the default scale, not zero");
        assert!(s.close_to_tray, "existing installs default to the tray");
    }

    #[test]
    fn close_to_tray_choice_roundtrips() {
        let settings = Settings {
            close_to_tray: false,
            ..Settings::default()
        };
        let restored: Settings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert!(!restored.close_to_tray);
    }

    #[test]
    fn main_window_bounds_roundtrip_and_old_settings_default() {
        let bounds = MainWindowBounds {
            x: -1200,
            y: 80,
            width: 1600,
            height: 900,
            maximized: true,
        };
        let settings = Settings {
            main_window_bounds: Some(bounds),
            ..Settings::default()
        };
        let restored: Settings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        let actual = restored.main_window_bounds.unwrap();
        assert_eq!((actual.x, actual.y, actual.width, actual.height, actual.maximized),
            (bounds.x, bounds.y, bounds.width, bounds.height, bounds.maximized));
        assert!(serde_json::from_str::<Settings>(r#"{"theme":"Dark"}"#)
            .unwrap()
            .main_window_bounds
            .is_none());
    }

    /// The window's own three flags survive a round trip, since they are what
    /// brings it back the way it was left.
    #[test]
    fn the_window_mode_roundtrips() {
        let s = Settings {
            winamp_window: true,
            winamp_shade: true,
            winamp_on_top: true,
            balance: -0.4,
            ..Settings::default()
        };
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(back.winamp_window && back.winamp_shade && back.winamp_on_top);
        assert!((back.balance - -0.4).abs() < f32::EPSILON);
    }

    #[test]
    fn ui_save_preserves_the_players_newer_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let player_state = Settings {
            last_track_urn: Some("soundcloud:tracks:42".into()),
            last_position_ms: Some(12_345),
            last_queue_idx: Some(0),
            last_wave_active: true,
            last_station: Some(("artist".into(), crate::demo::demo_tracks())),
            ..Settings::default()
        };
        write_settings(&path, &player_state).unwrap();
        let stale_ui = Settings {
            theme: ThemeMode::Light,
            ..Settings::default()
        };

        stale_ui.save_to_preserving_session(&path).unwrap();

        let saved = Settings::load_from(&path).unwrap();
        assert_eq!(saved.theme, ThemeMode::Light);
        assert_eq!(saved.last_track_urn, player_state.last_track_urn);
        assert_eq!(saved.last_position_ms, player_state.last_position_ms);
        assert_eq!(saved.last_queue_idx, player_state.last_queue_idx);
        assert_eq!(saved.last_wave_active, player_state.last_wave_active);
        assert_eq!(saved.last_station.as_ref().unwrap().0, "artist");
        assert_eq!(saved.last_station.as_ref().unwrap().1.len(), 10);
    }

    #[test]
    fn load_recovers_the_backup_when_the_primary_file_is_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let backup = path.with_extension("json.bak");
        let expected = Settings {
            volume: 0.37,
            theme: ThemeMode::Light,
            ..Settings::default()
        };
        std::fs::write(&path, b"{not json").unwrap();
        std::fs::write(&backup, serde_json::to_vec(&expected).unwrap()).unwrap();

        let recovered = Settings::load_from(&path).unwrap();

        assert_eq!(recovered.theme, ThemeMode::Light);
        assert!((recovered.volume - 0.37).abs() < f32::EPSILON);
    }

    #[test]
    fn successful_write_keeps_a_recoverable_previous_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let first = Settings {
            volume: 0.37,
            ..Settings::default()
        };
        let second = Settings {
            volume: 0.73,
            ..Settings::default()
        };
        write_settings(&path, &first).unwrap();
        write_settings(&path, &second).unwrap();
        assert!((Settings::load_from(&path).unwrap().volume - 0.73).abs() < f32::EPSILON);

        std::fs::write(&path, b"{not json").unwrap();
        assert!((Settings::load_from(&path).unwrap().volume - 0.37).abs() < f32::EPSILON);
    }
}
