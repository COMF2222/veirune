#![allow(dead_code)]

use crate::api::endpoints::StreamUrls;
use crate::api::models::Track;
use crate::audio::cache::AudioCache;
use crate::audio::decode::SegmentDecoder;
use crate::audio::hls::{HlsDownloader, SegmentPrefetch};
use crate::audio::output::AudioOutput;
use crate::config::Settings;
use anyhow::{Context as _, Result};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};
use std::collections::VecDeque;

#[derive(Clone)]
struct QueueSnapshot { queue: Vec<Track>, order: Vec<usize>, current: Option<usize>, position: u64, playing: bool, shuffle: bool, priority: Vec<usize> }
#[derive(Clone, Copy, Default)]
struct AudioPreferences { crossfade_ms: u32, gapless: bool }
struct SleepTimer { deadline: Option<Instant>, track_id: Option<u64> }
struct PreparedTrack { track: Track, decoder: DecoderSlot, samples: Vec<f32>, rate: u32, duration: u64, bitrate: u32, channels: u16 }
struct PendingNext {
    generation: u64, id: u64, fade: u32, prepared: Option<PreparedTrack>,
    attempts: u8, retry_at: Option<Instant>,
}

/// Decode enough before starting the audio device that network and scheduler
/// jitter cannot turn one MP3 packet into a burst followed by silence.
const START_BUFFER_MS: u64 = 3_000;
/// Each background pass produces a useful chunk instead of one ~26 ms frame.
const DECODE_BATCH_MS: u64 = 2_000;
/// Refill before the audio device gets close enough to the end to underrun.
const TARGET_BUFFER_MS: u64 = 8_000;
// Two bounded preparation attempts must fit before the requested fade begins.
const TRANSITION_PREPARE_LEAD_MS: u64 = 45_000;
const TRANSITION_RETRY_DELAY: Duration = Duration::from_secs(1);
/// Coalesce slider events before serializing the queue and settings to disk.
const CONTROL_SAVE_DELAY: Duration = Duration::from_millis(750);

#[derive(Default)]
struct RecoveryBudget { track_id: u64, attempts: u8, last_attempt: Option<Instant> }
impl RecoveryBudget {
    fn take(&mut self, track_id: u64, now: Instant) -> bool {
        if self.track_id != track_id || self.last_attempt.is_some_and(|last| now.duration_since(last) > Duration::from_secs(60)) {
            *self = Self { track_id, ..Self::default() };
        }
        if self.attempts >= 3 { return false; }
        self.attempts += 1;
        self.last_attempt = Some(now);
        true
    }
}

fn retryable_playback_error(error: &anyhow::Error) -> bool {
    use crate::api::error::ApiError;
    if let Some(api) = error.downcast_ref::<ApiError>() {
        return matches!(api, ApiError::PlaybackLinkExpired | ApiError::Network(_) | ApiError::Json(_) | ApiError::Http { status: 500..=599, .. });
    }
    if let Some(http) = error.downcast_ref::<reqwest::Error>() {
        return http.is_timeout() || http.is_connect() || http.is_body()
            || http.status().is_some_and(|status| status.is_server_error());
    }
    let message = format!("{error:#}").to_lowercase();
    !["401", "403", "404", "429", "no stream url", "no full stream", "stream is a preview", "stream has no audio", "blocked", "unavailable", "permission denied"]
        .iter().any(|permanent| message.contains(permanent))
        && ["stream", "decode", "probe", "timeout", "segment", "connect", "audio"]
            .iter().any(|stage| message.contains(stage))
}

fn offline_playlist_url(track_id: u64) -> Option<String> {
    let path = crate::config::app_paths()
        .ok()?
        .root
        .join("offline")
        .join(track_id.to_string())
        .join("playlist.m3u8");
    path.exists()
        .then(|| url::Url::from_file_path(path).ok())
        .flatten()
        .map(String::from)
}

fn initial_decode_target_ms(start_ms: u64) -> u64 {
    start_ms.saturating_add(START_BUFFER_MS)
}

fn visible_position_ms(position_ms: u64, duration_ms: u64) -> u64 {
    if duration_ms == 0 {
        position_ms
    } else {
        position_ms.min(duration_ms)
    }
}

fn playback_error_message(error: &anyhow::Error) -> String {
    if let Some(error) = error.downcast_ref::<crate::api::error::ApiError>() {
        return error.user_message();
    }
    error.to_string()
}

fn should_decode_more(buffered_ms: u64, loading: bool, crossfade_ms: u32) -> bool {
    // Decode through the fade boundary early enough to discover the real EOF,
    // rather than relying on the playlist's rounded duration.
    let target = TARGET_BUFFER_MS.max(u64::from(crossfade_ms) + DECODE_BATCH_MS);
    !loading && buffered_ms < target
}

/// Decoder EOF means no more samples can be appended; it does not mean the
/// samples already queued in the audio device have finished playing.
fn should_advance_after_decode_end(buffered_ms: u64) -> bool {
    buffered_ms == 0
}

fn control_save_due(deadline: Option<Instant>, now: Instant) -> bool {
    deadline.is_some_and(|deadline| now >= deadline)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RepeatMode {
    Off,
    All,
    One,
}

#[derive(Debug, Clone)]
pub struct QueueItem {
    pub track: Track,
}

pub struct PlayerState {
    pub queue: Vec<Track>,
    /// Changes whenever the queue contents or order change. The desktop UI
    /// uses this to avoid copying the whole queue on position-only polls.
    pub queue_revision: u64,
    pub order: Vec<usize>,
    pub priority_next: Vec<usize>,
    pub current: Option<usize>,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub volume: f32,
    pub playback_speed: f32,
    /// Stereo balance, -1 hard left to 1 hard right.
    pub balance: f32,
    pub mono: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub is_playing: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub preview_fallback: bool,
    /// The stream's sample rate in Hz, once one is decoded. Winamp showed it
    /// next to the bitrate, and it is the only one of the two we really know.
    pub sample_rate: u32,
    /// The chosen stream's bitrate in kbps, as the API names it.
    pub bitrate_kbps: u32,
    /// Active A-B repeat belongs to this track, so it cannot leak into the next one.
    pub ab_track_id: Option<u64>,
    pub ab_start_ms: Option<u64>,
    pub ab_end_ms: Option<u64>,
    /// How many channels the source has, before the upmix to stereo. Winamp's
    /// mono/stereo lamp.
    pub channels: u16,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            queue: Vec::new(),
            queue_revision: 0,
            order: Vec::new(),
            priority_next: Vec::new(),
            current: None,
            shuffle: false,
            repeat: RepeatMode::Off,
            volume: 0.8,
            playback_speed: 1.0,
            balance: 0.0,
            mono: false,
            position_ms: 0,
            duration_ms: 0,
            is_playing: false,
            loading: false,
            error: None,
            preview_fallback: false,
            sample_rate: 0,
            bitrate_kbps: 0,
            ab_track_id: None,
            ab_start_ms: None,
            ab_end_ms: None,
            channels: 0,
        }
    }
}

enum DecodeAction {
    None,
    Samples(Vec<f32>, u32),
    FetchMore,
    TrackEnd,
    Fatal(String),
}

/// Player engine: fetch HLS streams, decode in background, drive AudioOutput.
pub struct Player {
    audio_preferences: Mutex<AudioPreferences>,
    sleep_timer: Mutex<Option<SleepTimer>>,
    queue_history: Mutex<VecDeque<QueueSnapshot>>,
    next_track: Mutex<Option<PendingNext>>,
    recovery: Mutex<RecoveryBudget>,
    pub state: Mutex<PlayerState>,
    pub output: Arc<AudioOutput>,
    client: Arc<crate::api::ApiClient>,
    rt: tokio::runtime::Handle,
    hls: Arc<HlsDownloader>,
    http: reqwest::Client,
    settings_path: std::path::PathBuf,
    decoder: Mutex<DecoderSlot>,
    /// Monotonic id of the track load that is allowed to touch the decoder and
    /// output. A slower previous request must never start after a newer click.
    load_generation: AtomicU64,
    stop: parking_lot::Mutex<bool>,
    self_ref: parking_lot::Mutex<Weak<Player>>,
    pending_control_save: parking_lot::Mutex<Option<Instant>>,
    /// When set, playback synthesizes local PCM instead of fetching streams.
    pub demo: parking_lot::Mutex<bool>,
    /// Keep playing similar tracks when the queue runs out.
    pub autoplay: parking_lot::Mutex<bool>,
}

struct DecoderSlot {
    inner: SegmentDecoder,
    urn: String,
    segments_fetched: usize,
    playlist: Option<crate::audio::hls::MediaPlaylist>,
    prefetch: Option<SegmentPrefetch>,
    stream_url: Option<String>,
    is_preview: bool,
    /// The decoder has no more data and nothing has arrived since.
    ///
    /// Set when a pull comes back empty or errors, cleared by [`Self::feed`] —
    /// which is the only thing that can make it untrue. Clearing it only where
    /// a pull *succeeded* is what silenced playback a second after launch: the
    /// loop pulls before any track is loaded, `SegmentDecoder` rightly errors
    /// with "no data to decode", and the flag then blocked every later pull —
    /// including the ones after a track had actually arrived.
    exhausted: bool,
    /// A decoder failure is not a clean EOF and must never auto-advance.
    failed: bool,
}

impl DecoderSlot {
    fn new() -> Self {
        Self {
            inner: SegmentDecoder::new(None),
            urn: String::new(),
            segments_fetched: 0,
            playlist: None,
            prefetch: None,
            stream_url: None,
            is_preview: false,
            // Nothing to decode yet: a pull before the first append is not an
            // error worth logging, it is just early.
            exhausted: true,
            failed: false,
        }
    }

    /// Hand the decoder bytes, and note that there is something to pull again.
    fn feed(&mut self, bytes: &[u8]) {
        self.inner.append(bytes);
        self.exhausted = false;
        self.failed = false;
    }

    /// Point the slot at a new stream. Also resets `exhausted`, because the old
    /// decoder's emptiness says nothing about the new one.
    fn reset_decoder(&mut self, mime: Option<String>) {
        self.prefetch.take();
        self.inner = SegmentDecoder::new(mime);
        self.segments_fetched = 0;
        self.exhausted = true;
        self.failed = false;
    }
}

impl Player {
    pub fn new(
        output: Arc<AudioOutput>,
        client: Arc<crate::api::ApiClient>,
        cache: Arc<AudioCache>,
        settings_path: std::path::PathBuf,
        rt: tokio::runtime::Handle,
    ) -> Self {
        Self {
            audio_preferences: Mutex::new(AudioPreferences { crossfade_ms: 0, gapless: true }),
            sleep_timer: Mutex::new(None), queue_history: Mutex::new(VecDeque::new()), next_track: Mutex::new(None),
            recovery: Mutex::new(RecoveryBudget::default()),
            state: Mutex::new(PlayerState::default()),
            output,
            client,
            rt,
            hls: Arc::new(HlsDownloader::new(reqwest::Client::new(), cache)),
            http: reqwest::Client::new(),
            settings_path,
            decoder: Mutex::new(DecoderSlot::new()),
            load_generation: AtomicU64::new(0),
            stop: parking_lot::Mutex::new(false),
            self_ref: parking_lot::Mutex::new(Weak::new()),
            pending_control_save: parking_lot::Mutex::new(None),
            demo: parking_lot::Mutex::new(false),
            autoplay: parking_lot::Mutex::new(true),
        }
    }

    /// Shared API client (link resolution, stations).
    pub fn api(&self) -> &Arc<crate::api::ApiClient> {
        &self.client
    }

    /// Runtime handle for spawning fetches from any thread (including the
    /// egui UI thread, where bare `tokio::spawn` would panic).
    pub fn runtime(&self) -> &tokio::runtime::Handle {
        &self.rt
    }

    pub fn audio_cache_size(&self) -> u64 {
        self.hls.cache.total_size()
    }

    pub fn clear_audio_cache(&self) -> u64 {
        self.hls.cache.clear()
    }

    pub fn set_audio_cache_limit_mb(&self, limit_mb: u64) {
        self.hls
            .cache
            .set_max_bytes(limit_mb.saturating_mul(1024 * 1024));
    }

    /// Enable synthesized offline playback (demo mode).
    pub fn set_demo(&self, demo: bool) {
        *self.demo.lock() = demo;
    }

    /// Autoplay similar tracks when the queue runs out (demo only).
    pub fn set_autoplay(&self, autoplay: bool) {
        *self.autoplay.lock() = autoplay;
    }

    pub fn set_audio_preferences(&self, normalization: bool, crossfade_ms: u32, gapless: bool) {
        self.output.set_normalization(normalization);
        *self.audio_preferences.lock() = AudioPreferences { crossfade_ms: crossfade_ms.min(8000), gapless };
    }

