#![allow(dead_code)]

#[path = "../../../src/api/mod.rs"]
mod api;
#[path = "../../../src/audio/mod.rs"]
mod audio;
#[path = "../../../src/auth/mod.rs"]
mod auth;
#[path = "../../../src/config.rs"]
mod config;
#[path = "../../../src/demo.rs"]
mod demo;
#[path = "../../../src/desktop/discord.rs"]
mod discord;
#[path = "../../../src/import_yandex.rs"]
mod import_yandex;
#[path = "../../../src/link.rs"]
mod link;
mod media;
mod offline;
mod appearance;
mod diagnostics;
mod personal;
mod community;
mod chat;
mod wave;
mod station;
#[path = "../../../src/player/mod.rs"]
mod player;
#[path = "../../../src/skin/zip.rs"]
mod skin_zip;
#[path = "../../../src/store.rs"]
mod store;
#[path = "../../../src/ui/vibe.rs"]
mod vibe;
#[path = "../../../src/vis.rs"]
mod vis;
mod artwork;
mod app_icon;
mod wallpaper;
mod clap;
mod components;
mod lyrics;
mod lyrics_search;
mod update_events;
mod spotify;

// Legacy settings keep these enum/constant names. They are data compatibility
// shims; no egui code is compiled into the Tauri application.
mod ui {
    pub mod visualiser {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize,
        )]
        pub enum Mode {
            #[default]
            Spectrum,
            Scope,
            Off,
        }
    }
}
mod skin {
    pub mod layout {
        pub mod pl {
            pub const DEFAULT_ROWS: u32 = 8;
        }
    }
}

use api::models::{Comment, Me, Playlist, Track, User, WebProfile};
use parking_lot::Mutex;
use player::{Player, RepeatMode};
use serde::Serialize;
use std::sync::Arc;
use store::{Key, Slot, Store};
use tauri::Manager;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

struct AppState {
    queue_wave_history: Mutex<std::collections::VecDeque<Option<WaveSession>>>,
    personal: Arc<personal::Hub>,
    rt: tokio::runtime::Runtime,
    client: Arc<api::ApiClient>,
    player: Option<Arc<Player>>,
    store: Store,
    session: Arc<Mutex<Option<Arc<auth::Session>>>>,
    settings: Arc<Mutex<config::Settings>>,
    connection: Arc<Mutex<Connection>>,
    presence: discord::Presence,
    spotify: Arc<spotify::Import>,
    pending_link: Mutex<Option<String>>,
    demo_reposted_tracks: Mutex<std::collections::HashSet<u64>>,
    demo_reposted_playlists: Mutex<std::collections::HashSet<u64>>,
    demo_liked_playlists: Mutex<std::collections::HashSet<u64>>,
    last_eq_track: Mutex<Option<u64>>,
    last_presence: Mutex<Option<PresenceStamp>>,
    visualiser: Mutex<vis::Analyser>,
    image_cache: artwork::ArtworkCache,
    wave: Arc<wave::Engine>,
    wave_session: Arc<Mutex<Option<WaveSession>>>,
}

#[derive(Clone)]
struct WaveSession {
    station: Option<String>,
    continuation: bool,
    manual_ids: std::collections::HashSet<u64>,
    likes: Vec<Track>,
    seen: std::collections::HashSet<u64>,
    seen_identities: Vec<wave::TrackIdentity>,
    started_at: std::time::Instant,
    variation: u32,
    busy: bool,
    last_attempt: Option<std::time::Instant>,
}

#[derive(Clone, Copy)]
struct PresenceStamp {
    track_id: u64,
    position_ms: u64,
    duration_ms: u64,
    playing: bool,
    loading: bool,
    speed: f32,
    sent_at: std::time::Instant,
}

fn presence_due(previous: Option<PresenceStamp>, current: PresenceStamp) -> bool {
    let Some(previous) = previous else { return true };
    if previous.track_id != current.track_id
        || previous.duration_ms != current.duration_ms
        || previous.playing != current.playing
        || previous.loading != current.loading
        || previous.speed != current.speed
    {
        return true;
    }
    let elapsed = current.sent_at.saturating_duration_since(previous.sent_at);
    if elapsed >= std::time::Duration::from_secs(30) { return true; }
    let expected = previous.position_ms.saturating_add(
        if current.playing && !current.loading {
            (elapsed.as_millis() as f32 * current.speed) as u64
        } else { 0 }
    );
    expected.abs_diff(current.position_ms) > 1_000
}

#[cfg(test)]
#[test]
fn presence_skips_regular_progress_but_updates_for_seek_and_pause() {
    let start = PresenceStamp {
        track_id: 7, position_ms: 0, duration_ms: 180_000,
        playing: true, loading: false, speed: 1.0,
        sent_at: std::time::Instant::now(),
    };
    assert!(presence_due(None, start));
    let progress = PresenceStamp {
        position_ms: 500,
        sent_at: start.sent_at + std::time::Duration::from_millis(500),
        ..start
    };
    assert!(!presence_due(Some(start), progress));
    assert!(presence_due(Some(start), PresenceStamp { position_ms: 10_000, ..progress }));
    assert!(presence_due(Some(start), PresenceStamp { playing: false, ..progress }));
    assert!(presence_due(Some(start), PresenceStamp {
        position_ms: 30_000,
        sent_at: start.sent_at + std::time::Duration::from_secs(30),
        ..start
    }));
}

fn append_wave_tracks(
    player: &Arc<Player>,
    wave: &wave::Engine,
    wave_session: &Arc<Mutex<Option<WaveSession>>>,
    started_at: std::time::Instant,
    generated: Result<Vec<Track>, String>,
) {
    let mut guard = wave_session.lock();
    if let Some(session) = guard.as_mut().filter(|session| session.started_at == started_at) {
        if session.continuation && !*player.autoplay.lock() { session.busy = false; return; }
        match generated {
            Ok(tracks) => wave.with_dislike_filter(|dislikes| {
                let queued: std::collections::HashSet<_> = player.state.lock().queue.iter()
                    .map(|track| track.id).collect();
                let mut fresh = Vec::new();
                for track in tracks {
                    if dislikes.rejects(&track) { continue; }
                    if queued.contains(&track.id) || session.seen.contains(&track.id) { continue; }
                    let identity = wave::track_identity(&track);
                    if session.seen_identities.iter().any(|previous| wave::same_recording(previous, &identity)) { continue; }
                    session.seen.insert(track.id);
                    session.seen_identities.push(identity);
                    fresh.push(track);
                }
                if !fresh.is_empty() {
                    player.enqueue(fresh, false);
                    player.trim_played_history(120, 15);
                }
            }),
            Err(error) => log::warn!("My Wave refill failed: {error}"),
        }
        session.busy = false;
    }
}

#[cfg(test)]
#[test]
fn wave_refill_rechecks_dislikes_added_after_generation_started() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let player = Arc::new(Player::new(
        Arc::new(audio::output::AudioOutput::silent([0.0; 10], 0.8)),
        Arc::new(api::ApiClient::demo()), Arc::new(audio::cache::AudioCache::disabled()),
        dir.path().join("settings.json"), runtime.handle().clone(),
    ));
    let tracks = demo::demo_tracks();
    {
        let mut state = player.state.lock();
        state.queue = vec![tracks[0].clone()];
        state.order = vec![0];
        state.current = Some(0);
    }
    let started_at = std::time::Instant::now();
    let session = Arc::new(Mutex::new(Some(WaveSession {
        station: None,
        continuation: false, manual_ids: std::collections::HashSet::new(),
        likes: vec![tracks[0].clone()], seen: std::collections::HashSet::from([tracks[0].id]),
        seen_identities: vec![wave::track_identity(&tracks[0])],
        started_at, variation: 0, busy: true, last_attempt: Some(started_at),
    })));
    let engine = wave::Engine::new(dir.path().join("wave.json"));
    let pending_result = Ok(vec![tracks[1].clone(), tracks[2].clone()]);
    engine.dislike(&tracks[1], true);
    append_wave_tracks(&player, &engine, &session, started_at, pending_result);
    assert_eq!(player.state.lock().queue.iter().map(|track| track.id).collect::<Vec<_>>(),
        vec![tracks[0].id, tracks[2].id]);
    assert!(!session.lock().as_ref().unwrap().busy);
}

#[cfg(test)]
#[test]
fn disabling_autoplay_discards_an_in_flight_continuation() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let player = Arc::new(Player::new(
        Arc::new(audio::output::AudioOutput::silent([0.0; 10], 0.8)),
        Arc::new(api::ApiClient::demo()), Arc::new(audio::cache::AudioCache::disabled()),
        dir.path().join("settings.json"), runtime.handle().clone(),
    ));
    let tracks = demo::demo_tracks();
    player.state.lock().queue = vec![tracks[0].clone()];
    let started_at = std::time::Instant::now();
    let session = Arc::new(Mutex::new(Some(WaveSession {
        station: None,
        continuation: true, manual_ids: std::collections::HashSet::from([tracks[0].id]),
        likes: vec![tracks[0].clone()], seen: std::collections::HashSet::from([tracks[0].id]),
        seen_identities: vec![wave::track_identity(&tracks[0])],
        started_at, variation: 0, busy: true, last_attempt: Some(started_at),
    })));
    *player.autoplay.lock() = false;
    let engine = wave::Engine::new(dir.path().join("wave.json"));
    append_wave_tracks(&player, &engine, &session, started_at, Ok(vec![tracks[1].clone()]));
    assert_eq!(player.state.lock().queue.len(), 1);
    assert!(!session.lock().as_ref().unwrap().busy);
    assert!(!session.lock().as_ref().unwrap().seen.contains(&tracks[1].id));
}

#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct ImportStatus {
    running: bool,
    current: usize,
    total: usize,
    matched: usize,
    title: String,
    message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkResult {
    kind: String,
    id: u64,
    title: String,
}

#[derive(Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Connection {
    Demo,
    Public,
    SignedIn,
    Connecting,
    Pairing { code: String, url: String },
    Registering,
    Error { message: String },
}

#[derive(Serialize, serde::Deserialize)]
struct ApprovalUser {
    #[serde(default)]
    editor: bool,
    id: u64,
    username: String,
    status: String,
    updated_at: i64,
    #[serde(default)]
    last_seen: i64,
    #[serde(default)]
    online: Option<bool>,
    #[serde(default)]
    stream_requests_today: Option<u64>,
    #[serde(default)]
    usage_day: Option<String>,
}

#[derive(serde::Deserialize)]
struct ApprovalUsers { users: Vec<ApprovalUser> }

#[derive(Serialize, serde::Deserialize)]
struct ServerSession { user_id: u64, admin: bool }

#[cfg(test)]
#[test]
fn admin_payloads_preserve_legacy_unknown_usage_and_require_boolean_role() {
    let legacy: ApprovalUser = serde_json::from_str(r#"{"id":42,"username":"Fixture","status":"approved","updated_at":1}"#).unwrap();
    assert_eq!(legacy.online, None);
    assert_eq!(legacy.stream_requests_today, None);
    let current: ApprovalUser = serde_json::from_str(r#"{"id":42,"username":"Fixture","status":"approved","updated_at":1,"online":true,"stream_requests_today":7,"usage_day":"2026-10-04"}"#).unwrap();
    assert_eq!(current.online, Some(true));
    assert_eq!(current.stream_requests_today, Some(7));
    assert!(serde_json::from_str::<ServerSession>(r#"{"user_id":42,"admin":"true"}"#).is_err());
}

#[derive(Serialize)]
#[serde(tag = "status", content = "data", rename_all = "snake_case")]
enum Data<T: Serialize> {
    Loading,
    Ready(T),
    Failed(String),
    Unavailable,
}

impl<T: Serialize> From<Slot<T>> for Data<T> {
    fn from(slot: Slot<T>) -> Self {
        match slot {
            Slot::Loading => Self::Loading,
            Slot::Ready(value) => Self::Ready(value),
            Slot::Failed(error) => Self::Failed(error),
            Slot::Unavailable => Self::Unavailable,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlayerView {
    can_undo_queue: bool,
    sleep_remaining_ms: Option<u64>,
    sleep_after_track: bool,
    queue: Option<Vec<Track>>,
    queue_revision: u64,
    current: Option<usize>,
    wave_active: bool,
    is_playing: bool,
    loading: bool,
    position_ms: u64,
    duration_ms: u64,
    preview_fallback: bool,
    volume: f32,
    playback_speed: f32,
    shuffle: bool,
    repeat: RepeatMode,
    ab_start_ms: Option<u64>,
    ab_end_ms: Option<u64>,
    bitrate_kbps: u32,
    sample_rate: u32,
    error: Option<String>,
}

impl AppState {
    fn new() -> anyhow::Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let mut settings = config::Settings::load().unwrap_or_default();
        if settings.discord_client_id.trim().is_empty() {
            settings.discord_client_id = option_env!("FASTCLOUD_DISCORD_CLIENT_ID")
                .filter(|id| !id.trim().is_empty())
                .unwrap_or_default()
                .to_owned();
        }
        let has_credentials = auth::AppCredentials::discover().is_some();
        let demo = !has_credentials || std::env::args().any(|arg| arg == "--demo");
        let client = Arc::new(api::ApiClient::new(
            auth::AppCredentials::discover().map(|creds| creds.client_id),
            demo,
        ));
        let store = Store::new(client.clone(), rt.handle().clone());
        let wave = Arc::new(wave::Engine::new(config::app_paths()?.root.join("my-wave-profile.json")));
        let personal = Arc::new(personal::Hub::new(config::app_paths()?.root.join("personal"),config::settings_path()?)?);
        personal.seed_preferences(&settings);
        personal.seed_feedback(&wave.disliked_tracks());
        wave.replace_dislikes(&personal.feedback());
        wave.configure(&settings);
        if !demo {
            let previous_id = settings.last_queue_idx.and_then(|index| settings.last_queue.get(index)).map(|track| track.id);
            settings.last_queue_idx = wave.with_dislike_filter(|dislikes|
                dislikes.prune_saved_queue(&mut settings.last_queue, settings.last_queue_idx));
            let current_id = settings.last_queue_idx.and_then(|index| settings.last_queue.get(index)).map(|track| track.id);
            if previous_id != current_id { settings.last_position_ms = Some(0); }
        }

        let player = match (|| -> anyhow::Result<Arc<Player>> {
            let paths = config::app_paths()?;
            let cache = Arc::new(audio::cache::AudioCache::new(
                paths.audio_cache,
                settings.audio_cache_limit_mb.saturating_mul(1024 * 1024),
            )?);
            let output = Arc::new(audio::output::AudioOutput::open(
                settings.eq_gains_db,
                settings.volume,
            )?);
            let player = Arc::new(Player::new(
                output,
                client.clone(),
                cache.clone(),
                config::settings_path()?,
                rt.handle().clone(),
            ));
            player.attach();
            player.set_autoplay(settings.autoplay);
            player.set_audio_preferences(settings.normalization, settings.crossfade_ms, settings.gapless);
            player.restore_controls(settings.volume, settings.balance, settings.mono);
            player.set_eq(settings.eq_enabled, settings.eq_gains_db);
            player.set_eq_preamp_db(settings.eq_preamp_db);
            if demo {
                player.set_demo(true);
                let tracks = demo::demo_tracks();
                let mut state = player.state.lock();
                state.queue = tracks;
                state.queue_revision = state.queue_revision.wrapping_add(1);
                state.order = (0..state.queue.len()).collect();
                state.current = None;
            } else if !settings.last_queue.is_empty() {
                player.restore_session(settings.last_queue.clone(), settings.last_queue_idx);
                player.state.lock().position_ms = settings.last_position_ms.unwrap_or(0);
            }
            player.start_paused();
            rt.spawn(player.clone().run());
            rt.spawn(audio::cache::quota_task(
                cache,
                std::time::Duration::from_secs(300),
            ));
            Ok(player)
        })() {
            Ok(player) => Some(player),
            Err(error) => {
                log::error!("audio unavailable: {error:#}");
                None
            }
        };
        let presence = discord::Presence::spawn();
        if settings.discord_presence {
            presence.configure(Some(settings.discord_client_id.clone()));
        }
        let restored_wave = if !demo && (settings.last_wave_active || settings.autoplay) {
            player.as_ref().and_then(|player| {
                let playback = player.state.lock();
                playback.current.map(|_| {
                    let queue = playback.queue.clone();
                    WaveSession {
                        station: settings.last_station.as_ref().map(|(kind, _)| kind.clone()),
                        continuation: !settings.last_wave_active,
                        manual_ids: if settings.last_wave_active { Default::default() } else { queue.iter().map(|track|track.id).collect() },
                        likes: settings.last_station.as_ref().map_or_else(|| queue.clone(), |(_, seeds)| seeds.clone()),
                        seen: queue.iter().map(|track| track.id).collect(),
                        seen_identities: queue.iter().map(wave::track_identity).collect(),
                        started_at: std::time::Instant::now(),
                        variation: (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() / 60) as u32,
                        busy: false,
                        last_attempt: None,
                    }
                })
            })
        } else { None };
        let wave_session = Arc::new(Mutex::new(restored_wave));
        if let Some(player) = player.clone() {
            let personal = personal.clone();
            let wave = wave.clone();
            let wave_session = wave_session.clone();
            let client = client.clone();
            rt.spawn(async move {
                loop {
                    {
                        let session = wave_session.lock();
                        if let Some(session) = session.as_ref() {
                            wave.with_dislike_filter(|dislikes| {
                                player.remove_matching(|track| !session.manual_ids.contains(&track.id) && dislikes.rejects(track));
                            });
                        }
                    }
                    let snapshot = {
                        let state = player.state.lock();
                        let remaining = state.current.and_then(|current| state.order.iter()
                            .position(|&index| index == current)
                            .map(|position| state.order.len().saturating_sub(position)));
                        (state.current.and_then(|index| state.queue.get(index)).cloned(),
                            state.position_ms, state.duration_ms, state.is_playing, state.loading,
                            remaining, state.repeat == RepeatMode::Off)
                    };
                    wave.observe(snapshot.0.as_ref(), snapshot.1, snapshot.2, snapshot.3, snapshot.4);
                    if !client.is_demo() {
                        personal.observe(snapshot.0.as_ref(), snapshot.1, snapshot.3, snapshot.4, player.state.lock().playback_speed);
                    }
                    clap::release_if_idle();
                    if snapshot.1 >= 35_000 && snapshot.3 && !snapshot.4 && clap::available() {
                        if let Some(track) = snapshot.0.as_ref().filter(|track| wave.begin_audio_analysis(track.id)) {
                            let track = track.clone();
                            let samples = player.output.tap().window(vis::ANALYSIS_SAMPLES, 0);
                            let sample_rate = player.output.device_sample_rate();
                            let wave = wave.clone();
                            tokio::task::spawn_blocking(move || {
                                let result = clap::embed_audio(track.id, &samples, sample_rate)
                                    .map_err(|error| error.to_string());
                                wave.finish_audio_analysis(&track, result);
                            });
                        }
                    }
                    let refill = {
                        let mut session = wave_session.lock();
                        session.as_mut().and_then(|session| {
                            let permitted = !session.continuation || *player.autoplay.lock();
                            let active = snapshot.3;
                            let near_end = permitted && active && snapshot.6 && snapshot.5.is_some_and(|remaining| remaining <= if session.continuation {3} else {10});
                            let ready = session.last_attempt.is_none_or(|last| last.elapsed() >= std::time::Duration::from_secs(20));
                            if near_end && ready && !session.busy {
                                session.busy = true;
                                session.last_attempt = Some(std::time::Instant::now());
                                session.variation = session.variation.wrapping_add(1);
                                if session.seen.len() > 5_000 {
                                    let tracks = player.state.lock().queue.clone();
                                    session.seen = tracks.iter().map(|track| track.id).collect();
                                    session.seen_identities = tracks.iter().map(wave::track_identity).collect();
                                }
                                Some((session.likes.clone(), session.variation, session.started_at,
                                    session.seen.clone(), session.seen_identities.clone(), session.station.clone()))
                            } else { None }
                        })
                    };
                    if let Some((likes, variation, started_at, avoid, avoid_identities, station)) = refill {
                        let player = player.clone();
                        let wave = wave.clone();
                        let client = client.clone();
                        let wave_session = wave_session.clone();
                        tokio::spawn(async move {
                            let generated = if let Some(kind) = station {
                                let frontier = player.state.lock().queue.clone();
                                station::generate(client, &wave, &likes, &frontier, &kind, variation, &avoid, &avoid_identities).await
                            } else { wave::generate(client, &wave, likes, variation, &avoid, &avoid_identities).await };
                            append_wave_tracks(&player, &wave, &wave_session, started_at, generated);
                        });
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            });
        }
        let app = Self {
            queue_wave_history: Mutex::new(std::collections::VecDeque::new()),
            personal,
            rt,
            client,
            player,
            store,
            session: Arc::new(Mutex::new(None)),
            settings: Arc::new(Mutex::new(settings)),
            connection: Arc::new(Mutex::new(if demo {
                Connection::Demo
            } else {
                Connection::Connecting
            })),
            presence,
            spotify: Arc::new(spotify::Import::new(config::app_paths()?.root.join("imports"))),
            pending_link: Mutex::new(
                std::env::args()
                    .skip(1)
                    .find(|argument| link::parse(argument).is_ok()),
            ),
            demo_reposted_tracks: Mutex::new(std::collections::HashSet::new()),
            demo_reposted_playlists: Mutex::new(std::collections::HashSet::new()),
            demo_liked_playlists: Mutex::new(std::collections::HashSet::new()),
            last_eq_track: Mutex::new(None),
            last_presence: Mutex::new(None),
            visualiser: Mutex::new(vis::Analyser::default()),
            image_cache: artwork::ArtworkCache::new(config::app_paths()?.cover_cache.join("tauri"))?,
            wave,
            wave_session,
        };
        {
            let personal = app.personal.clone(); let client = app.client.clone();
            let settings = app.settings.clone(); let player = app.player.clone(); let connection = app.connection.clone(); let wave = app.wave.clone();
            app.rt.spawn(async move {
                let mut last_sync: Option<std::time::Instant> = None;
                loop {
                    let signed_in = matches!(*connection.lock(),Connection::SignedIn);
                    if signed_in && last_sync.is_none_or(|last| last.elapsed() >= std::time::Duration::from_secs(30)) {
                        last_sync = Some(std::time::Instant::now());
                        if personal.sync(&client,&settings,player.as_ref(),&wave).await.is_err() { personal.sync_failed(); }
                        wave.configure(&settings.lock());
                    }
                    if !signed_in { last_sync = None; }
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            });
        }
        if !demo {
            let client = app.client.clone();
            let store = app.store.clone();
            let player = app.player.clone();
            let session_slot = app.session.clone();
            let connection = app.connection.clone();
            let wave_session = app.wave_session.clone();
            app.rt.spawn(async move {
                if let Some(session) = auth::start(client.clone()).await {
                    let signed_in = session.signed_in().await;
                    store.set_signed_in(signed_in);
                    store.clear();
                    *session_slot.lock() = Some(session);
                    *connection.lock() = if signed_in {
                        Connection::SignedIn
                    } else {
                        Connection::Public
                    };
                    if signed_in && wave_session.lock().is_some() {
                        let client = client.clone();
                        tokio::spawn(async move {
                            match api::endpoints::my_liked_tracks(&client).await.collect_all().await {
                                Ok(likes) if !likes.is_empty() => {
                                    if let Some(session) = wave_session.lock().as_mut() {
                                        session.likes = likes;
                                    }
                                }
                                Ok(_) => {}
                                Err(error) => log::warn!("Could not restore My Wave likes: {error}"),
                            }
                        });
                    }
                } else {
                    client.set_demo(true);
                    if let Some(player) = &player {
                        player.set_demo(true);
                        player.stop();
                        let mut playback = player.state.lock();
                        playback.queue = demo::demo_tracks();
                        playback.queue_revision = playback.queue_revision.wrapping_add(1);
                        playback.order = (0..playback.queue.len()).collect();
                        playback.current = None;
                    }
                    *connection.lock() = Connection::Demo;
                }
            });
        }
        Ok(app)
    }

    fn player(&self) -> Result<&Arc<Player>, String> {
        self.player
            .as_ref()
            .ok_or_else(|| "Audio output is unavailable".into())
    }

    fn persist_wave_active(&self, active: bool) {
        { let mut settings = self.settings.lock(); settings.last_wave_active = active; if !active { settings.last_station = None; } }
        match config::settings_path() {
            Ok(path) => if let Err(error) = config::update_settings(&path, |settings| { settings.last_wave_active = active; if !active { settings.last_station = None; } }) {
                log::warn!("Could not save My Wave state: {error}");
            },
            Err(error) => log::warn!("Could not locate My Wave state: {error}"),
        }
    }

    fn persist_station(&self, station: Option<(String, Vec<Track>)>) {
        self.settings.lock().last_station = station.clone();
        if let Ok(path) = config::settings_path() {
            if let Err(error) = config::update_settings(&path, |settings| settings.last_station = station) {
                log::warn!("Could not save radio context: {error}");
            }
        }
    }
}

#[tauri::command]
fn connection(state: tauri::State<'_, AppState>) -> Connection {
    state.connection.lock().clone()
}

#[tauri::command]
fn my_profile(state: tauri::State<'_, AppState>) -> Data<Me> {
    if !state.store.signed_in() {
        return Data::Unavailable;
    }
    state.store.poll();
    state.store.me().into()
}

#[tauri::command]
fn take_pending_link(state: tauri::State<'_, AppState>) -> Option<String> {
    state.pending_link.lock().take()
}

fn queue_snapshot(queue: &[Track], revision: u64, known_revision: Option<u64>) -> Option<Vec<Track>> {
    (known_revision != Some(revision)).then(|| queue.to_vec())
}

#[cfg(test)]
#[test]
fn player_queue_snapshot_is_sent_only_when_revision_changes() {
    let queue = vec![serde_json::from_str::<Track>(r#"{"id":7,"title":"Song"}"#).unwrap()];
    assert!(queue_snapshot(&queue, 3, Some(3)).is_none());
    assert_eq!(queue_snapshot(&queue, 4, Some(3)).unwrap()[0].id, 7);
    assert_eq!(queue_snapshot(&queue, 3, None).unwrap()[0].id, 7);
}

#[cfg(test)]
#[test]
fn shared_catalogue_lists_keep_the_json_array_contract() {
    let tracks = serde_json::to_value(Data::Ready(Arc::new(demo::demo_tracks()))).unwrap();
    let playlists = serde_json::to_value(Data::Ready(Arc::new(demo::demo_playlists()))).unwrap();
    assert!(tracks["data"].is_array());
    assert!(playlists["data"].is_array());
}

#[tauri::command]
async fn player_state(
    state: tauri::State<'_, AppState>,
    known_queue_revision: Option<u64>,
) -> Result<PlayerView, String> {
    let player = state.player()?;
    // Dispatch polling off the native window event loop. Read independent
    // snapshots before playback state. Settings and audio
    // preferences are updated by other threads; nested locks here can freeze IPC.
    let discord_presence = state.settings.lock().discord_presence;
    let can_undo_queue = player.can_undo_queue();
    let (sleep_remaining_ms, sleep_after_track) = player.sleep_view();
    let current = player.state.lock();
    if discord_presence {
        let mut last_presence = state.last_presence.lock();
        if let Some(track) = current.current.and_then(|index| current.queue.get(index)) {
            let stamp = PresenceStamp {
                track_id: track.id,
                position_ms: current.position_ms,
                duration_ms: current.duration_ms,
                playing: current.is_playing,
                loading: current.loading,
                speed: current.playback_speed,
                sent_at: std::time::Instant::now(),
            };
            if presence_due(*last_presence, stamp) {
                state.presence.update(discord::NowPlaying {
                    title: track.title.clone(),
                    artist: track.artist().to_owned(),
                    artwork_url: track.artwork_url().map(str::to_owned),
                    track_url: track.permalink_url.clone(),
                    duration_secs: (current.duration_ms / 1000) as i64,
                    elapsed_secs: (current.position_ms / 1000) as i64,
                    playing: current.is_playing,
                });
                *last_presence = Some(stamp);
            }
        } else if last_presence.take().is_some() {
            state.presence.clear();
        }
    }
    let track_id = current.current.and_then(|index| current.queue.get(index)).map(|track| track.id);
    let mut view = PlayerView {
        can_undo_queue,
        sleep_remaining_ms,
        sleep_after_track,
        queue: queue_snapshot(&current.queue, current.queue_revision, known_queue_revision),
        queue_revision: current.queue_revision,
        current: current.current,
        wave_active: false,
        is_playing: current.is_playing,
        loading: current.loading,
        // Read the audio cursor directly; the saved state is published at 10 Hz.
        position_ms: if current.is_playing && !current.loading {
            let position = player.output.position_ms();
            if current.duration_ms > 0 { position.min(current.duration_ms) } else { position }
        } else { current.position_ms },
        duration_ms: current.duration_ms,
        preview_fallback: current.preview_fallback,
        volume: current.volume,
        playback_speed: current.playback_speed,
        shuffle: current.shuffle,
        repeat: current.repeat,
        ab_start_ms: current.ab_start_ms,
        ab_end_ms: current.ab_end_ms,
        bitrate_kbps: current.bitrate_kbps,
        sample_rate: current.sample_rate,
        error: current.error.clone(),
    };
    drop(current);
    view.wave_active = state.wave_session.lock().as_ref().is_some_and(|session| !session.continuation);
    let mut last = state.last_eq_track.lock();
    if *last != track_id {
        *last = track_id;
        if let Some(id) = track_id {
            let mut settings = state.settings.lock();
            if settings.eq_auto {
                if let Some(preset) = settings
                    .eq_presets
                    .iter()
                    .find(|preset| preset.track_id == id)
                    .copied()
                {
                    settings.eq_preamp_db = preset.preamp_db;
                    settings.eq_gains_db = preset.gains_db;
                    player.set_eq(settings.eq_enabled, preset.gains_db);
                    player.set_eq_preamp_db(preset.preamp_db);
                    if let Err(error) = settings.save() {
                        log::warn!("could not save EQ preset: {error}");
                    }
                }
            }
        }
    }
    Ok(view)
}

#[tauri::command]
fn offline_tracks() -> Result<Vec<offline::OfflineEntry>, String> {
    offline::list().map_err(|error| error.to_string())
}

#[tauri::command]
async fn my_wave(
    state: tauri::State<'_, AppState>,
    liked_tracks: Vec<Track>,
    variation: u32,
) -> Result<Vec<Track>, String> {
    if !feedback_account_ready(&state) { return Err("Personal data is loading; try again shortly".into()); }
    let variation = state.wave.next_variation(variation);
    let engine = state.wave.clone();
    // Ranking and saving the opening history must not stall the UI thread.
    let (liked_tracks, seed) = tokio::task::spawn_blocking(move || {
        let seed = wave::first_seed(&engine, &liked_tracks, variation);
        (liked_tracks, seed)
    }).await.map_err(|error| error.to_string())?;
    let (seed, prepared) = if let Some(seed) = seed { (seed, None) } else {
        // Strict modes may have no suitable local opening. Find an unheard or
        // mood-matched opening instead of falling back to a prohibited repeat.
        let generated = wave::generate(state.client.clone(), &state.wave, liked_tracks.clone(), variation,
            &Default::default(), &[]).await?;
        (generated.first().cloned().ok_or("No tracks match this wave mode yet")?, Some(generated))
    };
    let player = state.player()?.clone();
    let started_at = std::time::Instant::now();
    {
        let mut session = state.wave_session.lock();
        state.wave.with_dislike_filter(|dislikes| {
            if dislikes.rejects(&seed) {
                return Err("Track was excluded from My Wave; start a new wave".to_string());
            }
            player.play_queue(vec![seed.clone()], 0, false);
            *session = Some(WaveSession {
                station: None,
                continuation: false, manual_ids: Default::default(),
                likes: liked_tracks.clone(), seen: std::collections::HashSet::from([seed.id]),
                seen_identities: vec![wave::track_identity(&seed)],
                started_at, variation: variation.wrapping_add(1), busy: true, last_attempt: Some(started_at),
            });
            Ok(())
        })?;
    }
    state.persist_station(None);
    state.persist_wave_active(true);
    let client = state.client.clone();
    let wave = state.wave.clone();
    let wave_session = state.wave_session.clone();
    let avoid = std::collections::HashSet::from([seed.id]);
    let avoid_identities = vec![wave::track_identity(&seed)];
    state.rt.spawn(async move {
        let generated = if let Some(prepared) = prepared { Ok(prepared.into_iter().skip(1).collect()) } else {
            wave::generate(client, &wave, liked_tracks, variation.wrapping_add(1), &avoid, &avoid_identities).await
        };
        append_wave_tracks(&player, &wave, &wave_session, started_at, generated);
    });
    Ok(vec![seed])
}

#[tauri::command]
async fn start_station(state: tauri::State<'_, AppState>, tracks: Vec<Track>, kind: String) -> Result<(), String> {
    if !feedback_account_ready(&state) { return Err("Personal data is loading; try again shortly".into()); }
    if !["track", "collection", "artist"].contains(&kind.as_str()) { return Err("Unknown radio kind".into()); }
    let variation = state.wave.next_variation(0);
    let seeds = state.wave.with_dislike_filter(|filter| {
        let eligible: Vec<_> = tracks.into_iter().filter(|track| !filter.rejects(track)).collect();
        station::seeds(&eligible, if kind == "track" { 0 } else { variation })
    });
    let seed = seeds.first().ok_or("No playable radio seeds")?.clone();
    let player = state.player()?.clone();
    let started_at = std::time::Instant::now();
    {
        let mut session = state.wave_session.lock();
        state.wave.with_dislike_filter(|filter| {
            if filter.rejects(&seed) { return Err("Track was excluded from recommendations"); }
            player.play_queue(vec![seed.clone()], 0, false);
            *session = Some(WaveSession { station: Some(kind.clone()), continuation: false,
                manual_ids: Default::default(), likes: seeds.clone(),
                seen: std::collections::HashSet::from([seed.id]), seen_identities: vec![wave::track_identity(&seed)],
                started_at, variation: 0, busy: true, last_attempt: Some(started_at) });
            Ok(())
        }).map_err(str::to_string)?;
    }
    state.persist_station(Some((kind.clone(), seeds.clone())));
    state.persist_wave_active(true);
    let client = state.client.clone(); let wave = state.wave.clone(); let session = state.wave_session.clone();
    state.rt.spawn(async move {
        let result = station::generate(client, &wave, &seeds, &[], &kind, 0,
            &std::collections::HashSet::from([seed.id]), &[wave::track_identity(&seed)]).await;
        append_wave_tracks(&player, &wave, &session, started_at, result);
    });
    Ok(())
}

#[tauri::command]
fn wave_dislike(state: tauri::State<'_, AppState>, track: Track, disliked: bool) -> Result<(),String> {
    if !feedback_account_ready(&state) { return Err("Personal data is loading; try again shortly".into()); }
    let _session = state.wave_session.lock();
    state.personal.feedback_changed(&track,disliked).map_err(|error| error.to_string())?;
    state.wave.replace_dislikes(&state.personal.feedback());
    if disliked && let Some(player) = state.player.as_ref() {
        state.wave.with_dislike_filter(|dislikes| {
            player.remove_matching(|queued| dislikes.rejects(queued));
        });
    }
    Ok(())
}

#[tauri::command]
fn wave_dislikes(state: tauri::State<'_, AppState>) -> Result<Vec<Track>,String> {
    if !feedback_account_ready(&state) { return Err("Personal data is loading; try again shortly".into()); }
    Ok(state.personal.disliked_tracks())
}

fn feedback_account_ready(state: &AppState) -> bool {
    if state.client.is_demo() { return true; }
    matches!(state.store.me(), Slot::Ready(user) if user.id == state.personal.account_id())
}

#[tauri::command]
fn wave_disliked(state: tauri::State<'_, AppState>, track_id: u64) -> bool {
    feedback_account_ready(&state) && state.wave.disliked(track_id)
}

#[tauri::command]
fn set_offline_like_order(track_ids: Vec<u64>) -> Result<(), String> {
    offline::set_like_order(track_ids).map_err(|error| error.to_string())
}

#[tauri::command]
async fn download_offline_track(
    state: tauri::State<'_, AppState>,
    track: Track,
) -> Result<offline::OfflineEntry, String> {
    let limit_mb = state.settings.lock().offline_limit_mb;
    offline::download(state.client.clone(), track, limit_mb)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn offline_capacity(state: tauri::State<'_, AppState>) -> Result<offline::Capacity, String> {
    offline::capacity(state.settings.lock().offline_limit_mb).map_err(|e| e.to_string())
}

#[tauri::command]
fn pin_offline_track(track_id: u64, pinned: bool) -> Result<(), String> {
    offline::set_pinned(track_id, pinned).map_err(|e| e.to_string())
}

#[tauri::command]
fn clear_unpinned_tracks(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let current = state.player.as_ref().and_then(|p| p.current_track()).map(|t| t.id);
    offline::clear_unpinned(current).map_err(|e| e.to_string())
}

#[tauri::command]
fn remove_offline_track(track_id: u64) -> Result<(), String> {
    offline::remove(track_id).map_err(|error| error.to_string())
}

#[tauri::command]
fn clear_offline_tracks() -> Result<(), String> {
    offline::clear_all().map_err(|error| error.to_string())
}

/// Fetch SoundCloud artwork in the native process. WebView image requests may
/// fail even when the API and audio client can reach the CDN. Restrict this
/// bridge to the artwork CDN so it cannot fetch arbitrary local addresses.
#[tauri::command]
async fn image_data(state: tauri::State<'_, AppState>, url: String) -> Result<String, String> {
    let relay = state.client.relay_credentials().await.map_err(|error| error.to_string())?;
    state.image_cache.image_source(url, relay).await
}

#[derive(Serialize)]
struct WaveformSamples {
    values: Vec<u16>,
    height: u16,
}

/// SoundCloud exposes waveform samples beside its PNG URL. Keep this bridge
/// restricted to the waveform CDN, and bound the response before parsing it.
#[tauri::command]
async fn waveform_samples(url: String) -> Result<WaveformSamples, String> {
    let mut parsed = url::Url::parse(&url).map_err(|error| error.to_string())?;
    if parsed.host_str() != Some("wave.sndcdn.com") || !matches!(parsed.scheme(), "http" | "https") {
        return Err("Unsupported waveform URL".into());
    }
    let path = parsed.path().to_owned();
    if !path.ends_with("_m.png") && !path.ends_with("_m.json") {
        return Err("Unsupported waveform format".into());
    }
    parsed.set_scheme("https").map_err(|_| "Invalid waveform scheme")?;
    if path.ends_with("_m.png") {
        parsed.set_path(&path.replace("_m.png", "_m.json"));
    }
    // Reuse the connection pool across waveforms instead of creating a new
    // TLS client for every track detail view.
    static WAVEFORM_HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let mut response = WAVEFORM_HTTP.get_or_init(reqwest::Client::new)
        .get(parsed)
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    if response.content_length().unwrap_or(0) > 512_000 {
        return Err("Waveform is too large".into());
    }
    let mut body = Vec::with_capacity(response.content_length().unwrap_or(0).min(64 * 1024) as usize);
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        if chunk.len() > 512_000usize.saturating_sub(body.len()) {
            return Err("Waveform is too large".into());
        }
        body.extend_from_slice(&chunk);
    }
    #[derive(serde::Deserialize)]
    struct SoundCloudWaveform {
        height: u16,
        samples: Vec<u16>,
    }
    let waveform: SoundCloudWaveform = serde_json::from_slice(&body).map_err(|error| error.to_string())?;
    if waveform.samples.is_empty() || waveform.samples.len() > 20_000 {
        return Err("Invalid waveform samples".into());
    }
    Ok(WaveformSamples { values: waveform.samples, height: waveform.height.max(1) })
}

#[derive(Serialize)]
struct VisualiserFrame {
    bars: Vec<u8>,
    peaks: Vec<Option<u8>>,
    scope: Vec<u8>,
}

#[tauri::command]
fn visualiser_frame(state: tauri::State<'_, AppState>) -> Result<VisualiserFrame, String> {
    let player = state.player()?;
    let mode = state.settings.lock().visualiser;
    let playing = player.state.lock().is_playing;
    let tap = player.output_handle().tap();
    let wave = if playing {
        tap.window(vis::FFT_SAMPLES, vis::LAG)
    } else {
        vec![0.0; vis::FFT_SAMPLES]
    };
    let bars = if mode == ui::visualiser::Mode::Spectrum {
        state
            .visualiser
            .lock()
            .step(&wave, std::time::Instant::now())
    } else {
        [vis::Bar::default(); vis::BARS]
    };
    let scope = if mode == ui::visualiser::Mode::Scope && playing {
        vis::scope(&tap.window(vis::SCOPE_SAMPLES, vis::LAG)).to_vec()
    } else {
        vec![7; vis::COLUMNS]
    };
    Ok(VisualiserFrame {
        bars: bars.iter().map(|bar| bar.height).collect(),
        peaks: bars.iter().map(|bar| bar.peak).collect(),
        scope,
    })
}

async fn public_profile_id(state: &AppState) -> Result<Option<u64>, String> {
    let Some(raw) = state.settings.lock().soundcloud_profile_url.clone() else {
        return Ok(None);
    };
    let parsed = link::parse(&raw).map_err(|error| error.to_string())?;
    match parsed {
        link::ParsedLink::UserId(id) => Ok(Some(id)),
        link::ParsedLink::RemoteUrl(url) => {
            let value = api::endpoints::resolve(&state.client, &url)
                .await
                .map_err(|error| error.to_string())?;
            if value.get("kind").and_then(|kind| kind.as_str()) != Some("user") {
                return Err("The configured SoundCloud link is not a profile".into());
            }
            value
                .get("id")
                .and_then(|id| id.as_u64())
                .ok_or("SoundCloud profile has no ID".into())
                .map(Some)
        }
        _ => Err("The configured SoundCloud link is not a profile".into()),
    }
}

#[tauri::command]
async fn tracks(
    state: tauri::State<'_, AppState>,
    view: String,
    query: Option<String>,
    id: Option<u64>,
) -> Result<Data<Arc<Vec<Track>>>, String> {
    let recommendations = matches!(view.as_str(), "discover" | "related" | "recommended_genre");
    let result = tracks_data(&state, view, query, id).await;
    Ok(if recommendations { filter_recommendations(&state.wave, result) } else { result })
}

fn filter_recommendations(engine: &wave::Engine, result: Data<Arc<Vec<Track>>>) -> Data<Arc<Vec<Track>>> {
    match result {
        Data::Ready(mut tracks) => engine.with_dislike_filter(|dislikes| {
            if tracks.iter().any(|track| dislikes.rejects(track)) {
                Arc::make_mut(&mut tracks).retain(|track| !dislikes.rejects(track));
            }
            Data::Ready(tracks)
        }),
        other => other,
    }
}

#[cfg(test)]
#[test]
fn recommendation_filter_applies_old_dislikes_without_changing_library_cache() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wave.json");
    let engine = wave::Engine::new(path.clone());
    let source = Arc::new(demo::demo_tracks());
    let rejected = source[0].id;
    engine.dislike(&source[0], true);
    let restored = wave::Engine::new(path);
    let Data::Ready(filtered) = filter_recommendations(&restored, Data::Ready(source.clone())) else { panic!("ready tracks expected") };
    assert!(!filtered.iter().any(|track| track.id == rejected));
    assert!(source.iter().any(|track| track.id == rejected));
    restored.dislike(&source[0], false);
    let Data::Ready(allowed) = filter_recommendations(&restored, Data::Ready(source.clone())) else { panic!("ready tracks expected") };
    assert!(Arc::ptr_eq(&source, &allowed));
}

async fn tracks_data(
    state: &AppState,
    view: String,
    query: Option<String>,
    id: Option<u64>,
) -> Data<Arc<Vec<Track>>> {
    if !state.client.is_demo() && matches!(*state.connection.lock(), Connection::Connecting) {
        return Data::Loading;
    }
    if state.client.is_demo() {
        let tracks = demo::demo_tracks();
        let search_query = query.as_deref().unwrap_or_default().to_lowercase();
        let result = match view.as_str() {
            "feed" => {
                let activity = demo::demo_feed();
                activity
                    .into_iter()
                    .filter_map(|item| {
                        tracks
                            .iter()
                            .find(|track| track.id == item.track_id)
                            .cloned()
                            .map(|mut track| {
                                track.feed_reposted = item.kind == "reposted";
                                track
                            })
                    })
                    .collect()
            }
            "search" => tracks
                .into_iter()
                .filter(|track| track.title.to_lowercase().contains(&search_query))
                .collect(),
            "related" => tracks
                .into_iter()
                .filter(|track| Some(track.id) != id)
                .collect(),
            "likes" => {
                let liked = state.settings.lock().liked_ids.clone();
                tracks
                    .into_iter()
                    .filter(|track| liked.contains(&track.id))
                    .collect()
            }
            "reposts" => tracks
                .into_iter()
                .filter(|track| state.demo_reposted_tracks.lock().contains(&track.id))
                .collect(),
            "playlist" => {
                if let Some(custom) = state
                    .settings
                    .lock()
                    .custom_playlists
                    .iter()
                    .find(|item| Some(item.id) == id)
                    .cloned()
                {
                    return Data::Ready(Arc::new(
                        custom
                            .track_ids
                            .iter()
                            .filter_map(|track_id| {
                                tracks.iter().find(|track| track.id == *track_id).cloned()
                            })
                            .collect(),
                    ));
                }
                let count = demo::demo_playlists()
                    .into_iter()
                    .find(|playlist| Some(playlist.id) == id)
                    .and_then(|playlist| playlist.track_count)
                    .unwrap_or(0);
                tracks.into_iter().take(count as usize).collect()
            }
            "artist" => tracks
                .into_iter()
                .filter(|track| track.user.as_ref().is_some_and(|user| Some(user.id) == id))
                .collect(),
            _ => tracks,
        };
        return Data::Ready(Arc::new(result));
    }
    state.store.poll();
    if view == "likes" && !state.store.signed_in() {
        return match public_profile_id(&state).await {
            Ok(Some(id)) => state.store.tracks_shared(Key::UserLikes(id)).into(),
            Ok(None) => Data::Unavailable,
            Err(error) => Data::Failed(error),
        };
    }
    let key = match view.as_str() {
        "likes" => Key::Likes,
        "feed" => Key::Feed,
        "history" => Key::History,
        "following" => Key::FollowingTracks,
        "uploads" => Key::MyTracks,
        "reposts" => Key::MyRepostedTracks,
        "artist_likes" => Key::UserLikes(id.unwrap_or(0)),
        "artist_reposts" => Key::UserRepostedTracks(id.unwrap_or(0)),
        "search" => Key::SearchTracks(query.unwrap_or_default()),
        "related" => Key::Related(id.unwrap_or(0)),
        "playlist" => Key::PlaylistTracks(id.unwrap_or(0)),
        "artist" => Key::UserTracks(id.unwrap_or(0)),
        "genre" | "recommended_genre" => Key::Genre(query.unwrap_or_else(|| "Electronic".into())),
        _ => Key::Genre("electronic".into()),
    };
    state.store.tracks_shared(key).into()
}

#[tauri::command]
async fn playlists(
    state: tauri::State<'_, AppState>,
    view: String,
    query: Option<String>,
) -> Result<Data<Arc<Vec<Playlist>>>, String> {
    Ok(playlists_data(&state, view, query).await)
}

async fn playlists_data(
    state: &AppState,
    view: String,
    query: Option<String>,
) -> Data<Arc<Vec<Playlist>>> {
    if !state.client.is_demo() && matches!(*state.connection.lock(), Connection::Connecting) {
        return Data::Loading;
    }
    if state.client.is_demo() {
        let search = query.unwrap_or_default().to_lowercase();
        let custom = state.settings.lock().custom_playlists.clone();
        let playlists = demo::demo_playlists()
            .into_iter()
            .chain(custom.into_iter().map(|item| Playlist {
                id: item.id,
                title: item.title,
                artwork: None,
                artwork_url: None,
                user: None,
                track_count: Some(item.track_ids.len() as u64),
                genre: None,
                tag_list: None,
                likes_count: None,
                duration_ms: None,
                is_album: false,
                playlist_type: None,
                set_type: None,
                created_at: None,
                permalink_url: None,
                tracks: Vec::new(),
                feed_reposted: false,
            }))
            .filter(|playlist| match view.as_str() {
                "feed" => false,
                "search" => playlist.title.to_lowercase().contains(&search),
                "liked" => state.demo_liked_playlists.lock().contains(&playlist.id),
                "reposts" => state.demo_reposted_playlists.lock().contains(&playlist.id),
                "artist" | "artist_liked" | "artist_reposts" => false,
                _ => true,
            })
            .collect();
        return Data::Ready(Arc::new(playlists));
    }
    state.store.poll();
    if view == "mine" && !state.store.signed_in() {
        return match public_profile_id(&state).await {
            Ok(Some(id)) => state.store.playlists_shared(Key::UserPlaylists(id)).into(),
            Ok(None) => Data::Unavailable,
            Err(error) => Data::Failed(error),
        };
    }
    let key = match view.as_str() {
        "feed" => Key::Feed,
        "liked" => Key::LikedPlaylists,
        "reposts" => Key::MyRepostedPlaylists,
        "artist" => Key::UserPlaylists(
            query
                .as_deref()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
        ),
        "artist_liked" => Key::UserLikedPlaylists(
            query
                .as_deref()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
        ),
        "artist_reposts" => Key::UserRepostedPlaylists(
            query
                .as_deref()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
        ),
        "search" => Key::SearchPlaylists(query.unwrap_or_default()),
        _ => Key::MyPlaylists,
    };
    state.store.playlists_shared(key).into()
}

#[tauri::command]
async fn catalog_releases(state: tauri::State<'_, AppState>, day: u32) -> Result<Data<Vec<Playlist>>, String> {
    if state.client.is_demo() { return Ok(Data::Ready(demo::demo_playlists())); }
    if matches!(*state.connection.lock(), Connection::Connecting) { return Ok(Data::Loading); }
    let started = std::time::Instant::now();
    let rotating = ["album", "ep", "single", "compilation"][day as usize % 4];
    let browse = async {
        let mut pager = api::endpoints::browse_playlists(&state.client).await;
        let mut rows = pager.next_page().await.ok()?;
        for _ in 0..(day % 3 + 1) {
            let next = pager.next_page().await.unwrap_or_default();
            if next.is_empty() { break; }
            rows.extend(next);
        }
        Some(rows)
    };
    let extra = async { api::endpoints::search_playlists(&state.client, rotating).await.next_page().await.ok() };
    let (browsed, extra) = tokio::join!(browse, extra);
    let mut rows = browsed.unwrap_or_default();
    if rows.is_empty() && (rotating != "album" || extra.as_ref().is_none_or(Vec::is_empty)) {
        rows.extend(api::endpoints::search_playlists(&state.client, "album").await.next_page().await.map_err(|error| error.to_string())?);
    }
    if let Some(extra) = extra {
        rows.extend(extra);
    }
    let mut seen = std::collections::HashSet::new();
    rows.retain(|item| seen.insert(item.id));
    log::debug!("Catalog releases loaded {} entries in {:?}", rows.len(), started.elapsed());
    Ok(Data::Ready(rows))
}

#[tauri::command]
fn users(state: tauri::State<'_, AppState>, query: String) -> Data<Vec<User>> {
    if !state.client.is_demo() && matches!(*state.connection.lock(), Connection::Connecting) {
        return Data::Loading;
    }
    if state.client.is_demo() {
        return Data::Ready(
            demo_users()
                .into_iter()
                .filter(|user| user.username.to_lowercase().contains(&query.to_lowercase()))
                .collect(),
        );
    }
    state.store.poll();
    state.store.users(Key::SearchUsers(query)).into()
}

#[tauri::command]
fn track_detail(state: tauri::State<'_, AppState>, id: u64) -> Data<Track> {
    if !state.client.is_demo() && matches!(*state.connection.lock(), Connection::Connecting) {
        return Data::Loading;
    }
    if state.client.is_demo() {
        return demo::demo_tracks()
            .into_iter()
            .find(|track| track.id == id)
            .map(Data::Ready)
            .unwrap_or_else(|| Data::Failed("Track not found".into()));
    }
    state.store.poll();
    state.store.track(id).into()
}

#[tauri::command]
fn following(state: tauri::State<'_, AppState>) -> Data<Vec<User>> {
    if state.client.is_demo() {
        let ids = state.settings.lock().followed_user_ids.clone();
        return Data::Ready(
            demo_users()
                .into_iter()
                .filter(|user| ids.contains(&user.id))
                .collect(),
        );
    }
    state.store.poll();
    state.store.users(Key::Following).into()
}

#[tauri::command]
fn refresh_following(state: tauri::State<'_, AppState>) {
    state.store.invalidate(&Key::Following);
    state.store.invalidate(&Key::FollowingTracks);
}

#[tauri::command]
fn user_detail(state: tauri::State<'_, AppState>, id: u64) -> Data<User> {
    if state.client.is_demo() {
        return demo_users()
            .into_iter()
            .find(|user| user.id == id)
            .map(Data::Ready)
            .unwrap_or_else(|| Data::Failed("User not found".into()));
    }
    state.store.poll();
    state.store.user(id).into()
}

#[tauri::command]
fn playlist_detail(state: tauri::State<'_, AppState>, id: u64) -> Data<Playlist> {
    if state.client.is_demo() {
        let custom = state
            .settings
            .lock()
            .custom_playlists
            .iter()
            .find(|item| item.id == id)
            .cloned();
        return demo::demo_playlists()
            .into_iter()
            .find(|item| item.id == id)
            .or_else(|| {
                custom.map(|item| Playlist {
                    id: item.id,
                    title: item.title,
                    artwork: None,
                    artwork_url: None,
                    user: None,
                    track_count: Some(item.track_ids.len() as u64),
                    genre: None,
                    tag_list: None,
                    likes_count: None,
                    duration_ms: None,
                    is_album: false,
                    playlist_type: None,
                    set_type: None,
                    created_at: None,
                    permalink_url: None,
                    tracks: Vec::new(),
                    feed_reposted: false,
                })
            })
            .map(Data::Ready)
            .unwrap_or_else(|| Data::Failed("Playlist not found".into()));
    }
    state.store.poll();
    state.store.playlist(id).into()
}

#[tauri::command]
fn comments(state: tauri::State<'_, AppState>, id: u64) -> Data<Vec<Comment>> {
    if state.client.is_demo() {
        return Data::Ready(Vec::new());
    }
    state.store.poll();
    state.store.comments(Key::Comments(id)).into()
}

#[tauri::command]
async fn track_lyrics(artist: String, title: String, duration_ms: u64, album_name: Option<String>, isrc: Option<String>) -> Result<Option<lyrics::LyricsRecord>, String> {
    lyrics::lookup(artist, title, duration_ms, album_name, isrc).await
}

#[tauri::command]
async fn search_lyrics(query: String) -> Result<Vec<lyrics::LyricsRecord>, String> {
    lyrics::search(query).await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LyricTrackMatch {
    track: Track,
    excerpt: String,
    match_score: u16,
    recording_score: u16,
}

#[tauri::command]
async fn lyric_tracks(state: tauri::State<'_, AppState>, query: String) -> Result<Vec<LyricTrackMatch>, String> {
    if state.client.is_demo() { return Ok(Vec::new()); }
    let songs = lyrics_search::search(&query).await?;
    let mut requests = tokio::task::JoinSet::new();
    for song in songs {
        let client = state.client.clone();
        requests.spawn(async move {
            let query = format!("{} {}", song.record.artist_name, song.record.track_name);
            api::endpoints::search_tracks(&client, &query).await.next_page().await.map(|tracks| {
                tracks.into_iter().filter_map(|track| {
                    let recording_score = lyrics_search::track_score(&song, &track);
                    (recording_score > 0).then(|| LyricTrackMatch {
                        track, excerpt: song.excerpt.clone(), match_score: song.score, recording_score,
                    })
                }).collect::<Vec<_>>()
            })
        });
    }
    let mut results = Vec::new();
    let mut answered = requests.is_empty();
    let mut failure = String::new();
    while let Some(response) = requests.join_next().await {
        match response {
            Ok(Ok(matches)) => { answered = true; results.extend(matches); }
            Ok(Err(error)) => failure = error.to_string(),
            Err(_) => failure = "Track search interrupted".into(),
        }
    }
    if !answered { return Err(failure); }
    results.sort_by(|left, right| right.match_score.cmp(&left.match_score)
        .then_with(|| right.recording_score.cmp(&left.recording_score))
        .then_with(|| right.track.likes_count.cmp(&left.track.likes_count))
        .then_with(|| left.track.id.cmp(&right.track.id)));
    let mut seen = std::collections::HashSet::new();
    results.retain(|item| seen.insert(item.track.id));
    results.truncate(24);
    Ok(results)
}

#[tauri::command]
fn open_lyrics_source(raw: String) -> Result<(), String> {
    let url = url::Url::parse(&raw).map_err(|error| error.to_string())?;
    if url.scheme() != "https" || !matches!(url.host_str(), Some("genius.com" | "lrclib.net" | "lyrics.ovh")) {
        return Err("Only supported lyrics sources can be opened here".into());
    }
    webbrowser::open(url.as_str()).map_err(|error| error.to_string())
}

#[tauri::command]
fn user_profiles(state: tauri::State<'_, AppState>, id: u64) -> Data<Vec<WebProfile>> {
    if state.client.is_demo() {
        return Data::Ready(Vec::new());
    }
    state.store.poll();
    state.store.web_profiles(Key::UserWebProfiles(id)).into()
}

#[tauri::command]
fn related_users(state: tauri::State<'_, AppState>, id: u64) -> Data<Vec<User>> {
    if state.client.is_demo() {
        return Data::Ready(demo_users());
    }
    state.store.poll();
    state.store.users(Key::RelatedUsers(id)).into()
}

#[tauri::command]
async fn open_link(state: tauri::State<'_, AppState>, raw: String) -> Result<LinkResult, String> {
    let parsed = link::parse(&raw).map_err(|error| error.to_string())?;
    let (kind, id) = match parsed {
        link::ParsedLink::TrackId(id) => ("track", id),
        link::ParsedLink::PlaylistId(id) => ("playlist", id),
        link::ParsedLink::UserId(id) => ("user", id),
        link::ParsedLink::RemoteUrl(url) => {
            if state.client.is_demo() {
                return Err("Connect SoundCloud to resolve web links".into());
            }
            let resolved = api::endpoints::resolve(&state.client, &url)
                .await
                .map_err(|error| error.to_string())?;
            let id = resolved
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .ok_or("Unsupported SoundCloud link")?;
            let kind = match resolved.get("kind").and_then(serde_json::Value::as_str) {
                Some("playlist") => "playlist",
                Some("user") => "user",
                _ => "track",
            };
            (kind, id)
        }
    };
    let title = match kind {
        "track" => {
            let track = if state.client.is_demo() {
                demo::demo_tracks()
                    .into_iter()
                    .find(|track| track.id == id)
                    .ok_or("Track not found")?
            } else {
                api::endpoints::track(&state.client, id)
                    .await
                    .map_err(|error| error.to_string())?
            };
            let title = track.title.clone();
            *state.wave_session.lock() = None;
            state.persist_wave_active(false);
            state.player()?.play_queue(vec![track], 0, false);
            title
        }
        "playlist" => format!("Playlist {id}"),
        _ => format!("Profile {id}"),
    };
    {
        let mut settings = state.settings.lock();
        settings.inbox.retain(|item| item.link != raw);
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|time| time.as_secs())
            .unwrap_or(0);
        settings.inbox.push(config::InboxItem {
            label: title.clone(),
            link: raw,
            at,
        });
        if settings.inbox.len() > 50 {
            settings.inbox.remove(0);
        }
        settings.save().map_err(|error| error.to_string())?;
    }
    Ok(LinkResult {
        kind: kind.into(),
        id,
        title,
    })
}

#[tauri::command]
fn open_soundcloud_url(raw: String) -> Result<(), String> {
    let url = url::Url::parse(&raw).map_err(|error| error.to_string())?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https"
        || url.username() != ""
        || url.password().is_some()
        || !(host == "soundcloud.com" || host.ends_with(".soundcloud.com"))
    {
        return Err("Only SoundCloud HTTPS links can be opened here".into());
    }
    webbrowser::open(url.as_str()).map_err(|error| error.to_string())
}

#[tauri::command]
fn open_release_notes(raw: String) -> Result<(), String> {
    let url = url::Url::parse(&raw).map_err(|error| error.to_string())?;
    if url.scheme() != "https"
        || url.host_str() != Some("fastcloud.comf.workers.dev")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || !matches!(url.path(), "/changes.html" | "/en/changes.html")
    {
        return Err("Only the Veirune release history can be opened here".into());
    }
    webbrowser::open(url.as_str()).map_err(|error| error.to_string())
}

#[tauri::command]
async fn vibe_search(
    state: tauri::State<'_, AppState>,
    query: String,
) -> Result<Vec<Track>, String> {
    let query = query.trim();
    if query.len() < 2 {
        return Ok(Vec::new());
    }
    let profile = vibe::profile(query);
    if state.client.is_demo() {
        return Ok(vibe::rank(&profile, demo::demo_tracks()));
    }
    let mut requests = tokio::task::JoinSet::new();
    for search in profile.searches.iter().take(3) {
        let client = state.client.clone();
        let search = search.clone();
        requests.spawn(async move { api::endpoints::search_tracks(&client, &search).await.next_page().await });
    }
    for genre in profile.genres.iter().take(3) {
        let client = state.client.clone();
        let genre = genre.clone();
        let exact = profile.exact_genre.is_some();
        requests.spawn(async move {
            let rows = api::endpoints::tracks_by_genre(&client, &genre, 30).await.next_page().await?;
            if rows.is_empty() && exact {
                api::endpoints::tracks_by_genre(&client, &genre, 365).await.next_page().await
            } else { Ok(rows) }
        });
    }
    let mut candidates = Vec::new();
    let mut answered = false;
    let mut failure = String::new();
    while let Some(response) = requests.join_next().await {
        match response {
            Ok(Ok(rows)) => { answered = true; candidates.extend(rows); }
            Ok(Err(error)) => failure = error.to_string(),
            Err(_) => failure = "Mood search interrupted".into(),
        }
    }
    if !answered { return Err(failure); }
    state.wave.with_dislike_filter(|dislikes| candidates.retain(|track| !track.is_blocked() && !dislikes.rejects(track)));
    let mut result = vibe::rank(&profile, candidates);
    result.truncate(80);
    Ok(result)
}

#[tauri::command]
async fn post_comment(
    state: tauri::State<'_, AppState>,
    id: u64,
    body: String,
    timestamp_ms: Option<u64>,
) -> Result<(), String> {
    if !state.store.signed_in() {
        return Err("Sign in to comment".into());
    }
    let body = body.trim();
    if body.is_empty() {
        return Err("Comment cannot be empty".into());
    }
    api::endpoints::post_comment(&state.client, id, body, timestamp_ms)
        .await
        .map_err(|error| error.to_string())?;
    state.store.invalidate(&Key::Comments(id));
    Ok(())
}

#[tauri::command]
async fn repost(
    state: tauri::State<'_, AppState>,
    kind: String,
    id: u64,
    active: bool,
) -> Result<(), String> {
    if state.client.is_demo() {
        let mut ids = match kind.as_str() {
            "track" => state.demo_reposted_tracks.lock(),
            "playlist" => state.demo_reposted_playlists.lock(),
            _ => return Err("Unknown repost type".into()),
        };
        if active {
            ids.insert(id);
        } else {
            ids.remove(&id);
        }
        return Ok(());
    }
    if !state.store.signed_in() {
        return Err("Sign in to repost".into());
    }
    let result = match (kind.as_str(), active) {
        ("track", true) => api::endpoints::repost_track(&state.client, id).await,
        ("track", false) => api::endpoints::unrepost_track(&state.client, id).await,
        ("playlist", true) => api::endpoints::repost_playlist(&state.client, id).await,
        ("playlist", false) => api::endpoints::unrepost_playlist(&state.client, id).await,
        _ => return Err("Unknown repost type".into()),
    };
    result.map_err(|error| error.to_string())?;
    state.store.invalidate(&Key::MyRepostedTracks);
    state.store.invalidate(&Key::MyRepostedPlaylists);
    Ok(())
}

#[tauri::command]
async fn like_playlist(
    state: tauri::State<'_, AppState>,
    id: u64,
    liked: bool,
) -> Result<(), String> {
    if state.client.is_demo() {
        let mut ids = state.demo_liked_playlists.lock();
        if liked {
            ids.insert(id);
        } else {
            ids.remove(&id);
        }
        return Ok(());
    }
    if !state.store.signed_in() {
        return Err("Sign in to like playlists".into());
    }
    let result = if liked {
        api::endpoints::like_playlist(&state.client, id).await
    } else {
        api::endpoints::unlike_playlist(&state.client, id).await
    };
    result.map_err(|error| error.to_string())?;
    state.store.invalidate(&Key::LikedPlaylists);
    Ok(())
}

fn demo_users() -> Vec<User> {
    let mut users = std::collections::BTreeMap::new();
    for track in demo::demo_tracks() {
        if let Some(user) = track.user {
            users.entry(user.id).or_insert_with(|| User {
                id: user.id,
                username: user.username,
                permalink: user.permalink,
                avatar_url: user.avatar_url,
                full_name: None,
                description: None,
                city: None,
                country_code: None,
                permalink_url: None,
                followers_count: 0,
                followings_count: 0,
                track_count: 10,
                public_playlists_count: None,
            });
        }
    }
    users.into_values().collect()
}

#[tauri::command]
async fn set_followed(
    state: tauri::State<'_, AppState>,
    user_id: u64,
    followed: bool,
) -> Result<(), String> {
    if !state.client.is_demo() {
        if !state.store.signed_in() {
            return Err("Sign in to follow artists".into());
        }
        if followed {
            api::endpoints::follow_user(&state.client, user_id).await
        } else {
            api::endpoints::unfollow_user(&state.client, user_id).await
        }
        .map_err(|error| error.to_string())?;
        state.store.invalidate(&Key::Following);
        state.store.invalidate(&Key::FollowingTracks);
    }
    let mut settings = state.settings.lock();
    if followed && !settings.followed_user_ids.contains(&user_id) {
        settings.followed_user_ids.push(user_id);
    } else if !followed {
        settings.followed_user_ids.retain(|id| *id != user_id);
    }
    settings.save().map_err(|error| error.to_string())
}

#[tauri::command]
async fn create_playlist(
    state: tauri::State<'_, AppState>,
    title: String,
    track_id: Option<u64>,
    track_ids: Option<Vec<u64>>,
) -> Result<Playlist, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("Enter a playlist name".into());
    }
    let private = track_ids.is_some();
    let mut seen = std::collections::HashSet::new();
    let ids: Vec<u64> = track_ids.unwrap_or_default().into_iter().chain(track_id).filter(|id| *id > 0 && seen.insert(*id)).collect();
    if ids.len() > 1000 || title.chars().count() > 128 { return Err("Playlist is too large".into()); }
    if state.client.is_demo() {
        let mut settings = state.settings.lock();
        let id = settings
            .custom_playlists
            .iter()
            .map(|item| item.id)
            .max()
            .unwrap_or(8999)
            .max(8999)
            + 1;
        settings.custom_playlists.push(config::CustomPlaylist {
            id,
            title: title.into(),
            track_ids: ids.clone(),
        });
        settings.save().map_err(|error| error.to_string())?;
        return Ok(Playlist {
            id,
            title: title.into(),
            artwork: None,
            artwork_url: None,
            user: None,
            track_count: Some(ids.len() as u64),
            genre: None,
            tag_list: None,
            likes_count: None,
            duration_ms: None,
            is_album: false,
            playlist_type: None,
            set_type: None,
            created_at: None,
            permalink_url: None,
            tracks: Vec::new(),
            feed_reposted: false,
        });
    }
    if !state.store.signed_in() {
        return Err("Sign in to create playlists".into());
    }
    let result = api::endpoints::create_playlist(&state.client, title, !private, &ids)
        .await
        .map_err(|error| error.to_string())?;
    state.store.invalidate(&Key::MyPlaylists);
    Ok(result)
}

#[tauri::command]
async fn add_to_playlist(
    state: tauri::State<'_, AppState>,
    playlist_id: u64,
    track_id: u64,
) -> Result<(), String> {
    if state.client.is_demo() {
        let mut settings = state.settings.lock();
        let playlist = settings
            .custom_playlists
            .iter_mut()
            .find(|item| item.id == playlist_id)
            .ok_or("Choose a playlist you created")?;
        if !playlist.track_ids.contains(&track_id) {
            playlist.track_ids.push(track_id);
        }
        return settings.save().map_err(|error| error.to_string());
    }
    let mut ids = editable_playlist_track_ids(&state, playlist_id).await?;
    if !ids.contains(&track_id) {
        ids.push(track_id);
        api::endpoints::update_playlist(&state.client, playlist_id, None, None, &ids)
            .await
            .map_err(|error| error.to_string())?;
        state.store.invalidate(&Key::PlaylistTracks(playlist_id));
        state.store.invalidate(&Key::MyPlaylists);
    }
    Ok(())
}

#[tauri::command]
async fn add_tracks_to_playlist(
    state: tauri::State<'_, AppState>,
    playlist_id: u64,
    track_ids: Vec<u64>,
) -> Result<(), String> {
    if track_ids.is_empty() {
        return Ok(());
    }
    if state.client.is_demo() {
        let mut settings = state.settings.lock();
        let playlist = settings
            .custom_playlists
            .iter_mut()
            .find(|item| item.id == playlist_id)
            .ok_or("Choose a playlist you created")?;
        for track_id in track_ids {
            if !playlist.track_ids.contains(&track_id) {
                playlist.track_ids.push(track_id);
            }
        }
        return settings.save().map_err(|error| error.to_string());
    }
    let mut ids = editable_playlist_track_ids(&state, playlist_id).await?;
    let original_len = ids.len();
    for track_id in track_ids {
        if !ids.contains(&track_id) {
            ids.push(track_id);
        }
    }
    if ids.len() != original_len {
        api::endpoints::update_playlist(&state.client, playlist_id, None, None, &ids)
            .await
            .map_err(|error| error.to_string())?;
        state.store.invalidate(&Key::PlaylistTracks(playlist_id));
        state.store.invalidate(&Key::MyPlaylists);
    }
    Ok(())
}

async fn editable_playlist_track_ids(
    state: &AppState,
    playlist_id: u64,
) -> Result<Vec<u64>, String> {
    if !state.store.signed_in() {
        return Err("Sign in to edit playlists".into());
    }
    let playlist = api::endpoints::playlist(&state.client, playlist_id)
        .await
        .map_err(|error| error.to_string())?;
    let tracks = api::endpoints::playlist_tracks(&state.client, playlist_id)
        .await
        .collect_all()
        .await
        .map_err(|error| error.to_string())?;
    if playlist
        .track_count
        .is_some_and(|count| count != tracks.len() as u64)
    {
        return Err("Playlist contains tracks that could not be loaded; edit was cancelled".into());
    }
    Ok(tracks.iter().map(|track| track.id).collect())
}

#[tauri::command]
async fn remove_from_playlist(
    state: tauri::State<'_, AppState>,
    playlist_id: u64,
    track_id: u64,
) -> Result<(), String> {
    if state.client.is_demo() {
        let mut settings = state.settings.lock();
        let playlist = settings
            .custom_playlists
            .iter_mut()
            .find(|item| item.id == playlist_id)
            .ok_or("Choose a playlist you created")?;
        playlist.track_ids.retain(|id| *id != track_id);
        return settings.save().map_err(|error| error.to_string());
    }
    let mut ids = editable_playlist_track_ids(&state, playlist_id).await?;
    if ids.contains(&track_id) {
        ids.retain(|id| *id != track_id);
        api::endpoints::update_playlist(&state.client, playlist_id, None, None, &ids)
            .await
            .map_err(|error| error.to_string())?;
        state.store.invalidate(&Key::PlaylistTracks(playlist_id));
        state.store.invalidate(&Key::MyPlaylists);
    }
    Ok(())
}

#[tauri::command]
async fn move_in_playlist(
    state: tauri::State<'_, AppState>,
    playlist_id: u64,
    from: usize,
    to: usize,
) -> Result<(), String> {
    if state.client.is_demo() {
        let mut settings = state.settings.lock();
        let playlist = settings
            .custom_playlists
            .iter_mut()
            .find(|item| item.id == playlist_id)
            .ok_or("Choose a playlist you created")?;
        if from >= playlist.track_ids.len() || to >= playlist.track_ids.len() {
            return Err("Track position is out of range".into());
        }
        let id = playlist.track_ids.remove(from);
        playlist.track_ids.insert(to, id);
        return settings.save().map_err(|error| error.to_string());
    }
    let mut ids = editable_playlist_track_ids(&state, playlist_id).await?;
    if from >= ids.len() || to >= ids.len() {
        return Err("Track position is out of range".into());
    }
    let id = ids.remove(from);
    ids.insert(to, id);
    api::endpoints::update_playlist(&state.client, playlist_id, None, None, &ids)
        .await
        .map_err(|error| error.to_string())?;
    state.store.invalidate(&Key::PlaylistTracks(playlist_id));
    state.store.invalidate(&Key::MyPlaylists);
    Ok(())
}

#[tauri::command]
async fn rename_playlist(
    state: tauri::State<'_, AppState>,
    playlist_id: u64,
    title: String,
) -> Result<(), String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("Enter a playlist name".into());
    }
    if state.client.is_demo() {
        let mut settings = state.settings.lock();
        let playlist = settings
            .custom_playlists
            .iter_mut()
            .find(|item| item.id == playlist_id)
            .ok_or("Choose a playlist you created")?;
        playlist.title = title.into();
        return settings.save().map_err(|error| error.to_string());
    }
    let ids = editable_playlist_track_ids(&state, playlist_id).await?;
    api::endpoints::update_playlist(&state.client, playlist_id, Some(title), None, &ids)
        .await
        .map_err(|error| error.to_string())?;
    state.store.invalidate(&Key::MyPlaylists);
    state.store.invalidate(&Key::PlaylistTracks(playlist_id));
    Ok(())
}

#[tauri::command]
async fn delete_playlist(
    state: tauri::State<'_, AppState>,
    playlist_id: u64,
) -> Result<(), String> {
    if state.client.is_demo() {
        let mut settings = state.settings.lock();
        let len = settings.custom_playlists.len();
        settings
            .custom_playlists
            .retain(|item| item.id != playlist_id);
        if settings.custom_playlists.len() == len {
            return Err("Choose a playlist you created".into());
        }
        return settings.save().map_err(|error| error.to_string());
    }
    if !state.store.signed_in() {
        return Err("Sign in to delete playlists".into());
    }
    api::endpoints::delete_playlist(&state.client, playlist_id)
        .await
        .map_err(|error| error.to_string())?;
    state.store.invalidate(&Key::MyPlaylists);
    state.store.invalidate(&Key::PlaylistTracks(playlist_id));
    Ok(())
}

#[tauri::command]
fn transport(
    state: tauri::State<'_, AppState>,
    action: String,
    value: Option<f32>,
    index: Option<usize>,
    target: Option<usize>,
) -> Result<(), String> {
    let player = state.player()?;
    if ["remove", "move", "clear_played", "clear_upcoming", "clear_queue"].contains(&action.as_str()) {
        player.checkpoint_queue();
        let snapshot = state.wave_session.lock().clone();
        let mut history = state.queue_wave_history.lock(); history.push_back(snapshot); while history.len() > 5 { history.pop_front(); }
    }
    match action.as_str() {
        "undo_queue" => {
            if player.undo_queue() {
                let wave = state.queue_wave_history.lock().pop_back().flatten();
                let active = wave.as_ref().is_some_and(|session| !session.continuation);
                let station = wave.as_ref().and_then(|session| session.station.as_ref().map(|kind| (kind.clone(), session.likes.clone())));
                *state.wave_session.lock() = wave; state.persist_wave_active(active);
                state.persist_station(station);
                if active { state.wave.with_dislike_filter(|dislikes| { player.remove_matching(|track| dislikes.rejects(track)); }); }
            }
        },
        "toggle" => player.play_pause(),
        "next" => {
            player.next();
        }
        "previous" => {
            player.prev();
        }
        "shuffle" => player.toggle_shuffle(),
        "repeat" => player.cycle_repeat(),
        "ab_loop" => player.cycle_ab_loop(),
        "ab_clear" => player.clear_ab_loop(),
        "seek" => player.seek_ms(value.unwrap_or_default().max(0.0) as u64),
        "volume" => player.set_volume(value.unwrap_or_default().clamp(0.0, 1.0)),
        "speed" => player.set_playback_speed(value.unwrap_or(1.0)),
        "skip_to" => {
            player.skip_to(index.ok_or("Missing queue index")?);
        }
        "remove" => {
            player.remove_at(index.ok_or("Missing queue index")?);
        }
        "move" => {
            player.move_at(
                index.ok_or("Missing queue index")?,
                target.ok_or("Missing target index")?,
            );
        }
        "retry" => player.retry_current(),
        "clear_played" => { player.clear_played(); },
        "clear_upcoming" => {
            *state.wave_session.lock() = None;
            state.persist_wave_active(false);
            player.clear_upcoming();
        }
        "clear_queue" => {
            *state.wave_session.lock() = None;
            state.persist_wave_active(false);
            player.clear_queue();
        }
        "stop" => {
            *state.wave_session.lock() = None;
            state.persist_wave_active(false);
            player.stop();
        }
        _ => return Err("Unknown player action".into()),
    }
    Ok(())
}

#[tauri::command]
fn play_tracks(
    state: tauri::State<'_, AppState>,
    tracks: Vec<Track>,
    index: usize,
    recommended: Option<bool>,
) -> Result<(), String> {
    if tracks.is_empty() || index >= tracks.len() {
        return Err("Choose a track to play".into());
    }
    let mut session = state.wave_session.lock();
    *session = None;
    state.persist_wave_active(false);
    let player = state.player()?;
    let seed_tracks: Vec<_> = tracks.iter().rev().take(4).cloned().collect();
    let seen: std::collections::HashSet<_> = tracks.iter().map(|track|track.id).collect();
    let seen_identities: Vec<_> = tracks.iter().map(wave::track_identity).collect();
    if recommended.unwrap_or(false) {
        state.wave.with_dislike_filter(|dislikes| {
            let selected_id = tracks[index].id;
            let tracks: Vec<_> = tracks.into_iter().filter(|track| !dislikes.rejects(track)).collect();
            let index = tracks.iter().position(|track| track.id == selected_id)
                .ok_or("Track was excluded from recommendations")?;
            player.play_queue(tracks, index, false);
            Ok::<_, String>(())
        })?;
    } else {
        player.play_queue(tracks, index, false);
    }
    if state.settings.lock().autoplay {
        let manual_ids = if recommended.unwrap_or(false) { Default::default() } else { seen.clone() };
        *session = Some(WaveSession { station: None, continuation:true, manual_ids, likes:seed_tracks,
            seen, seen_identities, started_at:std::time::Instant::now(),variation:state.wave.next_variation(0),
            busy:false,last_attempt:None });
    }
    Ok(())
}

#[tauri::command]
fn enqueue(state: tauri::State<'_, AppState>, track: Track, next: bool) -> Result<(), String> {
    state.player()?.enqueue(vec![track], next);
    Ok(())
}

#[tauri::command]
fn enqueue_tracks(
    state: tauri::State<'_, AppState>,
    tracks: Vec<Track>,
    next: bool,
) -> Result<(), String> {
    if !tracks.is_empty() {
        state.player()?.enqueue(tracks, next);
    }
    Ok(())
}

#[tauri::command]
async fn set_liked(
    state: tauri::State<'_, AppState>,
    track_id: u64,
    liked: bool,
) -> Result<(), String> {
    if !state.client.is_demo() {
        if !state.store.signed_in() {
            return Err("Sign in to like tracks".into());
        }
        if liked {
            api::endpoints::like_track(&state.client, track_id)
                .await
                .map_err(|error| error.to_string())?;
        } else {
            api::endpoints::unlike_track(&state.client, track_id)
                .await
                .map_err(|error| error.to_string())?;
        }
        state.store.invalidate(&Key::Likes);
    }
    if liked && feedback_account_ready(&state) {
        let rated = state.personal.feedback().get(&track_id.to_string()).and_then(|item| item.track.as_track());
        if let Some(track) = rated {
            state.personal.feedback_changed(&track,false).map_err(|error| error.to_string())?;
            state.wave.replace_dislikes(&state.personal.feedback());
        }
    }
    let mut settings = state.settings.lock();
    if liked && !settings.liked_ids.contains(&track_id) {
        settings.liked_ids.push(track_id);
    } else if !liked {
        settings.liked_ids.retain(|id| *id != track_id);
    }
    settings.save().map_err(|error| error.to_string())?;
    drop(settings);
    // Likes on the current track affect the next refill immediately.
    if liked && feedback_account_ready(&state) { state.personal.liked(track_id); }
    let playing_track = state.player.as_ref().and_then(|player| {
        let playback = player.state.lock();
        playback.queue.iter().find(|track| track.id == track_id).cloned()
    });
    if let Some(session) = state.wave_session.lock().as_mut() {
        if liked {
            if let Some(track) = playing_track.filter(|_| !session.likes.iter().any(|item| item.id == track_id)) {
                session.likes.insert(0, track);
            }
        } else {
            session.likes.retain(|track| track.id != track_id);
        }
    }
    Ok(())
}

#[tauri::command]
fn theme_action(state: tauri::State<'_, AppState>, action: String, name: Option<String>, preset: Option<appearance::Preset>, path: Option<String>) -> Result<config::Settings, String> {
    let mut settings = state.settings.lock();
    match action.as_str() {
        "apply" => {
            let next = appearance::apply(&settings, &preset.ok_or("Choose a theme")?).map_err(|e| e.to_string())?;
            next.save().map_err(|e| e.to_string())?;
            *settings = next;
        }
        "save" => {
            let preset = appearance::capture(&settings, name.ok_or("Enter a theme name")?).map_err(|e| e.to_string())?;
            let mut next = settings.clone();
            next.theme_presets.retain(|saved| saved["name"].as_str() != Some(&preset.name));
            if next.theme_presets.len() >= 20 { return Err("You can save up to 20 themes".into()); }
            next.theme_presets.push(serde_json::to_value(preset).map_err(|e| e.to_string())?);
            next.save().map_err(|e| e.to_string())?;
            *settings = next;
        }
        "delete" => {
            let name = name.ok_or("Choose a saved theme")?;
            let mut next = settings.clone();
            next.theme_presets.retain(|preset| preset["name"].as_str() != Some(&name));
            next.save().map_err(|e| e.to_string())?;
            *settings = next;
        }
        "import" => {
            let preset = appearance::read(std::path::Path::new(&path.ok_or("Choose a file")?)).map_err(|e| e.to_string())?;
            let mut next = appearance::apply(&settings, &preset).map_err(|e| e.to_string())?;
            next.theme_presets.retain(|saved| saved["name"].as_str() != Some(&preset.name));
            if next.theme_presets.len() >= 20 { return Err("You can save up to 20 themes".into()); }
            next.theme_presets.push(serde_json::to_value(preset).map_err(|e| e.to_string())?);
            next.save().map_err(|e| e.to_string())?;
            *settings = next;
        }
        "export" => {
            let preset = appearance::capture(&settings, name.filter(|value| !value.trim().is_empty()).unwrap_or_else(|| "Veirune theme".into())).map_err(|e| e.to_string())?;
            let path = path.ok_or("Choose where to save the theme")?;
            if !path.to_lowercase().ends_with(".json") { return Err("Use a .json theme file".into()); }
            std::fs::write(path, serde_json::to_vec_pretty(&preset).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        }
        _ => return Err("Unknown theme action".into()),
    }
    state.wave.configure(&settings);
    state.personal.preferences_changed(&settings);
    Ok(settings.clone())
}

#[tauri::command]
fn personal_collections(state: tauri::State<'_, AppState>) -> serde_json::Value { state.personal.collections() }

#[tauri::command]
async fn community_request(state: tauri::State<'_, AppState>, action: community::Action, input: serde_json::Value) -> Result<serde_json::Value,String> {
    community::call(&state.client, action, input).await
}

#[tauri::command]
async fn leaderboard(state: tauri::State<'_, AppState>, days: u32, sort: String) -> Result<serde_json::Value, String> {
    if ![0, 7, 30].contains(&days) || !["time", "plays"].contains(&sort.as_str()) { return Err("Invalid leaderboard selection".into()); }
    let (server, token) = state.client.relay_credentials().await.map_err(|e| e.to_string())?.ok_or("Sign in through the Veirune server")?;
    let response = reqwest::Client::new().get(format!("{server}/v1/leaderboard"))
        .query(&[("days", days.to_string()), ("sort", sort)])
        .header("Authorization", format!("OAuth {token}"))
        .timeout(std::time::Duration::from_secs(15)).send().await.map_err(|e| e.to_string())?;
    if response.status() == reqwest::StatusCode::NOT_FOUND { return Err("Leaderboard is not available yet. Update the Veirune server.".into()); }
    response.error_for_status().map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn leaderboard_anonymity(state: tauri::State<'_, AppState>, anonymous: bool) -> Result<(), String> {
    let original = state.session.lock().clone().ok_or("Sign in first")?;
    let (server, token) = state.client.relay_credentials().await.map_err(|e| e.to_string())?.ok_or("Sign in through the Veirune server")?;
    reqwest::Client::new().post(format!("{server}/v1/me/personal"))
        .header("Authorization", format!("OAuth {token}")).header("X-Veirune-Preferences", "2")
        .json(&serde_json::json!({"preferences": {"leaderboard_anonymous": anonymous}}))
        .timeout(std::time::Duration::from_secs(15)).send().await.map_err(|e| e.to_string())?
        .error_for_status().map_err(|e| e.to_string())?;
    if !state.session.lock().as_ref().is_some_and(|session| std::sync::Arc::ptr_eq(session, &original)) { return Err("Account changed".into()); }
    let mut settings = state.settings.lock(); settings.leaderboard_anonymous = anonymous;
    settings.save().map_err(|e| e.to_string())?; state.personal.preferences_changed(&settings);
    Ok(())
}

#[tauri::command]
fn listening_statistics(state: tauri::State<'_, AppState>) -> serde_json::Value { state.personal.statistics() }

#[tauri::command]
fn update_personal_collection(state: tauri::State<'_, AppState>, section: String, id: String, value: serde_json::Value) -> Result<(),String> {
    state.personal.change_collection(&section,id,value).map_err(|error| error.to_string())
}

#[tauri::command]
async fn smart_playlist_tracks(state: tauri::State<'_, AppState>, id: String) -> Result<Data<Vec<Track>>,String> {
    match tracks_data(&state,"likes".into(),None,None).await {
        Data::Ready(likes) => state.personal.smart_tracks(&id,&likes).map(Data::Ready).map_err(|error| error.to_string()),
        Data::Loading => Ok(Data::Loading), Data::Unavailable => Ok(Data::Unavailable), Data::Failed(error) => Ok(Data::Failed(error)),
    }
}

#[tauri::command]
fn sleep_timer(state: tauri::State<'_, AppState>, seconds: Option<u64>, after_track: bool) -> Result<(),String> {
    state.player()?.set_sleep_timer(seconds,after_track).map_err(|error| error.to_string())
}

#[tauri::command]
async fn chat_request(state: tauri::State<'_, AppState>, action: chat::Action, input: serde_json::Value) -> Result<serde_json::Value,String> {
    chat::call(&state.client,action,input).await
}

#[tauri::command]
async fn sync_personal_data(state: tauri::State<'_, AppState>) -> Result<(),String> {
    state.personal.sync(&state.client,&state.settings,state.player.as_ref(),&state.wave).await.map_err(|_| "Could not sync personal data; the local copy is kept".to_owned())?;
    state.wave.configure(&state.settings.lock());
    Ok(())
}

#[tauri::command]
fn settings_backup(state: tauri::State<'_, AppState>, action: String, path: String) -> Result<config::Settings,String> {
    if !path.to_lowercase().ends_with(".json") { return Err("Use a .json backup file".into()); }
    let mut settings = state.settings.lock();
    if action == "export" {
        let collections = state.personal.collections();
        let value = serde_json::json!({"version":1,"preferences":personal::portable(&settings),"folders":collections["folders"],"smartPlaylists":collections["smartPlaylists"]});
        std::fs::write(path,serde_json::to_vec_pretty(&value).map_err(|error| error.to_string())?).map_err(|error|error.to_string())?;
    } else if action == "import" {
        let backup = personal::read_backup(std::path::Path::new(&path)).map_err(|error|error.to_string())?;
        let next = personal::apply_preferences(&settings,&backup.preferences).map_err(|error|error.to_string())?;
        state.personal.import_collections(backup.folders,backup.smart_playlists).map_err(|error|error.to_string())?;
        next.save().map_err(|error|error.to_string())?; *settings = next;
        state.wave.configure(&settings);
    state.personal.preferences_changed(&settings); state.wave.configure(&settings);
        if let Some(player) = &state.player { player.set_audio_preferences(settings.normalization,settings.crossfade_ms,settings.gapless); player.set_autoplay(settings.autoplay); }
    } else { return Err("Unknown backup action".into()); }
    Ok(settings.clone())
}

#[tauri::command]
fn problem_report(state: tauri::State<'_, AppState>) -> Result<diagnostics::Report, String> {
    let connection = match &*state.connection.lock() {
        Connection::SignedIn => "signed_in", Connection::Demo => "demo", Connection::Public => "public",
        Connection::Error { .. } => "error", _ => "connecting",
    }.to_owned();
    let mut report = diagnostics::Report {
        version: env!("CARGO_PKG_VERSION"), os: std::env::consts::OS, architecture: std::env::consts::ARCH,
        connection, queue_length: 0, playing: false, loading: false, error_stage: "none", sample_rate: 0,
        bitrate_kbps: 0, settings_version: state.settings.lock().settings_version,
    };
    if let Some(player) = &state.player {
        let player = player.state.lock();
        report.queue_length = player.queue.len(); report.playing = player.is_playing; report.loading = player.loading;
        report.error_stage = diagnostics::error_stage(player.error.as_deref());
        report.sample_rate = player.sample_rate; report.bitrate_kbps = player.bitrate_kbps;
    }
    Ok(report)
}

#[tauri::command]
fn save_problem_report(path: String, report: serde_json::Value) -> Result<(), String> {
    // Export only the exact report reviewed by the user. Accept a strict schema
    // and enumerated states, never arbitrary error logs or account objects.
    let object = report.as_object().ok_or("Invalid report")?;
    let allowed = ["version", "os", "architecture", "connection", "queueLength", "playing", "loading", "errorStage", "sampleRate", "bitrateKbps", "settingsVersion"];
    if object.len() != allowed.len() || object.keys().any(|key| !allowed.contains(&key.as_str())) { return Err("Invalid report fields".into()); }
    for key in ["version", "os", "architecture", "connection", "errorStage"] {
        let value = object[key].as_str().ok_or("Invalid report value")?;
        if value.len() > 48 || !value.chars().all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c)) { return Err("Invalid report value".into()); }
    }
    for key in ["queueLength", "sampleRate", "bitrateKbps", "settingsVersion"] {
        if object[key].as_u64().is_none() { return Err("Invalid report number".into()); }
    }
    if object["playing"].as_bool().is_none() || object["loading"].as_bool().is_none() { return Err("Invalid report state".into()); }
    if !path.to_lowercase().ends_with(".json") { return Err("Use a .json report file".into()); }
    std::fs::write(path, serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

#[tauri::command]
fn settings(state: tauri::State<'_, AppState>) -> config::Settings {
    state.settings.lock().clone()
}

#[tauri::command]
fn toggle_quick_access(
    state: tauri::State<'_, AppState>,
    item: config::QuickAccessShortcut,
) -> Result<bool, String> {
    let mut settings = state.settings.lock();
    let pinned = settings.toggle_quick_access(item);
    settings.save().map_err(|error| error.to_string())?;
    state.wave.configure(&settings);
    state.personal.preferences_changed(&settings);
    Ok(pinned)
}

#[tauri::command]
async fn save_background(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<String, String> {
    let source = if path.starts_with("file://") {
        url::Url::parse(&path)
            .map_err(|error| error.to_string())?
            .to_file_path()
            .map_err(|_| "Invalid background file URL".to_owned())?
    } else {
        std::path::PathBuf::from(path)
    };
    let directory = app.path().app_data_dir().map_err(|error| error.to_string())?;
    let settings = state.settings.clone();
    tokio::task::spawn_blocking(move || {
        let stored = wallpaper::save(&source, &directory)?.to_string_lossy().into_owned();
        let mut current = settings.lock();
        current.background_image = Some(stored.clone());
        current.save().map_err(|error| error.to_string())?;
        Ok(stored)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn save_font(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<String, String> {
    let source = std::path::PathBuf::from(path);
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|value| {
            matches!(
                value.as_str(),
                "ttf" | "otf" | "ttc" | "otc" | "woff" | "woff2"
            )
        })
        .ok_or("Choose a font file")?;
    let destination = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join(format!("interface-font.{extension}"));
    let settings = state.settings.clone();
    tokio::task::spawn_blocking(move || {
        let metadata = std::fs::metadata(&source).map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.len() > 20 * 1024 * 1024 {
            return Err("Choose a font smaller than 20 MiB".to_owned());
        }
        if source != destination {
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            std::fs::copy(&source, &destination).map_err(|error| error.to_string())?;
        }
        let stored = destination.to_string_lossy().into_owned();
        let mut current = settings.lock();
        current.interface_font = Some(destination);
        current.save().map_err(|error| error.to_string())?;
        Ok(stored)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn save_skin(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let source = std::path::PathBuf::from(path);
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let settings = state.settings.clone();
    tokio::task::spawn_blocking(move || {
        let meta = std::fs::metadata(&source).map_err(|error| error.to_string())?;
        if !meta.is_file() || meta.len() > 25 * 1024 * 1024 {
            return Err("Choose a skin smaller than 25 MiB".to_owned());
        }
        let data = std::fs::read(&source).map_err(|error| error.to_string())?;
        let digest = {
            use sha2::Digest as _;
            format!("{:x}", sha2::Sha256::digest(&data))
        };
        let archive = skin_zip::Archive::open(data).map_err(|error| error.to_string())?;
        let main = archive
            .read("main.bmp")
            .ok_or("The skin has no main.bmp")?
            .map_err(|error| error.to_string())?;
        if !main.starts_with(b"BM") {
            return Err("main.bmp is not a BMP image".to_owned());
        }
        let destination = directory.join(format!("winamp-skin-{}.wsz", &digest[..16]));
        if source != destination && !destination.exists() {
            std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
            std::fs::copy(&source, &destination).map_err(|error| error.to_string())?;
        }
        let mut current = settings.lock();
        current.winamp_skin = Some(destination);
        current.save().map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn skin_images(
    state: tauri::State<'_, AppState>,
) -> Result<std::collections::HashMap<String, String>, String> {
    use base64::Engine as _;
    let path = state.settings.lock().winamp_skin.clone();
    tokio::task::spawn_blocking(move || {
        let Some(path) = path else {
            return Ok(std::collections::HashMap::new());
        };
        let data = std::fs::read(path).map_err(|error| error.to_string())?;
        if data.len() > 25 * 1024 * 1024 {
            return Err("Skin archive is too large".to_owned());
        }
        let archive = skin_zip::Archive::open(data).map_err(|error| error.to_string())?;
        let mut images = std::collections::HashMap::new();
        let mut total_bytes = 0usize;
        for name in [
            "main", "titlebar", "cbuttons", "shufrep", "posbar", "volume", "balance", "eqmain",
            "pledit", "playpaus", "numbers", "nums_ex", "text", "monoster",
        ] {
            if let Some(sheet) = archive.read(&format!("{name}.bmp")) {
                let sheet = sheet.map_err(|error| error.to_string())?;
                total_bytes += sheet.len();
                if sheet.len() > 4 * 1024 * 1024 || total_bytes > 16 * 1024 * 1024 {
                    return Err("Skin image data is too large".to_owned());
                }
                if sheet.starts_with(b"BM") {
                    images.insert(
                        name.to_owned(),
                        format!(
                            "data:image/bmp;base64,{}",
                            base64::engine::general_purpose::STANDARD.encode(sheet)
                        ),
                    );
                }
            }
        }
        Ok(images)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn set_setting(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    key: String,
    value: serde_json::Value,
) -> Result<(), String> {
    let mut settings = state.settings.lock();
    match key.as_str() {
        "normalization" => settings.normalization = value.as_bool().ok_or("Expected true or false")?,
        "gapless" => settings.gapless = value.as_bool().ok_or("Expected true or false")?,
        "crossfade_ms" => settings.crossfade_ms = value.as_u64().ok_or("Expected transition duration")?.min(8000) as u32,
        "wave_mode" => settings.wave_mode = serde_json::from_value(value).map_err(|e| e.to_string())?,
        "wave_mood" => settings.wave_mood = serde_json::from_value(value).map_err(|e| e.to_string())?,
        "leaderboard_anonymous" => settings.leaderboard_anonymous = value.as_bool().ok_or("Expected true or false")?,
        "show_community" => settings.show_community = value.as_bool().ok_or("Expected true or false")?,
        "music_taste" => {
            let mut taste: config::MusicTaste = serde_json::from_value(value).map_err(|e| e.to_string())?;
            taste.normalize();
            state.wave.set_preferences(taste.clone());
            settings.music_taste = taste;
        }
        "offline_limit_mb" => settings.offline_limit_mb = value.as_u64().ok_or("Expected a storage limit")?.min(102_400),
        "autoplay" => {
            settings.autoplay = value.as_bool().ok_or("Expected true or false")?;
            if let Some(player) = &state.player {
                player.set_autoplay(settings.autoplay);
            }
        }
        "compact_rows" => {
            settings.compact_rows = value.as_bool().ok_or("Expected true or false")?
        }
        "close_to_tray" => {
            settings.close_to_tray = value.as_bool().ok_or("Expected true or false")?
        }
        "main_window_bounds" => {
            let bounds: config::MainWindowBounds =
                serde_json::from_value(value).map_err(|error| error.to_string())?;
            if bounds.width < 850 || bounds.height < 580 {
                return Err("Main window bounds are too small".into());
            }
            settings.main_window_bounds = Some(bounds);
        }
        "visualiser" => {
            settings.visualiser =
                serde_json::from_value(value).map_err(|error| error.to_string())?
        }
        "mono" => {
            settings.mono = value.as_bool().ok_or("Expected true or false")?;
            if let Some(player) = &state.player {
                player.set_mono(settings.mono);
            }
        }
        "balance" => {
            settings.balance = (value.as_f64().ok_or("Expected a number")? as f32).clamp(-1.0, 1.0);
            if let Some(player) = &state.player {
                player.set_balance(settings.balance);
            }
        }
        "eq_enabled" => {
            settings.eq_enabled = value.as_bool().ok_or("Expected true or false")?;
            if let Some(player) = &state.player {
                player.set_eq(settings.eq_enabled, settings.eq_gains_db);
            }
        }
        "eq_preamp_db" => {
            settings.eq_preamp_db =
                (value.as_f64().ok_or("Expected a number")? as f32).clamp(-12.0, 12.0);
            if let Some(player) = &state.player {
                player.set_eq_preamp_db(settings.eq_preamp_db);
            }
        }
        "eq_gains_db" => {
            let gains: [f32; 10] = serde_json::from_value(value).map_err(|e| e.to_string())?;
            settings.eq_gains_db = gains.map(|gain| gain.clamp(-12.0, 12.0));
            if let Some(player) = &state.player {
                player.set_eq(settings.eq_enabled, settings.eq_gains_db);
            }
        }
        "theme" => settings.theme = serde_json::from_value(value).map_err(|e| e.to_string())?,
        "language" => {
            settings.language = serde_json::from_value(value).map_err(|e| e.to_string())?
        }
        "startup_page" => {
            settings.startup_page = serde_json::from_value(value).map_err(|e| e.to_string())?
        }
        "memory_profile" => {
            settings.memory_profile = serde_json::from_value(value).map_err(|e| e.to_string())?
        }
        "reduced_motion" => {
            settings.reduced_motion = value.as_bool().ok_or("Expected true or false")?
        }
        "accent_rgb" => {
            settings.accent_rgb = serde_json::from_value(value).map_err(|e| e.to_string())?
        }
        "panel_rgb" => settings.panel_rgb = serde_json::from_value(value).map_err(|e| e.to_string())?,
        "text_rgb" => settings.text_rgb = serde_json::from_value(value).map_err(|e| e.to_string())?,
        "muted_text_rgb" => settings.muted_text_rgb = serde_json::from_value(value).map_err(|e| e.to_string())?,
        "panel_opacity" => {
            settings.panel_opacity = (value.as_f64().ok_or("Expected a number")? as f32).clamp(0.0, 1.0)
        }
        "panel_blur" => settings.panel_blur = value.as_u64().ok_or("Expected a number")?.min(40) as u8,
        "heading_opacity" => {
            settings.heading_opacity = if value.is_null() {
                None
            } else {
                Some((value.as_f64().ok_or("Expected a number")? as f32).clamp(0.0, 1.0))
            }
        }
        "interface_text_scale" => {
            settings.interface_text_scale = (value.as_f64().ok_or("Expected a number")? as f32)
                .clamp(config::INTERFACE_TEXT_SCALE_MIN, config::INTERFACE_TEXT_SCALE_MAX)
        }
        "interface_scale" => {
            settings.interface_scale = (value.as_f64().ok_or("Expected a number")? as f32)
                .clamp(config::INTERFACE_SCALE_MIN, config::INTERFACE_SCALE_MAX)
        }
        "background_image" => {
            settings.background_image = serde_json::from_value(value).map_err(|e| e.to_string())?
        }
        "interface_font" => {
            settings.interface_font = serde_json::from_value(value).map_err(|e| e.to_string())?
        }
        "background_opacity" => {
            settings.background_opacity =
                (value.as_f64().ok_or("Expected a number")? as f32).clamp(0.0, 0.7)
        }
        "background_dim" => {
            settings.background_dim =
                (value.as_f64().ok_or("Expected a number")? as f32).clamp(0.0, 0.85)
        }
        "background_blur" => {
            settings.background_blur = value.as_u64().ok_or("Expected a number")?.min(50) as u8
        }
        "background_overlay" => {
            settings.background_overlay = (value.as_f64().ok_or("Expected a number")? as f32).clamp(0.0, 1.0)
        }
        "lyrics_scale" => {
            settings.lyrics_scale = (value.as_f64().ok_or("Expected a number")? as f32).clamp(0.8, 1.5)
        }
        "lyrics_blur_past" => {
            settings.lyrics_blur_past = value.as_bool().ok_or("Expected true or false")?
        }
        "lyrics_auto_scroll" => {
            settings.lyrics_auto_scroll = value.as_bool().ok_or("Expected true or false")?
        }
        "show_track_numbers" => {
            settings.show_track_numbers = value.as_bool().ok_or("Expected true or false")?
        }
        "quick_access" => {
            let shortcuts: Vec<config::QuickAccessShortcut> =
                serde_json::from_value(value).map_err(|error| error.to_string())?;
            if shortcuts.len() > 100 {
                return Err("Too many quick access shortcuts".into());
            }
            settings.quick_access = shortcuts;
        }
        "soundcloud_profile_url" => {
            settings.soundcloud_profile_url =
                serde_json::from_value(value).map_err(|e| e.to_string())?
        }
        "discord_client_id" => {
            settings.discord_client_id = value.as_str().ok_or("Expected text")?.to_owned();
            *state.last_presence.lock() = None;
            if settings.discord_presence {
                state
                    .presence
                    .configure(Some(settings.discord_client_id.clone()));
            }
        }
        "discord_presence" => {
            settings.discord_presence = value.as_bool().ok_or("Expected true or false")?;
            *state.last_presence.lock() = None;
            state.presence.configure(
                settings
                    .discord_presence
                    .then(|| settings.discord_client_id.clone()),
            );
        }
        "audio_cache_limit_mb" => {
            settings.audio_cache_limit_mb = value.as_u64().ok_or("Expected a number")?.min(10240);
            if let Some(player) = &state.player {
                player.set_audio_cache_limit_mb(settings.audio_cache_limit_mb);
            }
        }
        "eq_auto" => settings.eq_auto = value.as_bool().ok_or("Expected true or false")?,
        "mini_player_style" => {
            settings.mini_player_style = serde_json::from_value(value).map_err(|e| e.to_string())?
        }
        "winamp_window" => {
            let mini = value.as_bool().ok_or("Expected true or false")?;
            if let Some(window) = app.get_webview_window("main") {
                if mini && !settings.winamp_window {
                    record_main_window_bounds(&window, &mut settings)?;
                }
                resize_player_window(&window, mini, settings.main_window_bounds)?;
            }
            settings.winamp_window = mini;
        }
        "winamp_on_top" => {
            settings.winamp_on_top = value.as_bool().ok_or("Expected true or false")?
        }
        "winamp_skin" => {
            if !value.is_null() {
                return Err("Choose a skin file to install it".into());
            }
            settings.winamp_skin = None;
        }
        "winamp_shade" => {
            settings.winamp_shade = value.as_bool().ok_or("Expected true or false")?
        }
        "winamp_eq_window" => {
            settings.winamp_eq_window = value.as_bool().ok_or("Expected true or false")?
        }
        "winamp_eq_shade" => {
            settings.winamp_eq_shade = value.as_bool().ok_or("Expected true or false")?
        }
        "winamp_pl_window" => {
            settings.winamp_pl_window = value.as_bool().ok_or("Expected true or false")?
        }
        "winamp_pl_shade" => {
            settings.winamp_pl_shade = value.as_bool().ok_or("Expected true or false")?
        }
        "winamp_pl_rows" => {
            settings.winamp_pl_rows = value.as_u64().ok_or("Expected a number")?.clamp(4, 64) as u32
        }
        "winamp_scale" => {
            settings.winamp_scale = value.as_u64().ok_or("Expected a number")?.clamp(1, 4) as u32
        }
        _ => return Err("Unknown setting".into()),
    }
    settings.save().map_err(|e| e.to_string())?;
    if let Some(player) = &state.player { player.set_audio_preferences(settings.normalization,settings.crossfade_ms,settings.gapless); }
    state.wave.configure(&settings);
    state.personal.preferences_changed(&settings);
    drop(settings);
    if key == "eq_auto" {
        *state.last_eq_track.lock() = None;
    }
    Ok(())
}

#[tauri::command]
fn eq_preset(state: tauri::State<'_, AppState>, clear: bool) -> Result<(), String> {
    let track = state
        .player()?
        .current_track()
        .ok_or("Choose a track first")?;
    let mut settings = state.settings.lock();
    settings
        .eq_presets
        .retain(|preset| preset.track_id != track.id);
    if !clear {
        let preset = config::EqPreset {
            track_id: track.id,
            preamp_db: settings.eq_preamp_db,
            gains_db: settings.eq_gains_db,
        };
        settings.eq_presets.push(preset);
        if settings.eq_presets.len() > 500 {
            settings.eq_presets.remove(0);
        }
    }
    settings.save().map_err(|error| error.to_string())
}

#[tauri::command]
fn audio_cache(state: tauri::State<'_, AppState>, clear: bool) -> Result<u64, String> {
    let player = state.player()?;
    Ok(if clear {
        player.clear_audio_cache()
    } else {
        player.audio_cache_size()
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageReport {
    installation_bytes: u64,
    clap_model_bytes: u64,
    clap_runtime_bytes: u64,
    clap_preparation_bytes: u64,
    offline_bytes: u64,
    audio_cache_bytes: u64,
    artwork_cache_bytes: u64,
    other_data_bytes: u64,
    other_cache_bytes: u64,
    extra_app_data_bytes: u64,
    installation_path: String,
    data_path: String,
    cache_path: String,
    extra_app_data_path: String,
}

fn folder_bytes(path: &std::path::Path) -> std::io::Result<u64> {
    if !path.exists() { return Ok(0); }
    let mut size = 0u64;
    let mut pending = vec![path.to_path_buf()];
    while let Some(folder) = pending.pop() {
        for entry in std::fs::read_dir(folder)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() { pending.push(entry.path()); }
            else if kind.is_file() { size = size.saturating_add(entry.metadata()?.len()); }
        }
    }
    Ok(size)
}

#[tauri::command]
fn component_status(resources: tauri::State<'_, Arc<components::Manager>>) -> components::Status {
    resources.status()
}

#[tauri::command]
async fn prepare_components(resources: tauri::State<'_, Arc<components::Manager>>) -> Result<(), String> {
    resources.inner().clone().prepare().await.map_err(|error| format!("{error:#}"))
}

#[tauri::command]
async fn preserve_components(resources: tauri::State<'_, Arc<components::Manager>>) -> Result<(), String> {
    resources.inner().clone().preserve_before_update().await.map_err(|error| format!("{error:#}"))
}

#[tauri::command]
async fn storage_report(app: tauri::AppHandle) -> Result<StorageReport, String> {
    let extra = app.path().app_data_dir().map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || -> Result<StorageReport, String> {
        let paths = config::app_paths().map_err(|error| error.to_string())?;
        let installation = std::env::current_exe().map_err(|error| error.to_string())?
            .parent().ok_or("Installation path unavailable")?.to_path_buf();
        let offline = paths.root.join("offline");
        let data_bytes = folder_bytes(&paths.root).map_err(|error| error.to_string())?;
        let component_root = paths.root.join("components").join("clap");
        let component_logical_bytes = folder_bytes(&component_root).map_err(|error| error.to_string())?;
        // The active set uses hardlinks to blobs, so count each stored file once.
        let clap_runtime_bytes = folder_bytes(&component_root.join("blobs")).map_err(|error| error.to_string())?;
        let cache_bytes = folder_bytes(&paths.cache).map_err(|error| error.to_string())?;
        let offline_bytes = folder_bytes(&offline).map_err(|error| error.to_string())?;
        let audio_cache_bytes = folder_bytes(&paths.audio_cache).map_err(|error| error.to_string())?;
        let artwork_cache_bytes = folder_bytes(&paths.cover_cache).map_err(|error| error.to_string())?;
        let installation_bytes = folder_bytes(&installation).map_err(|error| error.to_string())?
            .saturating_sub(if paths.cache.starts_with(&installation) { cache_bytes } else { 0 })
            .saturating_sub(if paths.root.starts_with(&installation) { data_bytes } else { 0 });
        let extra_app_data_bytes = if extra.starts_with(&installation) || extra.starts_with(&paths.root) || extra.starts_with(&paths.cache) {
            0
        } else { folder_bytes(&extra).map_err(|error| error.to_string())? };
        let clap_preparation_bytes = ["clap-build-venv", "clap-lite-venv", "clap-pyi-build", "clap-lite-pyi-build"]
            .iter().try_fold(0u64, |sum, name| folder_bytes(&installation.join(name)).map(|bytes| sum.saturating_add(bytes)))
            .map_err(|error| error.to_string())?;
        Ok(StorageReport {
            installation_bytes,
            clap_model_bytes: folder_bytes(&installation.join("resources").join("clap")).map_err(|error| error.to_string())?
                .saturating_add(clap_runtime_bytes),
            clap_runtime_bytes,
            clap_preparation_bytes,
            offline_bytes,
            audio_cache_bytes,
            artwork_cache_bytes,
            other_data_bytes: data_bytes.saturating_sub(offline_bytes).saturating_sub(component_logical_bytes),
            other_cache_bytes: cache_bytes.saturating_sub(audio_cache_bytes).saturating_sub(artwork_cache_bytes),
            extra_app_data_bytes,
            installation_path: installation.display().to_string(),
            data_path: paths.root.display().to_string(),
            cache_path: paths.cache.display().to_string(),
            extra_app_data_path: extra.display().to_string(),
        })
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
fn clear_clap_preparation() -> Result<(), String> {
    let installation = std::env::current_exe().map_err(|error| error.to_string())?
        .parent().ok_or("Installation path unavailable")?.to_path_buf();
    for name in ["clap-build-venv", "clap-lite-venv", "clap-pyi-build", "clap-lite-pyi-build"] {
        let path = installation.join(name);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_dir() => std::fs::remove_dir_all(path).map_err(|error| error.to_string())?,
            Ok(_) => return Err(format!("Unexpected file at {name}")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

#[tauri::command]
fn clear_artwork_cache() -> Result<(), String> {
    let base = config::app_paths().map_err(|error| error.to_string())?.cover_cache;
    for entry in std::fs::read_dir(base).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_dir() { std::fs::remove_dir_all(entry.path()).map_err(|error| error.to_string())?; }
        else if kind.is_file() { std::fs::remove_file(entry.path()).map_err(|error| error.to_string())?; }
    }
    Ok(())
}

#[tauri::command]
fn import_status(state: tauri::State<'_, AppState>) -> ImportStatus {
    let view = spotify::spotify_import_status(state);
    if view.source != "yandex" { return ImportStatus::default(); }
    ImportStatus { running: view.running, current: view.current, total: view.total, matched: view.matched, title: view.title, message: view.error }
}

#[tauri::command]
async fn check_yandex_token(token: String) -> Result<usize, String> {
    import_yandex::check_token(&token).await.map_err(|error| error.to_string())
}

#[tauri::command]
fn start_yandex_import(state: tauri::State<'_, AppState>, token: String) -> Result<(), String> {
    spotify::start_yandex(&state, token)
}

#[tauri::command]
async fn upload_track(
    state: tauri::State<'_, AppState>,
    path: String,
    title: String,
    artist: String,
    description: String,
    genre: String,
    tags: String,
    public: bool,
) -> Result<Track, String> {
    if !state.store.signed_in() {
        return Err("Sign in to upload".into());
    }
    let path = std::path::Path::new(path.trim());
    if !path.is_file() {
        return Err("Choose an existing audio file".into());
    }
    if title.trim().is_empty() {
        return Err("Enter a track title".into());
    }
    let result = api::endpoints::upload_track(
        &state.client,
        path,
        api::endpoints::TrackUpload {
            title: title.trim(),
            artist: Some(artist.trim()),
            description: Some(description.trim()),
            genre: Some(genre.trim()),
            tags: Some(tags.trim()),
            public,
        },
    )
    .await
    .map_err(|error| error.to_string())?;
    state.store.invalidate(&Key::MyTracks);
    Ok(result)
}

#[tauri::command]
async fn edit_track(
    state: tauri::State<'_, AppState>,
    id: u64,
    title: String,
    artist: String,
    description: String,
) -> Result<Track, String> {
    if !state.store.signed_in() {
        return Err("Sign in to edit tracks".into());
    }
    if title.trim().is_empty() {
        return Err("Enter a track title".into());
    }
    let result = api::endpoints::update_track_metadata(
        &state.client,
        id,
        title.trim(),
        Some(description.trim()),
        Some(artist.trim()),
    )
    .await
    .map_err(|error| error.to_string())?;
    state.store.invalidate(&Key::Track(id));
    state.store.invalidate(&Key::MyTracks);
    Ok(result)
}

#[tauri::command]
async fn delete_track(state: tauri::State<'_, AppState>, id: u64) -> Result<(), String> {
    if !state.store.signed_in() {
        return Err("Sign in to delete tracks".into());
    }
    api::endpoints::delete_track(&state.client, id)
        .await
        .map_err(|error| error.to_string())?;
    state.store.invalidate(&Key::Track(id));
    state.store.invalidate(&Key::MyTracks);
    Ok(())
}

#[tauri::command]
async fn save_storefront(
    state: tauri::State<'_, AppState>,
    id: u64,
    title: String,
    kind: String,
    link: String,
    link_title: String,
    description: String,
    price: String,
) -> Result<(), String> {
    if !state.store.signed_in() {
        return Err("Sign in to edit tracks".into());
    }
    if title.trim().is_empty() || kind.trim().is_empty() || link.trim().is_empty() {
        return Err("Title, type and link are required".into());
    }
    api::endpoints::update_track_storefront(
        &state.client,
        id,
        api::endpoints::StorefrontUpdate {
            title: title.trim(),
            kind: kind.trim(),
            link: link.trim(),
            link_title: Some(link_title.trim()),
            description: Some(description.trim()),
            price: Some(price.trim()),
        },
    )
    .await
    .map_err(|error| error.to_string())?;
    state.store.invalidate(&Key::Track(id));
    Ok(())
}

#[tauri::command]
fn connect_account(state: tauri::State<'_, AppState>) -> Result<(), String> {
    if !matches!(
        *state.connection.lock(),
        Connection::Demo | Connection::Error { .. }
    ) {
        return Err("Connection is already available or in progress".into());
    }
    *state.connection.lock() = Connection::Connecting;
    let client = state.client.clone();
    let store = state.store.clone();
    let player = state.player.clone();
    let app = state.inner().connection.clone();
    let session_slot = state.inner().session.clone();
    state.rt.spawn(async move {
        let result = async {
            let http = reqwest::Client::new();
            let pairing = auth::register::begin(&http)
                .await
                .map_err(|e| e.to_string())?;
            *app.lock() = Connection::Pairing {
                code: pairing.code.clone(),
                url: pairing.url.clone(),
            };
            let url = pairing.url.clone();
            let _ = tokio::task::spawn_blocking(move || webbrowser::open(&url)).await;
            let (cancel_tx, cancel) = tokio::sync::watch::channel(false);
            let registered = auth::register::finish(&http, &pairing, cancel)
                .await
                .map_err(|e| e.to_string())?;
            drop(cancel_tx);
            *app.lock() = Connection::Registering;
            registered
                .credentials
                .save_to_keyring()
                .map_err(|e| e.to_string())?;
            client.set_client_id(registered.credentials.client_id.clone());
            let session = auth::start(client.clone())
                .await
                .ok_or("Could not start SoundCloud session")?;
            if let Some(player) = &player {
                player.stop();
                {
                    let mut player_state = player.state.lock();
                    player_state.queue.clear();
                    player_state.priority_next.clear();
                    player_state.queue_revision = player_state.queue_revision.wrapping_add(1);
                    player_state.order.clear();
                    player_state.current = None;
                }
                player.set_demo(false);
            }
            client.set_demo(false);
            let signed_in = session.signed_in().await;
            store.set_signed_in(signed_in);
            store.clear();
            *session_slot.lock() = Some(session);
            *app.lock() = if signed_in {
                Connection::SignedIn
            } else {
                Connection::Public
            };
            Ok::<(), String>(())
        }
        .await;
        if let Err(message) = result {
            *app.lock() = Connection::Error { message };
        }
    });
    Ok(())
}

#[tauri::command]
async fn sign_in(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let _spotify_connection = state.spotify.cancel_and_wait().await;
    let session = state
        .session
        .lock()
        .clone()
        .ok_or("Connect SoundCloud first")?;
    session.sign_in().await.map_err(|error| error.to_string())?;
    state.store.set_signed_in(true);
    state.store.clear();
    *state.connection.lock() = Connection::SignedIn;
    Ok(())
}

#[tauri::command]
async fn sign_out(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let _spotify_connection = state.spotify.cancel_and_wait().await;
    let session = state
        .session
        .lock()
        .clone()
        .ok_or("No SoundCloud session")?;
    session
        .sign_out()
        .await
        .map_err(|error| error.to_string())?;
    state.personal.detach_account().map_err(|error| error.to_string())?;
    state.wave.replace_dislikes(&state.personal.feedback());
    state.store.set_signed_in(false);
    state.store.clear();
    *state.wave_session.lock() = None;
    state.persist_wave_active(false);
    if session.is_remote() {
        state.client.set_demo(true);
        if let Some(player) = &state.player {
            player.stop();
            player.set_demo(true);
            let tracks = demo::demo_tracks();
            let mut playback = player.state.lock();
            playback.queue = tracks;
            playback.queue_revision = playback.queue_revision.wrapping_add(1);
            playback.order = (0..playback.queue.len()).collect();
            playback.current = None;
        }
        *state.connection.lock() = Connection::Demo;
    } else {
        *state.connection.lock() = Connection::Public;
    }
    Ok(())
}

#[tauri::command]
fn approval_server_url() -> Option<String> {
    auth::saved_server_url()
}

#[tauri::command]
async fn connect_server(state: tauri::State<'_, AppState>, server_url: String) -> Result<(), String> {
    let _spotify_connection = state.spotify.cancel_and_wait().await;
    let previous = state.connection.lock().clone();
    *state.connection.lock() = Connection::Connecting;
    let result = async {
        let creds = auth::AppCredentials::from_server(&server_url).await
            .map_err(|error| error.to_string())?;
        let session = auth::Session::new(creds.clone(), state.client.clone());
        session.sign_in().await.map_err(|error| error.to_string())?;
        creds.save_remote().map_err(|error| error.to_string())?;
        state.client.set_client_id(creds.client_id);
        state.client.set_demo(false);
        if let Some(player) = &state.player {
            player.stop();
            let mut playback = player.state.lock();
            playback.queue.clear();
            playback.priority_next.clear();
            playback.queue_revision = playback.queue_revision.wrapping_add(1);
            playback.order.clear();
            playback.current = None;
            drop(playback);
            player.set_demo(false);
        }
        state.store.set_signed_in(true);
        state.store.clear();
        *state.session.lock() = Some(session);
        *state.connection.lock() = Connection::SignedIn;
        Ok::<(), String>(())
    }.await;
    if let Err(message) = &result {
        if let Some(old) = state.session.lock().as_ref() {
            state.client.attach_session(old);
        }
        *state.connection.lock() = match previous {
            Connection::Demo | Connection::Error { .. } => Connection::Error { message: message.clone() },
            other => other,
        };
    }
    result
}

async fn approval_admin_token(state: &AppState) -> Result<String, String> {
    let session = state.session.lock().clone().ok_or("Sign in to SoundCloud first")?;
    if !session.signed_in().await { return Err("Sign in to SoundCloud first".into()); }
    session.access_token().await.map_err(|error| error.to_string())
}

#[tauri::command]
async fn server_session(state: tauri::State<'_, AppState>) -> Result<ServerSession, String> {
    let session = state.session.lock().clone().ok_or("Sign in to SoundCloud first")?;
    let server = session.media_server_url().ok_or("No Veirune server configured")?;
    let server = auth::checked_server_url(&server).map_err(|error| error.to_string())?;
    let token = session.access_token().await.map_err(|error| error.to_string())?;
    let response = reqwest::Client::new().get(format!("{server}/v1/session"))
        .header(reqwest::header::AUTHORIZATION, format!("OAuth {token}"))
        .timeout(std::time::Duration::from_secs(10)).send().await.map_err(|error| error.to_string())?;
    if !response.status().is_success() { return Err(approval_response_error(response).await); }
    response.json().await.map_err(|error| error.to_string())
}

#[tauri::command]
async fn pair_mobile(state: tauri::State<'_, AppState>, code: String) -> Result<serde_json::Value, String> {
    let code = code.to_uppercase().replace(['-', ' '], "");
    if code.len() != 8 || !code.bytes().all(|b| b.is_ascii_uppercase() || (b'2'..=b'9').contains(&b)) {
        return Err("Enter the eight-character code shown on your phone".into());
    }
    let session = state.session.lock().clone().ok_or("Sign in to SoundCloud first")?;
    let server = session.media_server_url().ok_or("No Veirune server configured")?;
    let server = auth::checked_server_url(&server).map_err(|e| e.to_string())?;
    let token = session.access_token().await.map_err(|e| e.to_string())?;
    let http = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30))
        .build().map_err(|e| e.to_string())?;
    let owner = http.get(format!("{server}/v1/session"))
        .header(reqwest::header::AUTHORIZATION, format!("OAuth {token}"))
        .send().await.map_err(|e| e.to_string())?;
    if !owner.status().is_success() { return Err(approval_response_error(owner).await); }
    let owner: ServerSession = owner.json().await.map_err(|e| e.to_string())?;
    let checked = http.post(format!("{server}/v1/mobile/pairing/check"))
        .header(reqwest::header::AUTHORIZATION, format!("OAuth {token}"))
        .json(&serde_json::json!({"code": code})).send().await.map_err(|e| e.to_string())?;
    if !checked.status().is_success() { return Err(approval_response_error(checked).await); }
    // Authorize a new grant without saving it into the existing desktop Session.
    let credentials = auth::AppCredentials::from_server(&server).await.map_err(|e| e.to_string())?;
    let phone = auth::authorize(&credentials).await.map_err(|e| e.to_string())?;
    let identity = http.get(format!("{server}/v1/session"))
        .header(reqwest::header::AUTHORIZATION, format!("OAuth {}", phone.access_token))
        .send().await.map_err(|e| e.to_string())?;
    if !identity.status().is_success() { return Err(approval_response_error(identity).await); }
    let identity: ServerSession = identity.json().await.map_err(|e| e.to_string())?;
    if owner.user_id != identity.user_id {
        return Err("Sign in with the same SoundCloud account as on the PC".into());
    }
    let response = http.post(format!("{server}/v1/mobile/pairing/approve"))
        .header(reqwest::header::AUTHORIZATION, format!("OAuth {}", phone.access_token))
        .json(&serde_json::json!({"code": code, "refresh_token": phone.refresh_token}))
        .send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() { return Err(approval_response_error(response).await); }
    response.json().await.map_err(|e| e.to_string())
}

async fn approval_response_error(response: reqwest::Response) -> String {
    let status = response.status();
    let detail = response.json::<serde_json::Value>().await.ok()
        .and_then(|body| body.get("error").and_then(|value| value.as_str().map(str::to_owned)));
    match detail {
        Some(detail) if !detail.is_empty() => format!("Approval server returned {status}: {}", detail.chars().take(200).collect::<String>()),
        _ => format!("Approval server returned {status}"),
    }
}

#[tauri::command]
async fn approval_users(state: tauri::State<'_, AppState>, server_url: String) -> Result<Vec<ApprovalUser>, String> {
    let url = auth::save_server_url(&server_url).map_err(|error| error.to_string())?;
    let token = approval_admin_token(&state).await?;
    let response = reqwest::Client::new().get(format!("{url}/v1/admin/users"))
        .header(reqwest::header::AUTHORIZATION, format!("OAuth {token}"))
        .timeout(std::time::Duration::from_secs(15)).send().await.map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(approval_response_error(response).await);
    }
    response.json::<ApprovalUsers>().await.map(|body| body.users)
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn approval_set_user(state: tauri::State<'_, AppState>, server_url: String, user_id: u64, status: String) -> Result<(), String> {
    if !matches!(status.as_str(), "approved" | "denied" | "pending") {
        return Err("Invalid approval status".into());
    }
    let url = auth::save_server_url(&server_url).map_err(|error| error.to_string())?;
    let token = approval_admin_token(&state).await?;
    let response = reqwest::Client::new().post(format!("{url}/v1/admin/users/{user_id}"))
        .header(reqwest::header::AUTHORIZATION, format!("OAuth {token}"))
        .json(&serde_json::json!({ "status": status }))
        .send().await.map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(approval_response_error(response).await);
    }
    Ok(())
}

#[tauri::command]
async fn approval_settings(state: tauri::State<'_, AppState>, server_url: String, required: Option<bool>) -> Result<serde_json::Value, String> {
    let url = auth::save_server_url(&server_url).map_err(|error| error.to_string())?;
    let token = approval_admin_token(&state).await?;
    let http = reqwest::Client::new();
    let request = if let Some(required) = required {
        http.post(format!("{url}/v1/admin/settings")).json(&serde_json::json!({ "approval_required": required }))
    } else {
        http.get(format!("{url}/v1/admin/settings"))
    };
    let response = request.header(reqwest::header::AUTHORIZATION, format!("OAuth {token}"))
        .timeout(std::time::Duration::from_secs(15)).send().await.map_err(|error| error.to_string())?;
    if !response.status().is_success() { return Err(approval_response_error(response).await); }
    response.json().await.map_err(|error| error.to_string())
}

#[tauri::command]
async fn approval_media(state: tauri::State<'_, AppState>, server_url: String) -> Result<serde_json::Value, String> {
    let url = auth::save_server_url(&server_url).map_err(|error| error.to_string())?;
    let token = approval_admin_token(&state).await?;
    let response = reqwest::Client::new().get(format!("{url}/v1/admin/media"))
        .header(reqwest::header::AUTHORIZATION, format!("OAuth {token}"))
        .timeout(std::time::Duration::from_secs(15)).send().await.map_err(|error| error.to_string())?;
    if !response.status().is_success() { return Err(approval_response_error(response).await); }
    response.json().await.map_err(|error| error.to_string())
}

#[tauri::command]
async fn server_operations(state: tauri::State<'_, AppState>, server_url: String, settings: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
    let url = auth::save_server_url(&server_url).map_err(|error| error.to_string())?;
    let token = approval_admin_token(&state).await?;
    let http = reqwest::Client::new();
    let endpoint = format!("{url}/v1/admin/operations");
    let request = if let Some(settings) = settings { http.post(endpoint).json(&settings) } else { http.get(endpoint) };
    let response = request.header(reqwest::header::AUTHORIZATION, format!("OAuth {token}"))
        .timeout(std::time::Duration::from_secs(15)).send().await.map_err(|error| error.to_string())?;
    if !response.status().is_success() { return Err(approval_response_error(response).await); }
    response.json().await.map_err(|error| error.to_string())
}

#[tauri::command]
async fn server_incident(state: tauri::State<'_, AppState>, server_url: String, body: serde_json::Value) -> Result<serde_json::Value,String> {
    let url = auth::save_server_url(&server_url).map_err(|error| error.to_string())?;
    let token = approval_admin_token(&state).await?;
    let response = reqwest::Client::new().post(format!("{url}/v1/admin/incidents"))
        .header(reqwest::header::AUTHORIZATION,format!("OAuth {token}")).json(&body)
        .timeout(std::time::Duration::from_secs(15)).send().await.map_err(|error| error.to_string())?;
    if !response.status().is_success() { return Err(approval_response_error(response).await); }
    response.json().await.map_err(|error| error.to_string())
}

fn record_main_window_bounds(
    window: &tauri::WebviewWindow,
    settings: &mut config::Settings,
) -> Result<(), String> {
    if settings.winamp_window {
        return Ok(());
    }
    let maximized = window.is_maximized().map_err(|error| error.to_string())?;
    if maximized {
        let mut bounds = settings.main_window_bounds.unwrap_or_default();
        bounds.maximized = true;
        settings.main_window_bounds = Some(bounds);
    } else if !window.is_minimized().map_err(|error| error.to_string())? {
        let size = window.inner_size().map_err(|error| error.to_string())?;
        if size.width >= 850 && size.height >= 580 {
            let position = window.outer_position().map_err(|error| error.to_string())?;
            settings.main_window_bounds = Some(config::MainWindowBounds {
                x: position.x,
                y: position.y,
                width: size.width,
                height: size.height,
                maximized: false,
            });
        }
    }
    Ok(())
}

fn resize_player_window(
    window: &tauri::WebviewWindow,
    mini: bool,
    saved_bounds: Option<config::MainWindowBounds>,
) -> Result<(), String> {
    if mini {
        window
            .set_fullscreen(false)
            .map_err(|error| error.to_string())?;
        window.unmaximize().map_err(|error| error.to_string())?;
        window
            .set_min_size(Some(tauri::LogicalSize::new(420.0, 104.0)))
            .map_err(|error| error.to_string())?;
        window
            .set_size(tauri::LogicalSize::new(420.0, 104.0))
            .map_err(|error| error.to_string())?;
        window.set_resizable(false).map_err(|error| error.to_string())?;
        window.set_maximizable(false).map_err(|error| error.to_string())?;
    } else {
        window.set_resizable(true).map_err(|error| error.to_string())?;
        window.set_maximizable(true).map_err(|error| error.to_string())?;
        window
            .set_min_size(Some(tauri::LogicalSize::new(850.0, 580.0)))
            .map_err(|error| error.to_string())?;
        let bounds = saved_bounds.unwrap_or_default();
        window
            .set_size(tauri::PhysicalSize::new(bounds.width, bounds.height))
            .map_err(|error| error.to_string())?;
        if bounds.maximized {
            window.maximize().map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_single_instance::init(
            |app, arguments, _cwd| {
                if let Some(link) = arguments
                    .iter()
                    .skip(1)
                    .find(|argument| link::parse(argument).is_ok())
                {
                    if let Some(state) = app.try_state::<AppState>() {
                        *state.pending_link.lock() = Some(link.clone());
                    }
                }
                show_window(app);
            },
        ))
        .plugin(tauri_plugin_deep_link::init())
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let close_to_tray = if let Some(state) = window.app_handle().try_state::<AppState>() {
                    let mut settings = state.settings.lock();
                    if let Some(webview_window) = window.app_handle().get_webview_window("main") {
                        if let Err(error) = record_main_window_bounds(&webview_window, &mut settings) {
                            log::warn!("Could not capture main window bounds: {error}");
                        }
                    }
                    if let Err(error) = settings.save() {
                        log::warn!("Could not save window settings: {error}");
                    }
                    let close_to_tray = settings.close_to_tray;
                    drop(settings);
                    if !close_to_tray {
                        state.personal.flush();
                        state.persist_wave_active(state.wave_session.lock().as_ref().is_some_and(|session| !session.continuation));
                        if let Some(player) = &state.player { player.shutdown(); }
                    }
                    close_to_tray
                } else {
                    false
                };
                if close_to_tray {
                    if let Err(error) = window.hide() {
                        log::error!("Could not hide Veirune in the tray: {error}");
                    }
                } else {
                    window.app_handle().exit(0);
                }
            }
        })
        .setup(|app| {
            let resource_dir = app.path().resource_dir()?;
            let resources = components::Manager::new(config::app_paths()?.root.join("components").join("clap"),
                resource_dir.join("resources").join("clap"))?;
            clap::configure(&resource_dir, resources.clone());
            app.manage(resources.clone());
            tauri::async_runtime::spawn(async move {
                if let Err(error) = resources.prepare().await {
                    log::warn!("CLAP component preparation: {error:#}");
                }
            });
            app.manage(AppState::new()?);
            // Only this generated artwork directory is exposed, not the
            // account database, credentials, audio cache or arbitrary files.
            app.asset_protocol_scope().allow_directory(app.state::<AppState>().image_cache.directory(), false)?;
            app.manage(update_events::Subscription::default());
            let (mini, saved_bounds) = {
                let state = app.state::<AppState>();
                let settings = state.settings.lock();
                (settings.winamp_window, settings.main_window_bounds)
            };
            if mini {
                if let Some(window) = app.get_webview_window("main") {
                    resize_player_window(&window, true, saved_bounds)?;
                }
            } else {
                if let (Some(window), Some(bounds)) = (app.get_webview_window("main"), saved_bounds) {
                    window.set_size(tauri::PhysicalSize {
                        width: bounds.width,
                        height: bounds.height,
                    })?;
                    window.set_position(tauri::PhysicalPosition {
                        x: bounds.x,
                        y: bounds.y,
                    })?;
                    if bounds.maximized {
                        window.maximize()?;
                    }
                }
            }
            if let Some(player) = app.state::<AppState>().player.clone() {
                #[cfg(windows)]
                let hwnd = app
                    .get_webview_window("main")
                    .and_then(|window| window.hwnd().ok())
                    .map(|handle| handle.0 as usize);
                #[cfg(not(windows))]
                let hwnd = None;
                media::spawn(app.handle().clone(), player, hwnd);
            }
            let modifiers = Some(Modifiers::CONTROL | Modifiers::ALT);
            let hotkeys = [
                (Shortcut::new(modifiers, Code::KeyP), "toggle"),
                (Shortcut::new(modifiers, Code::ArrowRight), "next"),
                (Shortcut::new(modifiers, Code::ArrowLeft), "previous"),
                (Shortcut::new(modifiers, Code::KeyS), "stop"),
                (Shortcut::new(modifiers, Code::ArrowUp), "volume_up"),
                (Shortcut::new(modifiers, Code::ArrowDown), "volume_down"),
                (Shortcut::new(modifiers, Code::KeyM), "mute"),
                (Shortcut::new(modifiers, Code::KeyH), "shuffle"),
                (Shortcut::new(modifiers, Code::KeyR), "repeat"),
            ];
            app.handle().plugin(
                tauri_plugin_global_shortcut::Builder::new()
                    .with_handler(move |app, shortcut, event| {
                        if event.state() != ShortcutState::Pressed {
                            return;
                        }
                        let Some((_, action)) = hotkeys.iter().find(|(key, _)| key == shortcut)
                        else {
                            return;
                        };
                        let Some(state) = app.try_state::<AppState>() else {
                            return;
                        };
                        let Some(player) = &state.player else {
                            return;
                        };
                        match *action {
                            "toggle" => player.play_pause(),
                            "next" => {
                                player.next();
                            }
                            "previous" => {
                                player.prev();
                            }
                            "stop" => {
                                *state.wave_session.lock() = None;
                                state.persist_wave_active(false);
                                player.stop();
                            }
                            "volume_up" => {
                                let volume = player.state.lock().volume;
                                player.set_volume((volume + 0.1).min(1.0));
                            }
                            "volume_down" => {
                                let volume = player.state.lock().volume;
                                player.set_volume((volume - 0.1).max(0.0));
                            }
                            "mute" => player.set_volume(0.0),
                            "shuffle" => player.toggle_shuffle(),
                            "repeat" => player.cycle_repeat(),
                            _ => {}
                        }
                    })
                    .build(),
            )?;
            for (shortcut, _) in &hotkeys {
                if let Err(error) = app.global_shortcut().register(*shortcut) {
                    log::warn!("shortcut {shortcut:?} unavailable: {error}");
                }
            }
            let show = MenuItem::with_id(app, "show", "Show Veirune", true, None::<&str>)?;
            let toggle = MenuItem::with_id(app, "toggle", "Play/Pause", true, None::<&str>)?;
            let next = MenuItem::with_id(app, "next", "Next", true, None::<&str>)?;
            let previous = MenuItem::with_id(app, "previous", "Previous", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &toggle, &next, &previous, &quit])?;
            let mut tray = TrayIconBuilder::with_id("main-tray")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .tooltip("Veirune")
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => show_window(app),
                    "toggle" | "next" | "previous" => {
                        if let Some(state) = app.try_state::<AppState>() {
                            if let Some(player) = &state.player {
                                match event.id().as_ref() {
                                    "toggle" => player.play_pause(),
                                    "next" => {
                                        player.next();
                                    }
                                    _ => {
                                        player.prev();
                                    }
                                }
                            }
                        }
                    }
                    "quit" => {
                        if let (Some(window), Some(state)) =
                            (app.get_webview_window("main"), app.try_state::<AppState>())
                        {
                            {
                                let mut settings = state.settings.lock();
                                if let Err(error) = record_main_window_bounds(&window, &mut settings) {
                                    log::warn!("Could not capture main window bounds: {error}");
                                }
                                if let Err(error) = settings.save() {
                                    log::warn!("Could not save window settings: {error}");
                                }
                            }
                            state.persist_wave_active(state.wave_session.lock().as_ref().is_some_and(|session| !session.continuation));
                            state.personal.flush();
                            if let Some(player) = &state.player { player.shutdown(); }
                        }
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if matches!(
                        event,
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        }
                    ) {
                        show_window(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            app_icon::restore(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_icon::application_icon,
            chat_request,
            community_request, leaderboard, leaderboard_anonymity, personal_collections, listening_statistics, update_personal_collection, smart_playlist_tracks,
            sleep_timer, sync_personal_data, settings_backup,
            connection,
            my_profile,
            take_pending_link,
            player_state,
            image_data,
            waveform_samples,
            offline_tracks,
            offline_capacity,
            pin_offline_track,
            clear_unpinned_tracks,
            theme_action,
            problem_report,
            save_problem_report,
            my_wave,
            start_station,
            wave_dislike,
            wave_disliked,
            wave_dislikes,
            set_offline_like_order,
            download_offline_track,
            remove_offline_track,
            clear_offline_tracks,
            visualiser_frame,
            tracks,
            playlists,
            catalog_releases,
            users,
            track_detail,
            playlist_detail,
            user_detail,
            comments,
            track_lyrics,
            search_lyrics,
            lyric_tracks,
            open_lyrics_source,
            user_profiles,
            related_users,
            open_link,
            open_soundcloud_url,
            open_release_notes,
            vibe_search,
            following,
            refresh_following,
            post_comment,
            repost,
            like_playlist,
            set_followed,
            create_playlist,
            add_to_playlist,
            add_tracks_to_playlist,
            remove_from_playlist,
            move_in_playlist,
            rename_playlist,
            delete_playlist,
            transport,
            play_tracks,
            enqueue,
            enqueue_tracks,
            set_liked,
            settings,
            toggle_quick_access,
            save_background,
            save_font,
            save_skin,
            skin_images,
            set_setting,
            eq_preset,
            audio_cache,
            storage_report,
            component_status,
            prepare_components,
            preserve_components,
            clear_artwork_cache,
            clear_clap_preparation,
            import_status,
            check_yandex_token,
            start_yandex_import,
            spotify::spotify_import_status,
            spotify::preview_spotify_import,
            spotify::start_spotify_import,
            spotify::cancel_spotify_import,
            spotify::resume_import,
            spotify::discard_import,
            spotify::open_spotify_export,
            upload_track,
            edit_track,
            delete_track,
            save_storefront,
            connect_account,
            sign_in,
            sign_out,
            approval_server_url,
            connect_server,
            approval_users,
            server_session,
            pair_mobile,
            approval_set_user,
            approval_settings,
            approval_media,
            server_operations, server_incident,
            update_events::subscribe_updates
        ])
        .run(tauri::generate_context!())
        .expect("Veirune failed to start");
}

fn show_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg(test)]
#[tokio::test]
#[ignore = "requires a saved SoundCloud account and network access"]
async fn live_account_library_smoke() {
    let credentials = auth::AppCredentials::discover().expect("SoundCloud credentials are missing");
    let client = Arc::new(api::ApiClient::new(Some(credentials.client_id), false));
    let session = auth::start(client.clone()).await.expect("saved session could not be resumed");
    assert!(session.signed_in().await, "saved session is not signed in");

    let profile = api::endpoints::me(&client).await.expect("/me failed");
    assert!(profile.id > 0);
    let playlists = api::endpoints::my_playlists(&client).await.next_page().await.expect("/me/playlists failed");
    let public_playlists = api::endpoints::user_playlists(&client, profile.id).await.next_page().await.expect("/users/:id/playlists failed");
    let saved = api::endpoints::my_liked_playlists(&client).await.next_page().await.expect("/me/likes/playlists failed");
    let likes = api::endpoints::my_liked_tracks(&client).await.next_page().await.expect("/me/likes/tracks failed");
    println!("live library: my_playlists={}, public_profile_playlists={}, saved_playlists={}, liked_tracks={}", playlists.len(), public_playlists.len(), saved.len(), likes.len());
    assert!(!likes.is_empty(), "account likes unexpectedly empty");
}