    pub fn set_sleep_timer(&self, seconds: Option<u64>, after_track: bool) -> Result<()> {
        let track_id = if after_track { Some(self.current_track().context("Start a track before setting this timer")?.id) } else { None };
        if let Some(seconds) = seconds { anyhow::ensure!((1..=7200).contains(&seconds), "Timer must be within 120 minutes"); }
        *self.sleep_timer.lock() = if seconds.is_some() || after_track { Some(SleepTimer { deadline: seconds.map(|seconds| Instant::now() + Duration::from_secs(seconds)), track_id }) } else { None };
        self.output.set_sleep_gain(1.0);
        Ok(())
    }

    pub fn sleep_view(&self) -> (Option<u64>, bool) {
        let timer = self.sleep_timer.lock();
        (timer.as_ref().and_then(|timer| timer.deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()).as_millis() as u64)), timer.as_ref().is_some_and(|timer| timer.track_id.is_some()))
    }

    fn update_sleep_timer(&self) {
        let (remaining, after_track) = self.sleep_view();
        if remaining == Some(0) {
            self.pause(); *self.sleep_timer.lock() = None; self.output.set_sleep_gain(1.0);
        } else {
            let remaining = remaining.or_else(|| after_track.then(|| self.state.lock().duration_ms.saturating_sub(self.output.position_ms())));
            self.output.set_sleep_gain(remaining.map_or(1.0, |ms| (ms as f32 / 10000.0).min(1.0)));
        }
    }

    fn finish_sleep_track(&self) -> bool {
        let current = self.current_track().map(|track| track.id);
        let finish = self.sleep_timer.lock().as_ref().is_some_and(|timer| timer.track_id.is_some() && timer.track_id == current);
        if finish { self.pause(); *self.sleep_timer.lock() = None; self.output.set_sleep_gain(1.0); }
        finish
    }

    pub fn checkpoint_queue(&self) {
        let state = self.state.lock();
        let snapshot = QueueSnapshot { queue: state.queue.clone(), order: state.order.clone(), current: state.current, position: state.position_ms, playing: state.is_playing, shuffle: state.shuffle, priority: state.priority_next.clone() };
        drop(state);
        let mut history = self.queue_history.lock(); history.push_back(snapshot);
        while history.len() > 5 || (history.len() > 1 && history.iter().map(|item| item.queue.len()).sum::<usize>() > 10000) { history.pop_front(); }
    }

    pub fn can_undo_queue(&self) -> bool { !self.queue_history.lock().is_empty() }

    pub fn undo_queue(&self) -> bool {
        let Some(snapshot) = self.queue_history.lock().pop_back() else { return false; };
        let mut state = self.state.lock();
        let running_id = state.current.and_then(|index| state.queue.get(index)).map(|track| track.id);
        let preserved = running_id.and_then(|id| snapshot.queue.iter().position(|track| track.id == id));
        state.queue = snapshot.queue; state.order = snapshot.order; state.shuffle = snapshot.shuffle; state.priority_next = snapshot.priority;
        state.queue_revision = state.queue_revision.wrapping_add(1);
        state.current = preserved.or(snapshot.current);
        if preserved.is_some() { drop(state); self.save_session(); return true; }
        state.position_ms = snapshot.position; state.is_playing = snapshot.playing;
        let track = state.current.and_then(|index| state.queue.get(index)).cloned();
        drop(state);
        if let Some(track) = track { self.load_track(track,snapshot.position); if !snapshot.playing { self.pause(); } }
        else {
            self.stop();
            let mut state = self.state.lock();
            state.position_ms = 0; state.duration_ms = 0; state.loading = false;
            state.error = None; state.preview_fallback = false;
        }
        self.save_session(); true
    }

    fn transition_candidate(&self) -> Option<(u64, usize, Track, u64, u32)> {
        // A UI/sync update must not wait behind the playback-state lock.
        let preferences = *self.audio_preferences.lock();
        if !preferences.gapless && preferences.crossfade_ms == 0 { return None; }
        let state = self.state.lock();
        if !state.is_playing || state.loading || state.preview_fallback || state.repeat == RepeatMode::One || state.ab_start_ms.is_some() || self.sleep_view().1 { return None; }
        let current = state.current?;
        let at = state.order.iter().position(|&index| index == current)?;
        let next = state.order.get(at + 1).copied().or_else(|| (state.repeat == RepeatMode::All).then(|| state.order.first().copied()).flatten())?;
        if next == current { return None; }
        let remaining = state.duration_ms.saturating_sub(self.output.position_ms());
        let fade = preferences.crossfade_ms.min((state.duration_ms / 2).min(8000) as u32);
        let generation = self.load_generation.load(Ordering::Acquire);
        let track = state.queue.get(next)?.clone();
        drop(state);
        // The decoder lock comes before the player-state lock elsewhere. Do
        // not hold state while checking EOF. Generation is rechecked at commit.
        let decoded_end = {
            let decoder = self.decoder.lock();
            decoder.exhausted && !decoder.failed
        };
        let remaining = if decoded_end && !*self.demo.lock() { self.output.buffered_ms() } else { remaining };
        Some((generation, next, track, remaining, fade))
    }

    async fn prepare_next(&self, track: Track, fade: u32) -> Result<PreparedTrack> {
        if *self.demo.lock() && offline_playlist_url(track.id).is_none() {
            let (samples,rate) = crate::demo::pcm_window(&track,0);
            return Ok(PreparedTrack { duration: track.effective_duration_ms(), track, decoder: DecoderSlot::new(), samples, rate, bitrate: 0, channels: 2 });
        }
        let urn = track.urn();
        let (url,bitrate) = if let Some(url) = offline_playlist_url(track.id) { (url,160) }
            else { let streams = self.client.playback_streams_for_track(&track).await?; let (url,rate) = streams.best_full_with_bitrate().context("No full stream for transition")?; (url.to_owned(),rate) };
        self.hls.set_oauth(self.client.oauth_token());
        let playlist = self.hls.playlist(&url).await?;
        let duration = playlist.duration_ms().context("Empty transition stream")?;
        anyhow::ensure!(duration.saturating_add(10000) >= track.effective_duration_ms(), "Transition stream is a preview");
        let mut decoder = DecoderSlot::new(); decoder.urn = urn.clone(); decoder.stream_url = Some(url);
        decoder.reset_decoder(Some(if playlist.init_uri.is_some() { "audio/mp4".into() } else { "audio/mpeg".into() }));
        if let Some(init) = &playlist.init_uri {
            // Every independently decoded fMP4 fragment needs the same init
            // header, including fragments fetched after the fade starts.
            decoder.inner.set_init_segment(self.hls.init_segment(&urn, init).await?);
        }
        let mut samples = Vec::new();
        let target = u64::from(fade).saturating_add(START_BUFFER_MS).max(START_BUFFER_MS);
        'segments: for url in &playlist.segments {
            decoder.feed(&self.hls.segment(&urn,url).await?); decoder.segments_fetched += 1;
            loop {
                if !decoder.inner.next_packet(&mut samples)? { break; }
                if samples.len() as u64 / 2 * 1000 / u64::from(decoder.inner.rate().max(1)) >= target { break 'segments; }
            }
        }
        anyhow::ensure!(!samples.is_empty(), "Transition stream has no audio");
        let rate = decoder.inner.rate(); let channels = decoder.inner.channels();
        decoder.prefetch = Some(self.hls.prefetch(urn, playlist.segments[decoder.segments_fetched..].to_vec(), &self.rt));
        decoder.playlist = Some(playlist);
        Ok(PreparedTrack { track, decoder, samples, rate, duration, bitrate, channels })
    }

    fn prepare_transition(&self) {
        let Some((generation,_,track,remaining,fade)) = self.transition_candidate() else { self.next_track.lock().take(); return; };
        if remaining > u64::from(fade) + TRANSITION_PREPARE_LEAD_MS { return; }
        {
            let mut pending = self.next_track.lock();
            if let Some(pending) = pending.as_mut().filter(|pending| pending.generation == generation && pending.id == track.id && pending.fade == fade) {
                if pending.prepared.is_some() || pending.attempts >= 2 || pending.retry_at.is_none_or(|at| Instant::now() < at) { return; }
                pending.attempts += 1;
                pending.retry_at = None;
            } else {
                *pending = Some(PendingNext { generation, id: track.id, fade, prepared: None, attempts: 1, retry_at: None });
            }
        }
        let Some(me) = self.self_ref.lock().upgrade() else { return; };
        self.rt.spawn(async move {
            let id = track.id;
            let result = tokio::time::timeout(Duration::from_secs(20),me.prepare_next(track,fade)).await;
            let mut pending = me.next_track.lock();
            if let Some(pending) = pending.as_mut().filter(|pending| pending.id == id && pending.generation == generation && pending.fade == fade && me.load_is_current(generation)) {
                match result {
                    Ok(Ok(prepared)) => pending.prepared = Some(prepared),
                    failure => {
                        let retryable = match failure {
                            Ok(Err(error)) => retryable_playback_error(&error),
                            Err(_) => true,
                            Ok(Ok(_)) => false,
                        };
                        if retryable && pending.attempts < 2 {
                            pending.retry_at = Some(Instant::now() + TRANSITION_RETRY_DELAY);
                        }
                        log::debug!("Next-track preparation failed; scheduled retry: {}", pending.retry_at.is_some());
                    }
                }
            }
        });
    }

    fn transition_ready(&self, at_end: bool) -> bool {
        let Some((generation,index,track,remaining,fade)) = self.transition_candidate() else { return false; };
        if !at_end && (fade == 0 || remaining > u64::from(fade) || self.output.buffered_ms().saturating_add(100) < remaining) { return false; }
        let prepared = {
            let mut pending = self.next_track.lock();
            let Some(pending) = pending.as_mut().filter(|pending| pending.id == track.id && pending.generation == generation && pending.fade == fade) else { return false; };
            let Some(prepared) = pending.prepared.take() else { return false; }; prepared
        };
        let mut decoder = self.decoder.lock();
        let mut state = self.state.lock();
        // Manual navigation, pause, shuffle and queue edits always win.
        let ordered_next = state.current.and_then(|current| state.order.iter().position(|&row| row == current)).and_then(|at| state.order.get(at+1).copied().or_else(|| (state.repeat == RepeatMode::All).then(|| state.order.first().copied()).flatten()));
        if !self.load_is_current(generation) || !state.is_playing || state.loading || ordered_next != Some(index)
            || state.repeat == RepeatMode::One || state.ab_start_ms.is_some() || self.sleep_view().1
            || state.queue.get(index).is_none_or(|row| row.id != prepared.track.id) { return false; }
        self.load_generation.fetch_add(1,Ordering::AcqRel);
        *decoder = prepared.decoder;
        state.priority_next.retain(|&row| row != index);
        state.current = Some(index); state.position_ms = 0; state.duration_ms = prepared.duration;
        state.sample_rate = prepared.rate; state.bitrate_kbps = prepared.bitrate; state.channels = prepared.channels;
        state.error = None; state.preview_fallback = false;
        if at_end { self.output.start_track(prepared.samples,prepared.rate,true,0); }
        else { self.output.start_crossfade(prepared.samples,prepared.rate,remaining.min(u64::from(fade))); }
        drop(state); drop(decoder); self.save_session(); true
    }

    /// Register a shared self-reference so spawned threads can drive playback.
    pub fn attach(self: &Arc<Self>) {
        *self.self_ref.lock() = Arc::downgrade(self);
    }

    pub fn output_handle(&self) -> &Arc<AudioOutput> {
        &self.output
    }

    /// Apply persisted controls without treating boot as a new user action.
    pub fn restore_controls(&self, volume: f32, balance: f32, mono: bool) {
        let volume = volume.clamp(0.0, 1.0);
        let balance = balance.clamp(-1.0, 1.0);
        self.output.set_volume(volume);
        self.output.set_balance(balance);
        self.output.set_mono(mono);
        let mut state = self.state.lock();
        state.volume = volume;
        state.balance = balance;
        state.mono = mono;
    }

    /// Make process startup silent without rewriting the saved session.
    pub fn start_paused(&self) {
        self.state.lock().is_playing = false;
        self.output.set_playing(false);
    }

    // ===== Queue management =====

    pub fn play_queue(&self, tracks: Vec<Track>, start: usize, shuffle: bool) {
        let mut st = self.state.lock();
        st.queue = tracks;
        st.priority_next.clear();
        st.queue_revision = st.queue_revision.wrapping_add(1);
        st.shuffle = shuffle;
        st.order = build_order(st.queue.len(), shuffle);
        if shuffle {
            if let Some(position) = st.order.iter().position(|&index| index == start) {
                st.order.swap(0, position);
            }
        }
        st.current = Some(start);
        let track = st.queue[start].clone();
        drop(st);
        self.load_track(track, 0);
    }

    pub fn enqueue(&self, tracks: Vec<Track>, play_next: bool) {
        let mut st = self.state.lock();
        if tracks.is_empty() {
            return;
        }
        if st.queue.is_empty() {
            drop(st);
            self.play_queue(tracks, 0, false);
            return;
        }
        let old_len = st.queue.len();
        let added = tracks.len();
        if play_next {
            let cur = st.current.unwrap_or(0);
            let split = (cur + 1).min(st.queue.len());
            for index in &mut st.priority_next { if *index >= split { *index += added; } }
            st.priority_next.splice(0..0, split..split + added);
            let mut tail = st.queue.split_off(split);
            st.queue.extend(tracks);
            st.queue.append(&mut tail);
            if st.shuffle {
                for index in &mut st.order {
                    if *index >= split {
                        *index += added;
                    }
                }
                let insert_at = st.current.and_then(|current| st.order.iter().position(|&index| index == current))
                    .map_or(st.order.len(), |position| position + 1);
                st.order.splice(insert_at..insert_at, split..split + added);
            }
        } else {
            st.queue.extend(tracks);
            if st.shuffle {
                let mut incoming: Vec<usize> = (old_len..old_len + added).collect();
                use rand::seq::SliceRandom;
                incoming.shuffle(&mut rand::rng());
                st.order.extend(incoming);
            }
        }
        st.queue_revision = st.queue_revision.wrapping_add(1);
        if !st.shuffle { st.order = build_order(st.queue.len(), false); }
        protect_manual_next(&mut st);
        drop(st);
        self.save_session();
    }

    /// Keep the current track, upcoming tracks and a short back history.
    /// An endless station must not grow the in-memory queue without bound.
    pub fn trim_played_history(&self, max_rows: usize, keep_previous: usize) -> usize {
        let mut state = self.state.lock();
        if state.queue.len() <= max_rows { return 0; }
        let Some(current) = state.current else { return 0 };
        let Some(position) = state.order.iter().position(|&index| index == current) else { return 0 };
        let remove: std::collections::HashSet<_> = state.order[..position.saturating_sub(keep_previous)]
            .iter().copied().collect();
        if remove.is_empty() { return 0; }
        let old_len = state.queue.len();
        let mut remap = vec![None; old_len];
        let mut kept = Vec::with_capacity(old_len - remove.len());
        for (index, track) in std::mem::take(&mut state.queue).into_iter().enumerate() {
            if !remove.contains(&index) {
                remap[index] = Some(kept.len());
                kept.push(track);
            }
        }
        state.current = remap[current];
        state.priority_next = state.priority_next.iter().filter_map(|&index| remap[index]).collect();
        state.order = state.order.iter().filter_map(|&index| remap[index]).collect();
        state.queue = kept;
        state.queue_revision = state.queue_revision.wrapping_add(1);
        drop(state);
        self.save_session();
        remove.len()
    }

    pub fn current_track(&self) -> Option<Track> {
        let st = self.state.lock();
        st.current.and_then(|i| st.queue.get(i).cloned())
    }

    pub fn toggle_shuffle(&self) {
        let mut st = self.state.lock();
        st.shuffle = !st.shuffle;
        if st.shuffle {
            let upcoming_start = st.current.and_then(|current| st.order.iter().position(|&index| index == current))
                .map_or(0, |position| position + 1);
            use rand::seq::SliceRandom;
            st.order[upcoming_start..].shuffle(&mut rand::rng());
        } else {
            st.order = build_order(st.queue.len(), false);
        }
        protect_manual_next(&mut st);
        st.queue_revision = st.queue_revision.wrapping_add(1);
        drop(st); self.save_session();
    }

    /// Set shuffle directly (CLI/MPRIS); no-op when unchanged.
    pub fn set_shuffle(&self, enabled: bool) {
        if self.state.lock().shuffle == enabled {
            return;
        }
        self.toggle_shuffle();
    }

    pub fn cycle_repeat(&self) {
        let mut st = self.state.lock();
        st.repeat = match st.repeat {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        };
    }

    /// First press marks A, second marks B, third clears the loop.
    pub fn cycle_ab_loop(&self) {
        let mut st = self.state.lock();
        let Some(track_id) = st.current.and_then(|index| st.queue.get(index)).map(|track| track.id) else {
            return;
        };
        if st.ab_track_id == Some(track_id) && st.ab_end_ms.is_some() {
            st.ab_track_id = None;
            st.ab_start_ms = None;
            st.ab_end_ms = None;
        } else if st.ab_track_id != Some(track_id) {
            st.ab_track_id = Some(track_id);
            st.ab_start_ms = Some(st.position_ms);
            st.ab_end_ms = None;
        } else if let Some(start) = st.ab_start_ms {
            if st.position_ms >= start.saturating_add(500) {
                st.ab_end_ms = Some(st.position_ms);
            } else {
                st.ab_start_ms = Some(st.position_ms);
            }
        } else {
            st.ab_start_ms = Some(st.position_ms);
        }
    }

    pub fn clear_ab_loop(&self) {
        let mut st = self.state.lock();
        st.ab_track_id = None;
        st.ab_start_ms = None;
        st.ab_end_ms = None;
    }

    /// Set repeat directly (CLI/MPRIS).
    pub fn set_repeat(&self, mode: RepeatMode) {
        self.state.lock().repeat = mode;
    }

    // ===== Transport =====

    /// Resume loading when the output holds no audio for the current track
    /// (fresh boot with a restored queue, or a track that never buffered).
    /// Returns the track+offset to load, if a load is needed instead of a
    /// plain pause/play flip.
    fn resume_load(&self) -> Option<(Track, u64)> {
        let st = self.state.lock();
        let idx = st.current?;
        if st.loading || self.output.buffered_ms() > 0 {
            return None;
        }
        st.queue.get(idx).cloned().map(|t| {
            let position = visible_position_ms(st.position_ms, t.effective_duration_ms());
            // A session saved exactly at EOF should replay, not reopen a
            // zero-length tail and appear stuck.
            let position = if position < t.effective_duration_ms() {
                position
            } else {
                0
            };
            (t, position)
        })
    }

    pub fn play_pause(&self) {
        if let Some((track, pos)) = self.resume_load() {
            self.load_track(track, pos);
            return;
        }
        let playing = {
            let mut st = self.state.lock();
            st.is_playing = !st.is_playing;
            st.is_playing
        };
        self.output.set_playing(playing);
        self.save_session();
    }

    pub fn play(&self) {
        if let Some((track, pos)) = self.resume_load() {
            self.load_track(track, pos);
            return;
        }
        let mut st = self.state.lock();
        st.is_playing = true;
        drop(st);
        self.output.set_playing(true);
        self.save_session();
    }

    pub fn pause(&self) {
        let mut st = self.state.lock();
        st.is_playing = false;
        drop(st);
        self.output.set_playing(false);
        self.save_session();
    }

    pub fn stop(&self) {
        *self.sleep_timer.lock() = None;
        self.output.set_sleep_gain(1.0);
        self.load_generation.fetch_add(1, Ordering::AcqRel);
        *self.decoder.lock() = DecoderSlot::new();
        let mut st = self.state.lock();
        st.is_playing = false;
        st.current = None;
        // Nothing is loaded, so nothing has a rate or a bitrate: the mini
        // player's readouts go blank rather than keeping the last track's.
        st.sample_rate = 0;
        st.bitrate_kbps = 0;
        st.channels = 0;
        drop(st);
        self.output.set_playing(false);
        self.save_session();
    }

    pub fn next(&self) -> bool {
        {
            let mut st = self.state.lock();
            let Some(cur) = st.current else { return false };
            if st.order.is_empty() {
                return false;
            }
            let pos_in_order = st.order.iter().position(|&x| x == cur);
            let next = match (pos_in_order, st.repeat) {
                (Some(p), RepeatMode::All) if p + 1 < st.order.len() => st.order[p + 1],
                (Some(_), RepeatMode::All) => st.order[0],
                (Some(p), _) if p + 1 < st.order.len() => st.order[p + 1],
                _ => return false,
            };
            st.priority_next.retain(|&index| index != next);
            st.current = Some(next);
            let track = st.queue.get(next).cloned();
            drop(st);
            if let Some(t) = track {
                self.load_track(t, 0);
            }
            true
        }
    }

    pub fn prev(&self) -> bool {
        let mut st = self.state.lock();
        // If more than 3s into the track, restart it instead.
        if st.position_ms > 3000
            && let Some(cur) = st.current
        {
            let track = st.queue.get(cur).cloned();
            drop(st);
            if let Some(t) = track {
                self.load_track(t, 0);
            }
            return true;
        }
        let Some(cur) = st.current else { return false };
        let pos_in_order = st.order.iter().position(|&x| x == cur);
        let prev = match pos_in_order {
            Some(0) | None => return false,
            Some(p) => st.order[p - 1],
        };
        st.current = Some(prev);
        let track = st.queue.get(prev).cloned();
        drop(st);
        if let Some(t) = track {
            self.load_track(t, 0);
        }
        true
    }

    // ===== Queue panel support =====

    /// Upcoming queue indices in effective play order (after current).
    pub fn upcoming(&self) -> Vec<usize> {
        let st = self.state.lock();
        upcoming_order(&st.order, st.current)
    }

    /// Current + upcoming tracks in one lock (for the queue panel).
    pub fn snapshot(&self) -> (Option<Track>, Vec<(usize, Track)>) {
        let st = self.state.lock();
        let cur = st.current.and_then(|i| st.queue.get(i).cloned());
        let up = upcoming_order(&st.order, st.current)
            .into_iter()
            .filter_map(|i| st.queue.get(i).cloned().map(|t| (i, t)))
            .collect();
        (cur, up)
    }

    /// Skip to a queue index, like pressing Next down to it.
    pub fn skip_to(&self, idx: usize) -> bool {
        let track = {
            let mut st = self.state.lock();
            if idx >= st.queue.len() {
                return false;
            }
            st.priority_next.retain(|&index| index != idx);
            st.current = Some(idx);
            protect_manual_next(&mut st);
            st.queue.get(idx).cloned()
        };
        if let Some(t) = track {
            self.load_track(t, 0);
            true
        } else {
            false
        }
    }

    /// Remove a row from the queue. Removing the current row advances
    /// to the next one (or stops when it was the last).
    pub fn remove_at(&self, idx: usize) -> bool {
        enum Plan {
            Done,
            SkipTo(usize),
            Stop,
        }
        let plan = {
            let mut st = self.state.lock();
            if idx >= st.queue.len() {
                return false;
            }
            if Some(idx) == st.current {
                let next = st
                    .order
                    .iter()
                    .position(|&x| x == idx)
                    .and_then(|p| st.order.get(p + 1).copied())
                    .map(|n| if n > idx { n - 1 } else { n });
                st.priority_next = st.priority_next.iter().filter_map(|&row| if row == idx { None } else { Some(if row > idx { row - 1 } else { row }) }).collect();
                st.queue.remove(idx);
                st.queue_revision = st.queue_revision.wrapping_add(1);
                let (order, _) = drop_bookkeeping(&st.order, None, idx);
                st.order = order;
                st.current = None;
                match next {
                    Some(n) => Plan::SkipTo(n),
                    None => Plan::Stop,
                }
            } else {
                st.priority_next = st.priority_next.iter().filter_map(|&row| if row == idx { None } else { Some(if row > idx { row - 1 } else { row }) }).collect();
                st.queue.remove(idx);
                st.queue_revision = st.queue_revision.wrapping_add(1);
                let (order, current) = drop_bookkeeping(&st.order, st.current, idx);
                st.order = order;
                st.current = current;
                Plan::Done
            }
        };
        match plan {
            Plan::Done => {
                self.save_session();
                true
            }
            Plan::SkipTo(n) => self.skip_to(n),
            Plan::Stop => {
                self.stop();
                true
            }
        }
    }

    /// Remove matching tracks atomically, advancing to the next surviving row
    /// when the current track is removed. Preserve the effective shuffle order.
    pub fn remove_matching(&self, mut reject: impl FnMut(&Track) -> bool) -> usize {
        let (removed, advance, next) = {
            let mut state = self.state.lock();
            let rejected: Vec<_> = state.queue.iter().map(&mut reject).collect();
            let removed = rejected.iter().filter(|&&value| value).count();
            if removed == 0 { return 0; }
            let advance = state.current.is_some_and(|index| rejected[index]);
            let next = state.current.and_then(|current| state.order.iter()
                .position(|&index| index == current))
                .and_then(|position| state.order[position + 1..].iter()
                    .copied().find(|&index| !rejected[index]));
            let mut remap = vec![None; state.queue.len()];
            let mut kept = Vec::with_capacity(state.queue.len() - removed);
            for (index, track) in std::mem::take(&mut state.queue).into_iter().enumerate() {
                if !rejected[index] {
                    remap[index] = Some(kept.len());
                    kept.push(track);
                }
            }
            state.current = state.current.and_then(|index| remap[index]);
            state.priority_next = state.priority_next.iter().filter_map(|&index| remap[index]).collect();
        state.order = state.order.iter().filter_map(|&index| remap[index]).collect();
            state.queue = kept;
            state.queue_revision = state.queue_revision.wrapping_add(1);
            (removed, advance, next.and_then(|index| remap[index]))
        };
        if advance {
            if let Some(next) = next { self.skip_to(next); } else { self.stop(); }
        } else {
            self.save_session();
        }
        removed
    }

    /// Move a queue row while retaining the current track and shuffle order.
    pub fn move_at(&self, from: usize, to: usize) -> bool {
        let mut state = self.state.lock();
        if from >= state.queue.len() || to >= state.queue.len() || from == to {
            return false;
        }
        let track = state.queue.remove(from);
        state.queue.insert(to, track);
        state.queue_revision = state.queue_revision.wrapping_add(1);
        let remap = |index: usize| {
            if index == from {
                to
            } else if from < to && index > from && index <= to {
                index - 1
            } else if to < from && index >= to && index < from {
                index + 1
            } else {
                index
            }
        };
        state.priority_next = state.priority_next.iter().copied().map(remap).filter(|&index| index != to).collect();
        state.current = state.current.map(remap);
        for index in &mut state.order {
            *index = remap(*index);
        }
        drop(state);
        self.save_session();
        true
    }

    /// Drop everything except the current track. Returns rows removed.
    pub fn clear_upcoming(&self) -> usize {
        let mut st = self.state.lock();
        let Some(cur) = st.current else { return 0 };
        let Some(track) = st.queue.get(cur).cloned() else {
            return 0;
        };
        let removed = st.queue.len().saturating_sub(1);
        st.queue = vec![track];
        st.priority_next.clear();
        st.queue_revision = st.queue_revision.wrapping_add(1);
        st.current = Some(0);
        st.order = vec![0];
        drop(st);
        self.save_session();
        removed
    }

    pub fn clear_played(&self) -> usize {
        let removed = clear_played_state(&mut self.state.lock());
        if removed > 0 { self.save_session(); }
        removed
    }

    pub fn retry_current(&self) {
        if let Some(track) = self.current_track() {
            *self.recovery.lock() = RecoveryBudget::default();
            let position = self.state.lock().position_ms;
            self.load_track(track, position);
        }
    }

    fn recover_stream(&self, generation: u64, error: &anyhow::Error) -> bool {
        if !self.load_is_current(generation) || !retryable_playback_error(error) { return false; }
        let (track, position) = {
            let state = self.state.lock();
            if !state.is_playing { return false; }
            let Some(track) = state.current.and_then(|index| state.queue.get(index)).cloned() else { return false; };
            (track, visible_position_ms(self.output.position_ms(), state.duration_ms))
        };
        if !self.recovery.lock().take(track.id, Instant::now()) { return false; }
        // A decoder failure can be caused by a bad cached response that passed
        // the superficial transport check. Re-fetch that recording on recovery.
        let message = format!("{error:#}").to_lowercase();
        if message.contains("probe") || message.contains("decode") {
            self.hls.cache.evict_track(&track.urn());
        }
        self.load_track_guarded(track, position, Some(generation))
    }

    fn fail_current_load(&self, generation: u64, message: String) {
        let mut state = self.state.lock();
        if !self.load_is_current(generation) { return; }
        state.is_playing = false;
        state.loading = false;
        state.error = Some(message);
        self.output.set_playing(false);
        drop(state);
        self.save_session();
    }

    /// Stop playback and remove every queued track, including the persisted
    /// session. Keeping this mutation inside `Player` prevents the UI from
    /// clearing memory after `stop()` has already saved the old queue.
    pub fn clear_queue(&self) {
        self.load_generation.fetch_add(1, Ordering::AcqRel);
        {
            let mut state = self.state.lock();
            state.queue.clear();
            state.priority_next.clear();
            state.queue_revision = state.queue_revision.wrapping_add(1);
            state.order.clear();
            state.current = None;
            state.position_ms = 0;
            state.duration_ms = 0;
            state.is_playing = false;
            state.loading = false;
            state.sample_rate = 0;
            state.bitrate_kbps = 0;
            state.channels = 0;
        }
        self.output.set_playing(false);
        self.save_session();
    }

    /// Append a track to the end of the queue, preserving play order.
    pub fn push_back(&self, track: Track) {
        let mut st = self.state.lock();
        st.queue.push(track);
        st.queue_revision = st.queue_revision.wrapping_add(1);
        let last = st.queue.len() - 1;
        st.order.push(last);
        drop(st);
        self.save_session();
    }

    /// Autoplay: when the queue runs out, queue a related track.
    /// Demo-only; the real build resolves related tracks via the API.
    fn maybe_autoplay(&self) {
        if !*self.autoplay.lock() || !*self.demo.lock() {
            return;
        }
        let current_id = self.current_track().map(|t| t.id).unwrap_or(0);
        let all = crate::demo::demo_tracks();
        if all.is_empty() {
            return;
        }
        let rel = all[(current_id as usize + 1) % all.len()].clone();
        log::info!("autoplay: queueing {}", rel.title);
        self.push_back(rel);
        self.next();
    }

    /// Seek within the track.
    ///
    /// The output only holds a window of decoded audio (see
    /// `AudioOutput::append_samples`), so a seek outside that window reloads
    /// the stream from the new position instead of clamping to the buffer —
    /// which is what made dragging the bar backwards jump to wherever the
    /// buffer happened to start.
    pub fn seek_ms(&self, ms: u64) {
        let loading = {
            let mut st = self.state.lock();
            if st.preview_fallback && ms >= st.duration_ms {
                st.error = Some("SoundCloud provides only a short preview for this track".into());
                return;
            }
            st.loading
        };
        if !loading && self.output.can_seek_to(ms) {
            self.output.seek_ms(ms);
            let mut st = self.state.lock();
            st.position_ms = ms;
            drop(st);
            self.save_session();
            return;
        }
        let Some(track) = self.current_track() else {
            return;
        };
        self.load_track(track, ms);
    }

    pub fn set_volume(&self, v: f32) {
        let v = v.clamp(0.0, 1.0);
        self.output.set_volume(v);
        let mut st = self.state.lock();
        st.volume = v;
        drop(st);
        self.schedule_control_save();
    }

    pub fn set_playback_speed(&self, speed: f32) {
        let speed = speed.clamp(0.5, 2.0);
        self.output.set_playback_speed(speed);
        self.state.lock().playback_speed = speed;
    }

    /// Stereo balance, -1 hard left to 1 hard right. The mini player's second
    /// slider; the app's own interface has no control for it.
    pub fn set_balance(&self, balance: f32) {
        let balance = balance.clamp(-1.0, 1.0);
        self.output.set_balance(balance);
        let mut st = self.state.lock();
        st.balance = balance;
        drop(st);
        self.schedule_control_save();
    }

    pub fn set_mono(&self, mono: bool) {
        self.output.set_mono(mono);
        self.state.lock().mono = mono;
        self.schedule_control_save();
    }

    pub fn set_eq(&self, enabled: bool, gains: [f32; 10]) {
        self.output.set_eq(enabled, gains);
    }

    /// The equaliser's preamp, in dB. Persisted with the rest of the EQ.
    pub fn set_eq_preamp_db(&self, preamp_db: f32) {
        self.output.set_eq_preamp_db(preamp_db);
    }

    // ===== Stream loading =====

    fn load_track(&self, track: Track, start_ms: u64) {
        self.load_track_guarded(track, start_ms, None);
    }

    fn load_track_guarded(&self, track: Track, start_ms: u64, expected: Option<u64>) -> bool {
        let mut decoder = self.decoder.lock();
        let generation = {
            let mut st = self.state.lock();
            if expected.is_some_and(|generation| !self.load_is_current(generation) || !st.is_playing) { return false; }
            let generation = self.load_generation.fetch_add(1, Ordering::AcqRel) + 1;
            // A deadline continues across skips; an "after this track" timer
            // is cancelled when the listener chooses another song.
            let mut timer = self.sleep_timer.lock();
            if timer.as_ref().is_some_and(|timer| timer.track_id.is_some_and(|id| id != track.id)) {
                *timer = None;
                self.output.set_sleep_gain(1.0);
            }
            st.loading = true;
            st.is_playing = true;
            st.error = None;
            st.position_ms = start_ms;
            st.preview_fallback = false;
            st.duration_ms = track.effective_duration_ms();
            self.output.start_track(Vec::new(), self.output.device_sample_rate().max(1), false, start_ms);
            generation
        };
        decoder.prefetch.take();
        drop(decoder);
        // A manual Next/Previous must take effect audibly at once. Leaving the
        // old output running while the new stream resolves made the button
        // feel delayed and could leak a few seconds of the previous song.
        // Clone via the shared self-reference installed in attach().
        let Some(me) = self.self_ref.lock().upgrade() else {
            log::warn!("player not attached; cannot start loader");
            return false;
        };
        // Demo mode: instant local synthesis, no network. Only the stretch
        // being played is synthesized (see `demo::pcm_window`), so a long
        // demo track costs a few MB rather than the whole song at f32.
        if *self.demo.lock() && offline_playlist_url(track.id).is_none() {
            std::thread::spawn(move || {
                let (pcm, rate) = crate::demo::pcm_window(&track, start_ms);
                if !me.load_is_current(generation) {
                    return;
                }
                let mut st = me.state.lock();
                if !me.load_is_current(generation) { return; }
                me.output.start_track(pcm, rate, st.is_playing, start_ms);
                st.loading = false;
                st.sample_rate = rate;
                // Synthesized f32, so nothing was ever compressed. The
                // readout stays blank rather than inventing a bitrate.
                st.bitrate_kbps = 0;
                // `demo::pcm_window` writes interleaved pairs.
                st.channels = 2;
            });
            return true;
        }
        me.rt.clone().spawn(async move {
            let mut attempt = 0;
            loop {
                let result = me.load_track_async(track.clone(), start_ms, generation).await;
                let Err(e) = result else { break; };
                if !me.load_is_current(generation) { break; }
                if attempt < 3 && me.state.lock().is_playing && retryable_playback_error(&e)
                    && me.recovery.lock().take(track.id, Instant::now()) {
                    let message = format!("{e:#}").to_lowercase();
                    if message.contains("probe") || message.contains("decode") { me.hls.cache.evict_track(&track.urn()); }
                    attempt += 1;
                    tokio::time::sleep(Duration::from_secs(attempt * 2)).await;
                    if !me.load_is_current(generation) { break; }
                    if !me.state.lock().is_playing {
                        me.state.lock().loading = false;
                        break;
                    }
                    continue;
                }
                let mut slot = me.decoder.lock();
                if !me.load_is_current(generation) {
                    return;
                }
                slot.prefetch.take();
                slot.failed = true;
                slot.exhausted = true;
                drop(slot);
                let mut st = me.state.lock();
                if !me.load_is_current(generation) { break; }
                st.error = Some(playback_error_message(&e));
                st.loading = false;
                st.is_playing = false;
                break;
            }
        });
        true
    }

    fn load_is_current(&self, generation: u64) -> bool {
        self.load_generation.load(Ordering::Acquire) == generation
    }

    async fn load_track_async(&self, track: Track, start_ms: u64, generation: u64) -> Result<()> {
        let urn = track.urn();
        let offline_url = offline_playlist_url(track.id);
        let streams: Option<StreamUrls> = if offline_url.is_none() {
            Some(
                self.client
                    .playback_streams_for_track(&track)
                    .await
                    .context("fetch streams")?,
            )
        } else {
            None
        };
        if !self.load_is_current(generation) {
            return Ok(());
        }
        self.hls.set_oauth(self.client.oauth_token());

        // The API offers HLS only (MP3 then AAC); `stream_url` is deprecated
        // and preview-only, so a blocked/preview track falls back below.
        let chosen = offline_url.map(|url| (url, 160)).or_else(|| {
            streams
                .as_ref()
                .and_then(StreamUrls::best_full_with_bitrate)
                .map(|(url, kbps)| (url.to_owned(), kbps))
        });

        {
            let mut slot = self.decoder.lock();
            if !self.load_is_current(generation) {
                return Ok(());
            }
            *slot = DecoderSlot::new();
            slot.urn = urn.clone();
        }

        let mut first_samples: Vec<f32>;
        let mut decode_skip_ms = start_ms;
        let mut output_start_ms = start_ms;
        let available_duration_ms: u64;
        let is_preview: bool;
        let source_rate: u32;
        let bitrate_kbps;
        let channels: u16;

        if let Some((url, kbps)) = chosen {
            bitrate_kbps = kbps;
            // Both full-stream fields are HLS, including API URLs ending in
            // /hls. The URL extension cannot distinguish a playlist from MP3.
            {
                let pl = self.hls.playlist(&url).await.context("fetch playlist")?;
                if !self.load_is_current(generation) {
                    return Ok(());
                }
                let duration_ms = pl.duration_ms().context("empty HLS playlist")?;
                available_duration_ms = duration_ms;
                is_preview = duration_ms.saturating_add(10_000) < track.effective_duration_ms();
                output_start_ms = preview_seek_start(start_ms, duration_ms);
                let is_fmp4 = pl.init_uri.is_some();
                let (first_segment, segment_start_ms) = pl.seek_point(output_start_ms);
                decode_skip_ms = output_start_ms.saturating_sub(segment_start_ms);
                {
                    let mut slot = self.decoder.lock();
                    if !self.load_is_current(generation) {
                        return Ok(());
                    }
                    slot.playlist = Some(pl);
                    slot.stream_url = Some(url);
                    // fMP4 segments are unplayable without the init header;
                    // plain TS/MP3 segments stay on the mpeg probe.
                    slot.reset_decoder(Some(if is_fmp4 {
                        "audio/mp4".into()
                    } else {
                        "audio/mpeg".into()
                    }));
                    slot.segments_fetched = first_segment;
                    let urls = slot.playlist.as_ref()
                        .map(|playlist| playlist.segments[first_segment..].to_vec())
                        .unwrap_or_default();
                    slot.prefetch = Some(self.hls.prefetch(urn.clone(), urls, &self.rt));
                }
                // Init header first (cached), then media segments.
                let init_url = {
                    self.decoder
                        .lock()
                        .playlist
                        .as_ref()
                        .and_then(|p| p.init_uri.clone())
                };
                if let Some(init_url) = init_url {
                    let init_bytes = self
                        .hls
                        .init_segment(&urn, &init_url)
                        .await
                        .context("fetch init segment")?;
                    if !self.load_is_current(generation) {
                        return Ok(());
                    }
                    let mut slot = self.decoder.lock();
                    if !self.load_is_current(generation) {
                        return Ok(());
                    }
                    slot.inner.set_init_segment(init_bytes);
                }
                let mut out = Vec::new();
                let target_ms = initial_decode_target_ms(decode_skip_ms);
                loop {
                    // Start with one segment. Fetching three serially before
                    // decoding made manual track changes wait unnecessarily;
                    // subsequent passes fetch only when the initial 3 s target
                    // still is not satisfied.
                    self.fetch_and_append(generation, true).await?;
                    if !self.load_is_current(generation) {
                        return Ok(());
                    }
                    let needs_more = {
                        let mut slot = self.decoder.lock();
                        if !self.load_is_current(generation) {
                            return Ok(());
                        }
                        loop {
                            if !slot.inner.next_packet(&mut out)? {
                                break slot
                                    .playlist
                                    .as_ref()
                                    .is_some_and(|p| slot.segments_fetched < p.segments.len());
                            }
                            let rate = u64::from(slot.inner.rate().max(1));
                            if out.len() as u64 / 2 * 1000 / rate >= target_ms {
                                break false;
                            }
                        }
                    };
                    if !needs_more {
                        break;
                    }
                }
                let (rate, chans) = {
                    let slot = self.decoder.lock();
                    (slot.inner.rate(), slot.inner.channels())
                };
                anyhow::ensure!(!out.is_empty(), "stream contained no decodable audio");
                first_samples = out;
                source_rate = rate;
                channels = chans;
            }
        } else if let Some(preview) = streams
            .as_ref()
            .and_then(|streams| streams.preview_mp3_128.clone())
        {
            // 30-second preview fallback.
            let bytes = self.hls.segment(&urn, &preview).await?;
            if !self.load_is_current(generation) {
                return Ok(());
            }
            let (samples, rate, chans) =
                crate::audio::decode::decode_all(bytes, Some("audio/mpeg"))?;
            first_samples = samples;
            source_rate = rate;
            channels = chans;
            let duration_ms = first_samples.len() as u64 / 2 * 1000 / u64::from(rate.max(1));
            anyhow::ensure!(duration_ms > 0, "preview stream contained no audio");
            available_duration_ms = duration_ms;
            if start_ms >= duration_ms {
                output_start_ms = preview_seek_start(start_ms, duration_ms);
                decode_skip_ms = output_start_ms;
            }
            // The field names its own rate, as the full streams do.
            bitrate_kbps = 128;
            is_preview = true;
            let mut slot = self.decoder.lock();
            slot.is_preview = true;
        } else {
            anyhow::bail!("no stream URL available");
        }

        if decode_skip_ms > 0 {
            let skip_frames = (decode_skip_ms * source_rate as u64 / 1000) as usize * 2;
            first_samples.drain(..skip_frames.min(first_samples.len()));
        }
        anyhow::ensure!(!first_samples.is_empty(), "seek position has no decodable audio");

        if !self.load_is_current(generation) {
            return Ok(());
        }

        {
            let mut st = self.state.lock();
            if !self.load_is_current(generation) { return Ok(()); }
            self.output.start_track(first_samples, source_rate, st.is_playing, output_start_ms);
            st.loading = false;
            st.preview_fallback = is_preview;
            st.duration_ms = available_duration_ms;
            st.position_ms = output_start_ms;
            if start_ms >= available_duration_ms {
                st.error = Some("SoundCloud stream is shorter than the requested position".into());
            }
            st.sample_rate = source_rate;
            st.bitrate_kbps = bitrate_kbps;
            st.channels = channels;
        }
        self.save_session();
        Ok(())
    }

    async fn fetch_and_append(&self, generation: u64, wait: bool) -> Result<()> {
        if !self.load_is_current(generation) {
            return Ok(());
        }
        let queue = {
            let slot = self.decoder.lock();
            let Some(prefetch) = slot.prefetch.as_ref() else {
                return Ok(());
            };
            prefetch.queue.clone()
        };
        let mut receiver = queue.lock().await;
        let fetched = if wait {
            receiver.recv().await
        } else {
            match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => None,
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                    let mut slot = self.decoder.lock();
                    if self.load_is_current(generation) {
                        slot.inner.wait_for_append();
                    }
                    return Ok(());
                }
            }
        };
        drop(receiver);
        if !self.load_is_current(generation) {
            return Ok(());
        }
        let bytes = fetched.context("Audio download ended before the track completed")??;
        let mut slot = self.decoder.lock();
        if self.load_is_current(generation) {
            // Make each ready segment decodable immediately; a slow following
            // segment must not hold back audio that has already arrived.
            slot.feed(&bytes);
            slot.segments_fetched += 1;
        }
        Ok(())
    }

    /// Background decode loop: pull packets, append to output, fetch more
    /// segments as needed, advance the queue on track end (gapless).
    pub async fn run(self: Arc<Self>) {
        let mut last_checkpoint = Instant::now();
        loop {
            if *self.stop.lock() {
                return;
            }
            self.flush_control_save();
            self.update_sleep_timer();
            self.prepare_transition();
            if self.transition_ready(false) { continue; }
            let buffered = self.output.buffered_ms();
            // `load_track_async` owns the decoder while it builds the initial
            // buffer. Pulling here at the same time can steal the first packets
            // and append them to the output of the previous track.
            let loading = self.state.lock().loading;
            let crossfade_ms = self.audio_preferences.lock().crossfade_ms;
            // Demo mode synthesizes its own audio in windows; there is no
            // decoder to pull from.
            if *self.demo.lock() {
                if should_decode_more(buffered, loading, crossfade_ms) {
                    self.top_up_demo();
                }
                self.publish_position();
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
            if should_decode_more(buffered, loading, crossfade_ms) {
                let cycle_generation = self.load_generation.load(Ordering::Acquire);
                let action = {
                    let mut slot = self.decoder.lock();
                    if slot.exhausted {
                        if !slot.failed && should_advance_after_decode_end(buffered) {
                            DecodeAction::TrackEnd
                        } else {
                            DecodeAction::None
                        }
                    } else {
                        let mut out = Vec::new();
                        let mut decoded = false;
                        let pulled = loop {
                            match slot.inner.next_packet(&mut out) {
                                Ok(true) => {
                                    decoded = true;
                                    let rate = u64::from(slot.inner.rate().max(1));
                                    if out.len() as u64 / 2 * 1000 / rate >= DECODE_BATCH_MS {
                                        break Ok(true);
                                    }
                                }
                                Ok(false) if decoded => break Ok(true),
                                other => break other,
                            }
                        };
                        match pulled {
                            Ok(true) => DecodeAction::Samples(out, slot.inner.rate()),
                            Ok(false) => {
                                let remaining = slot
                                    .playlist
                                    .as_ref()
                                    .map(|p| p.segments.len() - slot.segments_fetched)
                                    .unwrap_or(0);
                                if remaining > 0 && !slot.is_preview {
                                    DecodeAction::FetchMore
                                } else {
                                    slot.exhausted = true;
                                    if should_advance_after_decode_end(buffered) {
                                        DecodeAction::TrackEnd
                                    } else {
                                        DecodeAction::None
                                    }
                                }
                            }
                            Err(e) => {
                                log::warn!("decode: {e:#}");
                                let remaining = slot
                                    .playlist
                                    .as_ref()
                                    .map(|p| p.segments.len() - slot.segments_fetched)
                                    .unwrap_or(0);
                                if remaining > 0 && !slot.is_preview {
                                    slot.inner.discard_window();
                                    DecodeAction::FetchMore
                                } else {
                                    slot.exhausted = true;
                                    slot.failed = true;
                                    DecodeAction::Fatal(format!("{e:#}"))
                                }
                            }
                        }
                    }
                };
                if !self.load_is_current(cycle_generation) {
                    continue;
                }
                match action {
                    DecodeAction::None => {}
                    DecodeAction::Samples(out, rate) => {
                        self.output.append_samples(&out, rate);
                    }
                    DecodeAction::FetchMore => {
                        match self.fetch_and_append(cycle_generation, false).await {
                            Ok(()) => {},
                            Err(e) if self.load_is_current(cycle_generation) => {
                                if self.recover_stream(cycle_generation, &e) { continue; }
                                // The download task has already retried transient
                                // failures; preserve the original terminal error.
                                let mut slot = self.decoder.lock();
                                if !self.load_is_current(cycle_generation) { continue; }
                                slot.failed = true;
                                slot.exhausted = true;
                                slot.prefetch.take();
                                drop(slot);
                                self.fail_current_load(cycle_generation, format!("Audio stream interrupted: {e}"));
                            }
                            Err(_) => {}
                        }
                    }
                    DecodeAction::TrackEnd => {
                        if self.finish_sleep_track() { continue; }
                        if self.transition_ready(true) { continue; }
                        let st = self.state.lock();
                        // A restored queue is paused with an empty decoder. It
                        // must not fetch the next track before sign-in resumes.
                        let nextable = st.is_playing && st.current.is_some();
                        let repeat = st.repeat;
                        drop(st);
                        if nextable {
                            match repeat {
                                RepeatMode::One => {
                                    if let Some(t) = self.current_track() {
                                        self.load_track(t, 0);
                                    }
                                }
                                _ => {
                                    if !self.next() {
                                        self.maybe_autoplay();
                                    }
                                }
                            }
                        }
                    }
                    DecodeAction::Fatal(why) => {
                        if self.recover_stream(cycle_generation, &anyhow::anyhow!(why.clone())) { continue; }
                        self.fail_current_load(cycle_generation, format!("Playback stopped: {why}"));
                    }
                }
            }

            // Publish the position at ~10 Hz. `AudioOutput` reports where the
            // *track* is (it counts the frames it dropped), so this follows a
            // backwards seek too — clamping it monotonic used to pin the bar
            // at the furthest point reached.
            self.publish_position();
            if last_checkpoint.elapsed() >= Duration::from_secs(15) {
                last_checkpoint = Instant::now();
                if self.state.lock().is_playing {
                    self.save_session();
                }
            }

            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    fn publish_position(&self) {
        let pos = self.output.position_ms();
        let mut st = self.state.lock();
        // A restored queue has no audio loaded yet. Keep its saved position
        // while paused; the output device still reports zero until Play.
        if !st.loading && st.is_playing {
            st.position_ms = visible_position_ms(pos, st.duration_ms);
        }
        let current_id = st.current.and_then(|index| st.queue.get(index)).map(|track| track.id);
        if st.ab_track_id != current_id {
            st.ab_track_id = None;
            st.ab_start_ms = None;
            st.ab_end_ms = None;
        }
        let restart = if st.is_playing && !st.loading {
            st.ab_end_ms.filter(|end| pos >= *end).and(st.ab_start_ms)
        } else {
            None
        };
        drop(st);
        if let Some(start) = restart {
            self.seek_ms(start);
        }
    }

    /// Demo mode: synthesize the next window, or move on at the track's end.
    fn top_up_demo(&self) {
        let Some(track) = self.current_track() else {
            return;
        };
        let duration = track.effective_duration_ms();
        let from = self.output.buffered_until_ms();
        if from >= duration {
            if self.output.buffered_ms() == 0 && self.finish_sleep_track() { return; }
            if self.output.buffered_ms() == 0 && self.transition_ready(true) { return; }
            // The pad has played out: same end-of-track handling as a stream.
            let (nextable, repeat) = {
                let st = self.state.lock();
                (st.current.is_some() && st.is_playing, st.repeat)
            };
            if !nextable || !should_advance_after_decode_end(self.output.buffered_ms()) {
                return;
            }
            match repeat {
                RepeatMode::One => self.load_track(track, 0),
                _ => {
                    if !self.next() {
                        self.maybe_autoplay();
                    }
                }
            }
            return;
        }
        let (pcm, rate) = crate::demo::pcm_window(&track, from);
        if !pcm.is_empty() {
            self.output.append_samples(&pcm, rate);
        }
    }

    // ===== Session persistence =====

    fn schedule_control_save(&self) {
        *self.pending_control_save.lock() = Some(Instant::now() + CONTROL_SAVE_DELAY);
    }

    fn flush_control_save(&self) {
        let now = Instant::now();
        let due = {
            let mut deadline = self.pending_control_save.lock();
            if control_save_due(*deadline, now) {
                *deadline = None;
                true
            } else {
                false
            }
        };
        if due {
            self.save_session();
        }
    }

    fn save_session(&self) {
        if *self.demo.lock() {
            return;
        }
        let (urn, pos, vol, balance, mono, queue, current, priority) = {
            let st = self.state.lock();
            let urn = st.current.and_then(|i| st.queue.get(i).map(|t| t.urn()));
            (
                urn,
                st.position_ms,
                st.volume,
                st.balance,
                st.mono,
                st.queue.clone(),
                st.current,
                st.priority_next.clone(),
            )
        };
        // Persist the queue itself so it survives restarts. Tracks are
        // small structs; keep the current one plus its surroundings
        // (at most ~200 entries ≈ 100 KB of JSON).
        let window_start = current.filter(|&index| index < queue.len()).map_or(0, |index| index.saturating_sub(50));
        let (kept, kept_current) = match current {
            Some(i) if i < queue.len() => {
                let start = i.saturating_sub(50);
                let end = (i + 150).min(queue.len());
                (queue[start..end].to_vec(), Some(i - start))
            }
            _ => (queue.into_iter().take(200).collect(), None),
        };
        if let Err(error) = crate::config::update_settings(&self.settings_path, |settings| {
            settings.last_track_urn = urn;
            settings.last_position_ms = Some(pos);
            settings.volume = vol;
            settings.balance = balance;
            settings.mono = mono;
            settings.last_queue_priority = priority.iter().filter_map(|&index| index.checked_sub(window_start)).filter(|&index| index < kept.len()).collect();
            settings.last_queue = kept;
            settings.last_queue_idx = kept_current.filter(|_| !settings.last_queue.is_empty());
        }) {
            log::warn!("cannot save playback session: {error}");
        }
    }

    /// Restore a persisted queue on boot (paused; position applied by caller).
    /// Returns the saved position for the current track, if any.
    pub fn restore_session(&self, queue: Vec<Track>, current: Option<usize>) -> Option<u64> {
        if queue.is_empty() {
            return None;
        }
        let saved = Settings::load_from(&self.settings_path).ok();
        let pos = saved.as_ref().and_then(|s| s.last_position_ms);
        {
            let mut st = self.state.lock();
            let current = current.filter(|&i| i < queue.len());
            st.duration_ms = current
                .and_then(|i| queue.get(i))
                .map(|t| t.effective_duration_ms())
                .unwrap_or(0);
            st.queue = queue;
            st.queue_revision = st.queue_revision.wrapping_add(1);
            st.order = (0..st.queue.len()).collect();
            st.priority_next = saved.as_ref().map(|settings| settings.last_queue_priority.clone()).unwrap_or_default();
            st.current = current;
            protect_manual_next(&mut st);
            st.position_ms = pos.unwrap_or(0);
            st.is_playing = false;
            st.loading = false;
        }
        pos
    }

    /// Snapshot the queue for "save as playlist" (current + upcoming ids).
    pub fn queue_track_ids(&self) -> Vec<u64> {
        let st = self.state.lock();
        let mut ids = Vec::with_capacity(st.queue.len());
        if let Some(cur) = st.current {
            for &i in std::iter::once(&cur).chain(upcoming_order(&st.order, st.current).iter()) {
                if let Some(t) = st.queue.get(i) {
                    ids.push(t.id);
                }
            }
        }
        ids
    }

    pub fn shutdown(&self) {
        *self.stop.lock() = true;
        self.decoder.lock().prefetch.take();
        self.save_session();
    }
}

fn protect_manual_next(state: &mut PlayerState) {
    let start = state.current.and_then(|current| state.order.iter().position(|&index| index == current)).map_or(0, |position| position + 1);
    let upcoming: std::collections::HashSet<_> = state.order[start..].iter().copied().collect();
    let mut seen = std::collections::HashSet::new();
    state.priority_next.retain(|index| *index < state.queue.len() && upcoming.contains(index) && seen.insert(*index));
    let tail: Vec<_> = state.order[start..].iter().copied().filter(|index| !seen.contains(index)).collect();
    state.order.splice(start.., state.priority_next.iter().copied().chain(tail));
}

fn build_order(len: usize, shuffle: bool) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    if shuffle && len > 1 {
        use rand::seq::SliceRandom;
        order.shuffle(&mut rand::rng());
    }
    order
}

fn preview_seek_start(requested_ms: u64, preview_duration_ms: u64) -> u64 {
    if requested_ms >= preview_duration_ms { 0 } else { requested_ms }
}

/// Upcoming indices after `current` in play `order`.
fn upcoming_order(order: &[usize], current: Option<usize>) -> Vec<usize> {
    let Some(cur) = current else {
        return Vec::new();
    };
    match order.iter().position(|&x| x == cur) {
        Some(p) => order[p + 1..].to_vec(),
        None => Vec::new(),
    }
}

/// Remove queue index `idx` from play-order bookkeeping.
/// Returns `(new_order, new_current)`; removing the current row yields
/// `None` so the caller can advance first.
fn drop_bookkeeping(
    order: &[usize],
    current: Option<usize>,
    idx: usize,
) -> (Vec<usize>, Option<usize>) {
    let order: Vec<usize> = order
        .iter()
        .filter(|&&x| x != idx)
        .map(|&x| if x > idx { x - 1 } else { x })
        .collect();
    let current = current.and_then(|cur| {
        if cur == idx {
            None
        } else if cur > idx {
            Some(cur - 1)
        } else {
            Some(cur)
        }
    });
    (order, current)
}

fn clear_played_state(state: &mut PlayerState) -> usize {
    let Some(current) = state.current else { return 0; };
    let Some(position) = state.order.iter().position(|&index| index == current) else { return 0; };
    let removed: std::collections::HashSet<_> = state.order[..position].iter().copied().collect();
    if removed.is_empty() { return 0; }
    let mut mapping = std::collections::HashMap::new();
    let mut old_index = 0;
    let mut kept = 0;
    state.queue.retain(|_| {
        let index = old_index;
        old_index += 1;
        if removed.contains(&index) { false } else { mapping.insert(index, kept); kept += 1; true }
    });
    state.priority_next = state.priority_next.iter().filter_map(|index| mapping.get(index).copied()).collect();
    state.current = mapping.get(&current).copied();
    state.order = state.order.iter().filter_map(|index| mapping.get(index).copied()).collect();
    state.queue_revision = state.queue_revision.wrapping_add(1);
    removed.len()
}

#[cfg(test)]
mod transition_tests;

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn improved_player_fixture(runtime: &tokio::runtime::Runtime, path: std::path::PathBuf) -> Arc<Player> {
        let player = Arc::new(Player::new(Arc::new(AudioOutput::silent([0.0;10],0.8)),Arc::new(crate::api::ApiClient::demo()),Arc::new(AudioCache::disabled()),path,runtime.handle().clone()));
        player.attach(); player.set_demo(true);
        { let mut state = player.state.lock(); state.queue = crate::demo::demo_tracks().into_iter().take(3).collect(); state.order = vec![2,0,1]; state.current = Some(0); state.shuffle = true; state.position_ms = 42000; state.is_playing = false; state.duration_ms = state.queue[0].effective_duration_ms(); }
        player
    }

    #[test]
    fn transition_waiting_for_playback_does_not_block_preference_updates() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let player = improved_player_fixture(&runtime, dir.path().join("settings.json"));
        player.set_audio_preferences(false, 8000, true);
        let playback = player.state.lock();
        let candidate_player = player.clone();
        let (started, entered) = std::sync::mpsc::channel();
        let candidate = std::thread::spawn(move || {
            started.send(()).unwrap();
            candidate_player.transition_candidate()
        });
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        // Let the candidate wait on the deliberately held playback state.
        std::thread::sleep(Duration::from_millis(25));
        let update_player = player.clone();
        let (done, updated) = std::sync::mpsc::channel();
        let update = std::thread::spawn(move || {
            update_player.set_audio_preferences(true, 4000, true);
            done.send(()).unwrap();
        });
        let result = updated.recv_timeout(Duration::from_secs(2));
        // Always release and join, even when testing the old deadlocking order.
        drop(playback);
        candidate.join().unwrap();
        update.join().unwrap();
        assert!(result.is_ok(), "preference update waited behind playback state");
    }
    #[test]
    fn undo_restores_queue_and_shuffle_without_interrupting_the_current_track() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap(); let dir = tempfile::tempdir().unwrap();
        let player = improved_player_fixture(&runtime,dir.path().join("settings.json"));
        player.checkpoint_queue(); player.clear_upcoming(); assert_eq!(player.state.lock().queue.len(),1);
        assert!(player.undo_queue());
        let state = player.state.lock(); assert_eq!(state.order,vec![2,0,1]); assert_eq!(state.queue.len(),3); assert_eq!(state.current,Some(0)); assert_eq!(state.position_ms,42000); assert!(!state.is_playing);
    }
    #[test]
    fn expired_sleep_timer_pauses_without_changing_saved_volume() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap(); let dir = tempfile::tempdir().unwrap();
        let player = improved_player_fixture(&runtime,dir.path().join("settings.json"));
        player.state.lock().is_playing = true;
        *player.sleep_timer.lock() = Some(SleepTimer { deadline: Some(Instant::now()-Duration::from_secs(1)), track_id: None });
        player.update_sleep_timer(); assert!(!player.state.lock().is_playing); assert_eq!(player.state.lock().volume,0.8); assert_eq!(player.sleep_view(),(None,false));
        assert!(player.set_sleep_timer(Some(8000),false).is_err());
    }
    #[test]
    fn choosing_another_track_cancels_only_the_after_track_timer() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap(); let dir = tempfile::tempdir().unwrap();
        let player = improved_player_fixture(&runtime,dir.path().join("settings.json"));
        player.set_sleep_timer(None,true).unwrap();
        assert!(player.next()); assert_eq!(player.sleep_view(),(None,false));
        player.set_sleep_timer(Some(900),false).unwrap();
        assert!(player.prev()); assert!(player.sleep_view().0.is_some());
        player.stop(); assert_eq!(player.sleep_view(),(None,false));
    }
    #[test]
    fn transition_uses_shuffle_order_and_respects_pause_repeat_and_sleep() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap(); let dir = tempfile::tempdir().unwrap();
        let player = improved_player_fixture(&runtime,dir.path().join("settings.json"));
        player.set_audio_preferences(false,4000,true); player.state.lock().is_playing = true;
        assert_eq!(player.transition_candidate().unwrap().1,1);
        player.state.lock().is_playing = false; assert!(player.transition_candidate().is_none());
        player.state.lock().is_playing = true; player.state.lock().repeat = RepeatMode::One; assert!(player.transition_candidate().is_none());
        player.state.lock().repeat = RepeatMode::Off; player.set_sleep_timer(None,true).unwrap(); assert!(player.transition_candidate().is_none());
        assert!(player.finish_sleep_track()); assert!(!player.state.lock().is_playing);
    }
    #[test]
    fn prepared_transition_commits_the_next_track_but_stale_preparation_cannot_take_over() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap(); let dir = tempfile::tempdir().unwrap();
        let player = improved_player_fixture(&runtime,dir.path().join("settings.json"));
        player.set_audio_preferences(false,4000,true);
        { let mut state = player.state.lock(); state.is_playing = true; state.duration_ms = 10000; }
        player.output.start_track(vec![0.2;48000*2],48000,true,9000);
        let next = player.state.lock().queue[1].clone();
        *player.next_track.lock() = Some(PendingNext { generation:0,id:next.id,fade:4000,attempts:1,retry_at:None,prepared:Some(PreparedTrack { duration: next.effective_duration_ms(), track: next.clone(),decoder:DecoderSlot::new(),samples:vec![0.3;48000*2*3],rate:48000,bitrate:160,channels:2 }) });
        player.load_generation.store(1,Ordering::Release);
        assert!(!player.transition_ready(false)); assert_eq!(player.state.lock().current,Some(0));
        player.load_generation.store(0,Ordering::Release);
        assert!(player.transition_ready(false));
        let state = player.state.lock(); assert_eq!(state.current,Some(1)); assert_eq!(state.duration_ms,next.effective_duration_ms()); assert_eq!(state.position_ms,0); assert!(state.is_playing);
    }

    #[test]
    fn eight_second_transition_uses_decoded_tail_when_playlist_duration_is_inaccurate() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        for duration in [30_500, 29_500] {
            let player = improved_player_fixture(&runtime, dir.path().join("settings.json"));
            player.set_demo(false);
            player.set_audio_preferences(false, 8000, false);
            { let mut state = player.state.lock(); state.is_playing = true; state.duration_ms = duration; }
            // All remaining audio is decoded: eight seconds after position 22s.
            player.output.start_track(vec![0.2; 48000 * 2 * 8], 48000, true, 22000);
            let next = player.state.lock().queue[1].clone();
            *player.next_track.lock() = Some(PendingNext { generation: 0, id: next.id, fade: 8000, attempts: 1, retry_at: None,
                prepared: Some(PreparedTrack { duration: next.effective_duration_ms(), track: next,
                    decoder: DecoderSlot::new(), samples: vec![0.3; 48000 * 2 * 11], rate: 48000,
                    bitrate: 160, channels: 2 }) });
            assert!(player.transition_ready(false), "playlist duration {duration} must not suppress the fade");
            assert_eq!(player.state.lock().current, Some(1));
        }
    }

    #[test]
    fn eight_second_transition_waits_for_the_outgoing_audio_to_be_buffered() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let player = improved_player_fixture(&runtime, dir.path().join("settings.json"));
        player.set_demo(false);
        player.set_audio_preferences(false, 8000, false);
        { let mut state = player.state.lock(); state.is_playing = true; state.duration_ms = 30000; }
        player.decoder.lock().exhausted = false;
        player.output.start_track(vec![0.2; 48000 * 2 * 4], 48000, true, 22000);
        let next = player.state.lock().queue[1].clone();
        *player.next_track.lock() = Some(PendingNext { generation: 0, id: next.id, fade: 8000,
            attempts: 1, retry_at: None, prepared: Some(PreparedTrack {
                duration: next.effective_duration_ms(), track: next, decoder: DecoderSlot::new(),
                samples: vec![0.3; 48000 * 2 * 11], rate: 48000, bitrate: 160, channels: 2 }) });
        assert!(!player.transition_ready(false));
        assert_eq!(player.state.lock().current, Some(0));
        player.output.append_samples(&vec![0.2; 48000 * 2 * 4], 48000);
        assert!(player.transition_ready(false));
    }

    #[test]
    fn transition_preparation_retries_once_then_uses_the_prepared_track() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let player = improved_player_fixture(&runtime, dir.path().join("settings.json"));
        player.set_audio_preferences(false, 8000, false);
        { let mut state = player.state.lock(); state.is_playing = true; state.duration_ms = 30000; }
        player.output.start_track(vec![0.2; 48000 * 2 * 8], 48000, true, 22000);
        let next = player.state.lock().queue[1].clone();
        *player.next_track.lock() = Some(PendingNext { generation: 0, id: next.id, fade: 8000,
            prepared: None, attempts: 1, retry_at: Some(Instant::now() - Duration::from_secs(1)) });
        player.prepare_transition();
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if player.next_track.lock().as_ref().is_some_and(|next| next.prepared.is_some()) { break; }
                    tokio::task::yield_now().await;
                }
            }).await.unwrap();
        });
        assert_eq!(player.next_track.lock().as_ref().unwrap().attempts, 2);
        assert!(player.transition_ready(false));
        assert_eq!(player.state.lock().current, Some(1));
    }

    #[test]
    fn transition_preparation_does_not_retry_while_loading_or_after_two_attempts() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        for (attempts, retry_at) in [(1, None), (2, Some(Instant::now() - Duration::from_secs(1)))] {
            let player = improved_player_fixture(&runtime, dir.path().join("settings.json"));
            player.set_audio_preferences(false, 8000, false);
            { let mut state = player.state.lock(); state.is_playing = true; state.duration_ms = 30000; }
            let next = player.state.lock().queue[1].clone();
            *player.next_track.lock() = Some(PendingNext { generation: 0, id: next.id, fade: 8000,
                prepared: None, attempts, retry_at });
            player.prepare_transition();
            runtime.block_on(async { tokio::task::yield_now().await; });
            let pending = player.next_track.lock();
            let pending = pending.as_ref().unwrap();
            assert_eq!(pending.attempts, attempts);
            assert!(pending.prepared.is_none());
        }
    }

    #[test]
    fn recovery_is_bounded_and_permanent_failures_are_not_retried() {
        let mut budget = RecoveryBudget::default();
        let now = Instant::now();
        for _ in 0..3 { assert!(budget.take(1, now)); }
        assert!(!budget.take(1, now));
        assert!(budget.take(2, now));
        assert!(retryable_playback_error(&anyhow::anyhow!("probe audio stream")));
        assert!(retryable_playback_error(&crate::api::error::ApiError::PlaybackLinkExpired.into()));
        assert!(!retryable_playback_error(&crate::api::error::ApiError::Unauthorized.into()));
        assert!(!retryable_playback_error(&anyhow::anyhow!("HTTP 403 stream")));
    }

    #[test]
    fn clear_played_preserves_shuffled_upcoming_and_current_position() {
        let mut state = PlayerState { queue: crate::demo::demo_tracks().into_iter().take(4).collect(),
            current: Some(1), order: vec![3, 1, 0, 2], position_ms: 42_000, ..Default::default() };
        let current = state.queue[1].id;
        assert_eq!(clear_played_state(&mut state), 1);
        assert_eq!(state.queue[state.current.unwrap()].id, current);
        assert_eq!(state.order, vec![1, 0, 2]);
        assert_eq!(state.position_ms, 42_000);
        assert_eq!(clear_played_state(&mut state), 0);
    }

    #[tokio::test]
    async fn recovery_cannot_restart_after_pause_or_overwrite_a_newer_track() {
        let folder = tempfile::tempdir().unwrap();
        let player = Player::new(Arc::new(AudioOutput::silent([0.0; 10], 0.8)),
            Arc::new(crate::api::ApiClient::demo()), Arc::new(AudioCache::disabled()),
            folder.path().join("settings.json"), tokio::runtime::Handle::current());
        player.load_generation.store(1, Ordering::Release);
        let tracks = crate::demo::demo_tracks();
        {
            let mut state = player.state.lock();
            state.queue = tracks[..2].to_vec(); state.current = Some(0); state.position_ms = 42_000;
            state.is_playing = false;
        }
        assert!(!player.recover_stream(1, &anyhow::anyhow!("probe audio stream")));
        assert!(!player.load_track_guarded(tracks[0].clone(), 42_000, Some(1)));
        {
            let mut state = player.state.lock(); state.current = Some(1); state.is_playing = true;
            player.load_generation.store(2, Ordering::Release);
        }
        assert!(!player.recover_stream(1, &anyhow::anyhow!("probe audio stream")));
        assert!(!player.load_track_guarded(tracks[0].clone(), 42_000, Some(1)));
        assert_eq!(player.state.lock().current, Some(1));
        assert!(!player.state.lock().loading);
        player.fail_current_load(1, "stale error".into());
        assert!(player.state.lock().is_playing);
        assert!(player.state.lock().error.is_none());
    }

    #[tokio::test]
    async fn delayed_audio_download_does_not_block_the_player_loop() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/segment", listener.local_addr().unwrap());
        let (began, waiting) = tokio::sync::oneshot::channel();
        let (release, gate) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            socket.read(&mut request).await.unwrap();
            began.send(()).unwrap();
            gate.await.unwrap();
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\naudio").await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let player = Player::new(
            Arc::new(AudioOutput::silent([0.0; 10], 0.8)),
            Arc::new(crate::api::ApiClient::demo()),
            Arc::new(AudioCache::disabled()),
            dir.path().join("settings.json"),
            tokio::runtime::Handle::current(),
        );
        player.load_generation.store(1, Ordering::Release);
        player.decoder.lock().prefetch = Some(player.hls.prefetch("track".into(), vec![url], &player.rt));
        tokio::time::timeout(Duration::from_secs(2), waiting).await.unwrap().unwrap();
        // The 100 ms loop must remain available for position updates and
        // controls even when a segment's network response has not arrived.
        tokio::time::timeout(Duration::from_millis(80), player.fetch_and_append(1, false)).await.unwrap().unwrap();
        assert_eq!(player.decoder.lock().segments_fetched, 0);
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), player.fetch_and_append(1, true)).await.unwrap().unwrap();
        assert_eq!(player.decoder.lock().segments_fetched, 1);
        server.await.unwrap();
    }

    #[test]
    fn order_shuffle_preserves_all_indices() {
        let order = build_order(10, true);
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..10).collect::<Vec<_>>());
    }

    #[test]
    fn order_linear() {
        assert_eq!(build_order(3, false), vec![0, 1, 2]);
    }

    #[test]
    fn seeking_past_a_short_preview_restarts_it_instead_of_playing_an_empty_buffer() {
        assert_eq!(preview_seek_start(120_000, 30_000), 0);
        assert_eq!(preview_seek_start(12_000, 30_000), 12_000);
    }

    #[test]
    fn shuffle_enqueue_keeps_current_and_existing_upcoming_tracks() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let player = Player::new(
            Arc::new(AudioOutput::silent([0.0; 10], 0.8)),
            Arc::new(crate::api::ApiClient::demo()),
            Arc::new(AudioCache::disabled()),
            dir.path().join("settings.json"),
            runtime.handle().clone(),
        );
        let track = |id| Track {
            id, title: format!("Track {id}"),
            ..serde_json::from_str(r#"{"id":0,"title":"Track"}"#).unwrap()
        };
        {
            let mut state = player.state.lock();
            state.queue = (1..=4).map(track).collect();
            state.current = Some(1);
            state.order = vec![0, 1, 3, 2];
            state.shuffle = true;
        }
        player.enqueue(vec![track(5)], true);
        {
            let state = player.state.lock();
            assert_eq!(state.current, Some(1));
            assert_eq!(state.order, vec![0, 1, 2, 4, 3]);
            assert_eq!(state.queue[2].id, 5);
        }
        player.enqueue(vec![track(6)], false);
        let state = player.state.lock();
        assert_eq!(state.order, vec![0, 1, 2, 4, 3, 5]);
    }

    #[test]
    fn manual_next_survives_shuffle_and_autoplay_then_clears_with_queue() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let player = Player::new(Arc::new(AudioOutput::silent([0.0; 10], 0.8)), Arc::new(crate::api::ApiClient::demo()), Arc::new(AudioCache::disabled()), dir.path().join("settings.json"), runtime.handle().clone());
        let track = |id| serde_json::from_value::<Track>(serde_json::json!({"id": id, "title": format!("Track {id}")})).unwrap();
        { let mut state = player.state.lock(); state.queue = (1..=4).map(track).collect(); state.order = vec![0,1,2,3]; state.current = Some(0); }
        player.enqueue(vec![track(5), track(6)], true);
        player.enqueue(vec![track(7)], false);
        for _ in 0..8 {
            player.toggle_shuffle();
            let state = player.state.lock();
            let after = state.order.iter().position(|&i| Some(i) == state.current).unwrap() + 1;
            assert_eq!(state.order[after..after+2].iter().map(|&i| state.queue[i].id).collect::<Vec<_>>(), vec![5,6]);
        }
        player.enqueue(vec![track(8)], true);
        { let state = player.state.lock(); let after = state.order.iter().position(|&i| Some(i) == state.current).unwrap()+1; assert_eq!(state.order[after..after+3].iter().map(|&i| state.queue[i].id).collect::<Vec<_>>(), vec![8,5,6]); }
        player.clear_queue();
        assert!(player.state.lock().priority_next.is_empty());
    }

    #[test]
    fn priority_occurrences_survive_removal_restart_and_undo() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("settings.json");
        let player = Player::new(Arc::new(AudioOutput::silent([0.0;10], 0.8)), Arc::new(crate::api::ApiClient::demo()), Arc::new(AudioCache::disabled()), path.clone(), runtime.handle().clone());
        let track = |id| serde_json::from_value::<Track>(serde_json::json!({"id":id,"title":format!("Track {id}")})).unwrap();
        { let mut state = player.state.lock(); state.queue = vec![track(1),track(2),track(3)]; state.order = vec![0,1,2]; state.current = Some(0); }
        player.enqueue(vec![track(2),track(4)], true);
        // Remove the ordinary copy, preserving the explicitly queued occurrence.
        player.remove_at(3);
        assert_eq!(player.state.lock().priority_next, vec![1,2]);
        player.toggle_shuffle();
        let saved = Settings::load_from(&path).unwrap();
        assert_eq!(saved.last_queue_priority, vec![1,2]);
        let restored = Player::new(Arc::new(AudioOutput::silent([0.0;10], 0.8)), Arc::new(crate::api::ApiClient::demo()), Arc::new(AudioCache::disabled()), path, runtime.handle().clone());
        restored.restore_session(saved.last_queue, saved.last_queue_idx);
        { let state = restored.state.lock(); assert_eq!(state.order[..3], [0,1,2]); assert_eq!(state.priority_next, vec![1,2]); }
        player.checkpoint_queue();
        player.clear_upcoming();
        assert!(player.state.lock().priority_next.is_empty());
        assert!(player.undo_queue());
        assert_eq!(player.state.lock().priority_next, vec![1,2]);
    }

    #[test]
    fn url_encoding() {
        assert_eq!(url_encode("soundcloud:tracks:1"), "soundcloud%3Atracks%3A1");
    }

    #[test]
    fn decode_top_up_is_blocked_while_track_load_owns_decoder() {
        assert!(!should_decode_more(0, true, 0));
    }

    #[test]
    fn decode_top_up_starts_below_target_buffer() {
        assert!(should_decode_more(TARGET_BUFFER_MS - 1, false, 0));
    }

    #[test]
    fn decode_top_up_waits_at_target_buffer() {
        assert!(!should_decode_more(TARGET_BUFFER_MS, false, 0));
    }

    #[test]
    fn eight_second_crossfade_decodes_past_the_fade_boundary_to_discover_eof() {
        assert!(should_decode_more(8000, false, 8000));
        assert!(should_decode_more(9999, false, 8000));
        assert!(!should_decode_more(10000, false, 8000));
        assert!(!should_decode_more(8000, true, 8000));
    }

    #[test]
    fn decoder_eof_waits_for_the_output_buffer_to_finish() {
        assert!(!should_advance_after_decode_end(4_000));
        assert!(!should_advance_after_decode_end(50));
        assert!(!should_advance_after_decode_end(1));
        assert!(should_advance_after_decode_end(0));
    }

    #[test]
    fn attaching_player_does_not_create_an_arc_cycle() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let player = Arc::new(Player::new(
            Arc::new(AudioOutput::silent([0.0; 10], 0.8)),
            Arc::new(crate::api::ApiClient::demo()),
            Arc::new(AudioCache::disabled()),
            std::env::temp_dir().join("fastcloud-cycle-test.json"),
            runtime.handle().clone(),
        ));

        player.attach();

        assert_eq!(Arc::strong_count(&player), 1);
    }

    #[test]
    fn control_persistence_waits_until_dragging_has_settled() {
        let changed_at = std::time::Instant::now();
        let deadline = changed_at + CONTROL_SAVE_DELAY;

        assert!(!control_save_due(Some(deadline), changed_at));
        assert!(control_save_due(Some(deadline), deadline));
    }

    #[test]
    fn seeking_decodes_past_the_local_segment_offset_before_starting_output() {
        assert_eq!(initial_decode_target_ms(0), START_BUFFER_MS);
        assert_eq!(initial_decode_target_ms(70_000), 70_000 + START_BUFFER_MS);
        assert_eq!(initial_decode_target_ms(u64::MAX), u64::MAX);
    }

    #[test]
    fn visible_position_never_runs_past_the_track_duration() {
        assert_eq!(visible_position_ms(144_000, 91_847), 91_847);
        assert_eq!(visible_position_ms(12_000, 0), 12_000);
    }

    #[test]
    fn stream_errors_show_actionable_cause_instead_of_context_label() {
        let error = anyhow::Error::new(crate::api::error::ApiError::Http {
            status: 403, body: "private response content".into(),
        }).context("fetch streams");
        let message = playback_error_message(&error);
        assert!(message.contains("does not allow access"));
        assert!(!message.contains("private response"));
        assert!(!message.contains("fetch streams"));
    }

    #[test]
    fn restored_session_is_always_paused() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let dir = tempfile::tempdir().expect("tempdir");
        let player = Player::new(
            Arc::new(AudioOutput::silent([0.0; 10], 0.8)),
            Arc::new(crate::api::ApiClient::demo()),
            Arc::new(AudioCache::disabled()),
            dir.path().join("settings.json"),
            runtime.handle().clone(),
        );
        player.state.lock().is_playing = true;
        let track: Track = serde_json::from_str(r#"{"id":7,"title":"Restored"}"#).unwrap();

        player.restore_session(vec![track], Some(0));

        assert!(!player.state.lock().is_playing);
    }

    #[test]
    fn paused_restored_position_survives_background_polling() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all().build().expect("runtime");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        crate::config::update_settings(&path, |settings| settings.last_position_ms = Some(42_000)).unwrap();
        let player = Player::new(
            Arc::new(AudioOutput::silent([0.0; 10], 0.8)),
            Arc::new(crate::api::ApiClient::demo()),
            Arc::new(AudioCache::disabled()),
            path,
            runtime.handle().clone(),
        );
        let track: Track = serde_json::from_str(r#"{"id":7,"title":"Restored","duration":120000}"#).unwrap();
        player.restore_session(vec![track], Some(0));
        player.publish_position();
        assert_eq!(player.state.lock().position_ms, 42_000);
    }

    #[tokio::test]
    async fn restored_queue_does_not_fetch_or_advance_before_play() {
        let dir = tempfile::tempdir().expect("tempdir");
        let client = Arc::new(crate::api::ApiClient::new(None, false));
        // Keep even a regressed request on localhost, never a real account.
        let _session = crate::auth::Session::with_test_user(crate::auth::AppCredentials {
            client_id: "test-client".into(), client_secret: String::new(),
            redirect_uri: "http://127.0.0.1:41317/callback".into(),
            server_url: Some("http://127.0.0.1:9".into()),
        }, client.clone()).await;
        let path = dir.path().join("settings.json");
        crate::config::update_settings(&path, |settings| settings.last_position_ms = Some(42_000)).unwrap();
        for repeat in [RepeatMode::Off, RepeatMode::All, RepeatMode::One] {
            let player = Arc::new(Player::new(
                Arc::new(AudioOutput::silent([0.0; 10], 0.8)), client.clone(),
                Arc::new(AudioCache::disabled()), path.clone(), tokio::runtime::Handle::current(),
            ));
            player.attach();
            player.set_repeat(repeat);
            let queue = vec![
                serde_json::from_str(r#"{"id":7,"title":"Restored","duration":120000}"#).unwrap(),
                serde_json::from_str(r#"{"id":8,"title":"Next","duration":120000}"#).unwrap(),
            ];
            player.restore_session(queue, Some(0));
            let task = tokio::spawn(player.clone().run());
            tokio::time::sleep(Duration::from_millis(350)).await;
            player.shutdown();
            task.await.unwrap();
            assert_eq!(player.load_generation.load(Ordering::Acquire), 0, "{repeat:?}");
            let state = player.state.lock();
            assert_eq!(state.current, Some(0), "{repeat:?}");
            assert_eq!(state.position_ms, 42_000, "{repeat:?}");
            assert!(!state.is_playing && !state.loading && state.error.is_none(), "{repeat:?}");
        }
    }

    #[test]
    fn enqueue_next_preserves_current_track_and_tail_order() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all().build().expect("runtime");
        let dir = tempfile::tempdir().expect("tempdir");
        let player = Player::new(
            Arc::new(AudioOutput::silent([0.0; 10], 0.8)),
            Arc::new(crate::api::ApiClient::demo()),
            Arc::new(AudioCache::disabled()),
            dir.path().join("settings.json"),
            runtime.handle().clone(),
        );
        let track = |id| Track {
            id, title: format!("Track {id}"),
            ..serde_json::from_str(r#"{"id":0,"title":"Track"}"#).unwrap()
        };
        {
            let mut state = player.state.lock();
            state.queue = (1..=4).map(track).collect();
            state.current = Some(1);
        }

        player.enqueue(vec![track(5), track(6)], true);

        let state = player.state.lock();
        assert_eq!(state.current, Some(1));
        assert_eq!(state.queue.iter().map(|track| track.id).collect::<Vec<_>>(), vec![1, 2, 5, 6, 3, 4]);
        assert_eq!(state.order, (0..6).collect::<Vec<_>>());
        assert_eq!(state.queue_revision, 1);
    }

    #[test]
    fn trimming_endless_station_keeps_current_and_upcoming_order() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all().build().expect("runtime");
        let dir = tempfile::tempdir().expect("tempdir");
        let player = Player::new(
            Arc::new(AudioOutput::silent([0.0; 10], 0.8)),
            Arc::new(crate::api::ApiClient::demo()),
            Arc::new(AudioCache::disabled()),
            dir.path().join("settings.json"),
            runtime.handle().clone(),
        );
        {
            let mut state = player.state.lock();
            state.queue = (0..150).map(|id| Track {
                id, title: format!("Track {id}"), ..serde_json::from_str(r#"{"id":0,"title":"Track"}"#).unwrap()
            }).collect();
            state.order = (0..150).collect();
            state.current = Some(110);
        }
        assert_eq!(player.trim_played_history(120, 15), 95);
        let state = player.state.lock();
        assert_eq!(state.queue.len(), 55);
        assert_eq!(state.queue[state.current.unwrap()].id, 110);
        assert_eq!(state.order[state.current.unwrap() + 1], 16);
        assert_eq!(state.queue[16].id, 111);
    }

    #[test]
    fn removing_rejected_tracks_preserves_current_and_shuffle_order() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let player = Player::new(
            Arc::new(AudioOutput::silent([0.0; 10], 0.8)),
            Arc::new(crate::api::ApiClient::demo()), Arc::new(AudioCache::disabled()),
            dir.path().join("settings.json"), runtime.handle().clone(),
        );
        {
            let mut state = player.state.lock();
            state.queue = (1..=5).map(|id| Track {
                id, ..serde_json::from_str(r#"{"id":0,"title":"Track"}"#).unwrap()
            }).collect();
            state.current = Some(2);
            state.shuffle = true;
            state.order = vec![1, 2, 4, 0, 3];
        }
        assert_eq!(player.remove_matching(|track| matches!(track.id, 2 | 5)), 2);
        let state = player.state.lock();
        assert_eq!(state.queue.iter().map(|track| track.id).collect::<Vec<_>>(), vec![1, 3, 4]);
        assert_eq!(state.current, Some(1));
        assert_eq!(state.order, vec![1, 0, 2]);
        assert_eq!(state.queue_revision, 1);
    }

    #[test]
    fn removing_current_rejected_track_skips_all_rejected_copies() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let player = Player::new(
            Arc::new(AudioOutput::silent([0.0; 10], 0.8)),
            Arc::new(crate::api::ApiClient::demo()), Arc::new(AudioCache::disabled()),
            dir.path().join("settings.json"), runtime.handle().clone(),
        );
        {
            let mut state = player.state.lock();
            state.queue = (1..=5).map(|id| Track {
                id, ..serde_json::from_str(r#"{"id":0,"title":"Track"}"#).unwrap()
            }).collect();
            state.current = Some(2);
            state.order = vec![0, 2, 4, 1, 3];
        }
        player.remove_matching(|track| matches!(track.id, 3 | 5));
        assert_eq!(player.current_track().unwrap().id, 2);
        player.remove_matching(|_| true);
        assert!(player.current_track().is_none());
        assert!(!player.state.lock().is_playing);
    }

    #[test]
    fn startup_controls_restore_volume_and_remain_paused() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let dir = tempfile::tempdir().expect("tempdir");
        let player = Player::new(
            Arc::new(AudioOutput::silent([0.0; 10], 0.8)),
            Arc::new(crate::api::ApiClient::demo()),
            Arc::new(AudioCache::disabled()),
            dir.path().join("settings.json"),
            runtime.handle().clone(),
        );
        player.state.lock().is_playing = true;

        player.restore_controls(0.23, -0.4, true);
        player.start_paused();

        let state = player.state.lock();
        assert!((state.volume - 0.23).abs() < f32::EPSILON);
        assert!((state.balance - -0.4).abs() < f32::EPSILON);
        assert!(state.mono);
        assert!(!state.is_playing);
    }

    #[test]
    fn clearing_queue_persists_an_empty_session() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let dir = tempfile::tempdir().expect("tempdir");
        let settings_path = dir.path().join("settings.json");
        let player = Player::new(
            Arc::new(AudioOutput::silent([0.0; 10], 0.8)),
            Arc::new(crate::api::ApiClient::demo()),
            Arc::new(AudioCache::disabled()),
            settings_path.clone(),
            runtime.handle().clone(),
        );
        let track: Track = serde_json::from_str(r#"{"id":1,"title":"Song"}"#).unwrap();
        {
            let mut state = player.state.lock();
            state.queue.push(track);
            state.order.push(0);
            state.current = Some(0);
        }

        player.clear_queue();

        let saved = Settings::load_from(&settings_path).unwrap();
        assert!(saved.last_queue.is_empty());
        assert_eq!(saved.last_queue_idx, None);
    }

    #[test]
    fn repeat_cycle() {
        // state machine check without Player instance
        let seq = [
            RepeatMode::Off,
            RepeatMode::All,
            RepeatMode::One,
            RepeatMode::Off,
        ];
        assert_eq!(seq[0], RepeatMode::Off);
    }

    #[test]
    fn upcoming_linear_and_shuffled() {
        assert_eq!(upcoming_order(&[0, 1, 2, 3], Some(1)), vec![2, 3]);
        assert_eq!(upcoming_order(&[2, 0, 3, 1], Some(0)), vec![3, 1]);
        assert_eq!(upcoming_order(&[0, 1], Some(1)), Vec::<usize>::new());
        assert_eq!(upcoming_order(&[0, 1], None), Vec::<usize>::new());
        assert_eq!(upcoming_order(&[0, 1], Some(9)), Vec::<usize>::new());
    }

    #[test]
    fn drop_bookkeeping_middle() {
        let (order, cur) = drop_bookkeeping(&[0, 1, 2, 3], Some(0), 2);
        assert_eq!(order, vec![0, 1, 2]);
        assert_eq!(cur, Some(0));
    }

    #[test]
    fn drop_bookkeeping_before_current() {
        let (order, cur) = drop_bookkeeping(&[0, 1, 2, 3], Some(3), 1);
        assert_eq!(order, vec![0, 1, 2]);
        assert_eq!(cur, Some(2));
    }

    #[test]
    fn drop_bookkeeping_current_yields_none() {
        let (order, cur) = drop_bookkeeping(&[2, 0, 1], Some(0), 0);
        assert_eq!(order, vec![1, 0]);
        assert_eq!(cur, None);
    }
}
