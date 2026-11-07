use super::*;
use crate::api::web_playback::WebPlaybackSource;
use serde::Serialize;

pub(super) struct WebSlot {
    pub(super) generation: u64,
    source: WebPlaybackSource,
    seek_ms: u64,
    seek_revision: u64,
    updated: Instant,
    started: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebPlaybackView {
    pub generation: u64,
    pub source: WebPlaybackSource,
    pub playing: bool,
    pub volume: f32,
    pub speed: f32,
    pub seek_ms: u64,
    pub seek_revision: u64,
}

pub(super) fn eligible(error: &anyhow::Error) -> bool {
    if !skippable_playback_error(error) {
        return false;
    }
    if matches!(
        error.downcast_ref::<crate::api::error::ApiError>(),
        Some(crate::api::error::ApiError::Http {
            status: 403 | 422,
            ..
        })
    ) {
        return true;
    }
    let text = format!("{error:#}").to_ascii_lowercase();
    text.contains("no full stream") || text.contains("stream is a preview")
}

impl Player {
    pub fn enable_web_playback(&self) {
        self.web_enabled.store(true, Ordering::Release);
    }

    pub fn web_is_active(&self) -> bool {
        self.web
            .lock()
            .as_ref()
            .is_some_and(|slot| self.load_is_current(slot.generation))
    }

    pub fn web_view(&self) -> Option<WebPlaybackView> {
        let state = self.state.lock();
        let slot = self.web.lock();
        let slot = slot
            .as_ref()
            .filter(|slot| self.load_is_current(slot.generation))?;
        Some(WebPlaybackView {
            generation: slot.generation,
            source: slot.source.clone(),
            playing: state.is_playing,
            volume: (crate::audio::output::volume_gain(state.volume) * self.output.sleep_gain())
                .clamp(0.0, 1.0),
            speed: state.playback_speed,
            seek_ms: slot.seek_ms,
            seek_revision: slot.seek_revision,
        })
    }

    pub(super) fn touch_web(&self) {
        if let Some(slot) = self.web.lock().as_mut() {
            slot.updated = Instant::now();
        }
    }

    pub(super) fn seek_web(&self, ms: u64) -> bool {
        let mut state = self.state.lock();
        let mut slot = self.web.lock();
        let Some(slot) = slot
            .as_mut()
            .filter(|slot| self.load_is_current(slot.generation))
        else {
            return false;
        };
        slot.seek_ms = ms.min(state.duration_ms.saturating_sub(1));
        slot.seek_revision = slot.seek_revision.wrapping_add(1);
        slot.updated = Instant::now();
        state.position_ms = slot.seek_ms;
        true
    }

    pub(super) async fn load_web(
        &self,
        track: &Track,
        start_ms: u64,
        generation: u64,
    ) -> Result<bool> {
        if !self.web_enabled.load(Ordering::Acquire) || !self.load_is_current(generation) {
            return Ok(false);
        }
        let Some(source) = self.client.web_playback_source(track).await? else {
            return Ok(false);
        };
        let mut state = self.state.lock();
        if !self.load_is_current(generation) {
            return Ok(true);
        }
        state.duration_ms = source.duration_ms;
        state.position_ms = start_ms.min(source.duration_ms.saturating_sub(1));
        state.sample_rate = 0;
        state.bitrate_kbps = source.bitrate_kbps;
        state.channels = 2;
        state.error = None;
        *self.web.lock() = Some(WebSlot {
            generation,
            source,
            seek_ms: state.position_ms,
            seek_revision: 0,
            updated: Instant::now(),
            started: false,
        });
        self.failed_tracks.lock().clear();
        self.output.set_playing(false);
        Ok(true)
    }

    /// Ignore all callbacks from a previous track, even when its license or
    /// decoder finishes after the listener has already pressed Next.
    pub fn web_report(
        &self,
        generation: u64,
        event: &str,
        position_ms: u64,
        seek_revision: Option<u64>,
    ) {
        let mut state = self.state.lock();
        let mut web = self.web.lock();
        let Some(slot) = web
            .as_mut()
            .filter(|slot| slot.generation == generation && self.load_is_current(generation))
        else {
            return;
        };
        if matches!(event, "ready" | "progress" | "playing")
            && seek_revision.is_some_and(|revision| revision != slot.seek_revision)
        {
            return;
        }
        slot.updated = Instant::now();
        match event {
            "ready" | "progress" | "playing" => {
                state.position_ms = position_ms.min(state.duration_ms);
                state.loading = false;
                if event == "playing" && !slot.started {
                    slot.started = true;
                    let path = slot.source.playlist_path.replace("/index.m3u8", "/started");
                    let client = self.client.clone();
                    self.rt.spawn(async move {
                        let _ = client
                            .web_playback_resource(&path, true, b"{}".to_vec())
                            .await;
                    });
                }
            }
            "blocked" => {
                state.is_playing = false;
                state.loading = false;
                state.error = None;
            }
            "rate_limited" => {
                state.is_playing = false;
                state.loading = false;
                state.error = Some(
                    crate::api::error::ApiError::RateLimited {
                        retry_after_ms: None,
                    }
                    .user_message(),
                );
                *web = None;
            }
            "ended" => {
                if !state.is_playing {
                    return;
                }
                let repeat = state.repeat;
                *web = None;
                drop(web);
                drop(state);
                if self.finish_sleep_track() {
                    return;
                }
                if repeat == RepeatMode::One {
                    if let Some(track) = self.current_track() {
                        self.load_track(track, 0);
                    }
                } else if !self.next() {
                    self.maybe_autoplay();
                }
            }
            "failed" => {
                *web = None;
                drop(web);
                drop(state);
                self.handle_playback_failure(
                    generation,
                    &anyhow::anyhow!("SoundCloud web playback is unavailable for this track"),
                );
            }
            _ => {}
        }
    }

    pub(super) fn web_watchdog(&self) {
        let stalled = self
            .web
            .lock()
            .as_ref()
            .filter(|slot| slot.updated.elapsed() > Duration::from_secs(45))
            .map(|slot| slot.generation);
        let playing = self.state.lock().is_playing;
        if let Some(generation) = stalled
            && playing
        {
            self.web_report(generation, "failed", 0, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn web_fallback_does_not_retry_account_denials_or_request_limits() {
        use crate::api::error::ApiError;
        assert!(!eligible(&ApiError::Unauthorized.into()));
        assert!(!eligible(
            &ApiError::RateLimited {
                retry_after_ms: None
            }
            .into()
        ));
        assert!(!eligible(&anyhow::anyhow!(
            "Access was disabled by the owner"
        )));
        assert!(eligible(&anyhow::anyhow!("No full stream URL available")));
        assert!(!eligible(&anyhow::anyhow!(
            "unrelated audio output failure"
        )));
    }

    fn player() -> (tempfile::TempDir, tokio::runtime::Runtime, Arc<Player>) {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let player = Arc::new(Player::new(
            Arc::new(AudioOutput::silent([0.0; 10], 0.8)),
            Arc::new(crate::api::ApiClient::demo()),
            Arc::new(AudioCache::disabled()),
            directory.path().join("settings.json"),
            runtime.handle().clone(),
        ));
        player.attach();
        player.load_generation.store(5, Ordering::Release);
        {
            let mut state = player.state.lock();
            state.queue = crate::demo::demo_tracks();
            state.current = Some(0);
            state.order = (0..state.queue.len()).collect();
            state.is_playing = true;
            state.duration_ms = 127000;
        }
        *player.web.lock() = Some(WebSlot {
            generation: 5,
            source: WebPlaybackSource {
                playlist_path: format!("/v1/media/web/{}/index.m3u8", "a".repeat(43)),
                license_paths: Default::default(),
                duration_ms: 127000,
                bitrate_kbps: 160,
            },
            seek_ms: 0,
            seek_revision: 0,
            updated: Instant::now(),
            started: false,
        });
        (directory, runtime, player)
    }

    #[test]
    fn pause_and_resume_keep_the_same_external_recording() {
        let (_directory, _runtime, player) = player();
        player.play_pause();
        assert!(!player.web_view().unwrap().playing);
        player.play_pause();
        assert!(player.web_view().unwrap().playing);
        assert_eq!(player.load_generation.load(Ordering::Acquire), 5);
    }

    #[test]
    fn old_seek_and_old_track_reports_cannot_move_the_current_position() {
        let (_directory, _runtime, player) = player();
        player.seek_ms(40000);
        assert_eq!(player.web_view().unwrap().seek_revision, 1);
        player.web_report(5, "progress", 3000, Some(0));
        player.web_report(4, "progress", 90000, Some(1));
        assert_eq!(player.state.lock().position_ms, 40000);
        player.web_report(5, "progress", 41000, Some(1));
        assert_eq!(player.state.lock().position_ms, 41000);
    }

    #[test]
    fn a_rate_limit_stops_without_skipping_and_allows_a_fresh_manual_load() {
        let (_directory, _runtime, player) = player();
        player.web_report(5, "rate_limited", 0, None);
        assert!(!player.web_is_active());
        let state = player.state.lock();
        assert!(!state.is_playing);
        assert_eq!(state.current, Some(0));
        assert!(state.error.is_some());
    }

    #[test]
    fn protected_track_end_uses_the_existing_queue_and_repeat_controls() {
        for repeat in [RepeatMode::Off, RepeatMode::One] {
            let (_directory, _runtime, player) = player();
            *player.demo.lock() = true;
            player.state.lock().repeat = repeat;
            player.web_report(5, "ended", 127000, Some(0));
            let state = player.state.lock();
            assert_eq!(
                state.current,
                Some(if repeat == RepeatMode::One { 0 } else { 1 })
            );
            assert_eq!(state.position_ms, 0);
            assert!(state.is_playing);
            drop(state);
            assert!(!player.web_is_active());
        }
    }

    #[test]
    fn protected_playback_preserves_the_ab_loop_without_using_the_silent_native_cursor() {
        let (_directory, _runtime, player) = player();
        {
            let mut state = player.state.lock();
            state.ab_track_id = Some(state.queue[0].id);
            state.ab_start_ms = Some(40000);
            state.ab_end_ms = Some(50000);
        }
        player.web_report(5, "progress", 51000, Some(0));
        player.publish_position();
        assert_eq!(player.state.lock().position_ms, 40000);
        assert_eq!(player.web_view().unwrap().seek_revision, 1);
    }

    #[test]
    fn delayed_failure_from_the_previous_recording_cannot_stop_the_replacement() {
        let (_directory, _runtime, player) = player();
        player.load_generation.store(6, Ordering::Release);
        player.web.lock().as_mut().unwrap().generation = 6;
        player.web_report(5, "failed", 0, None);
        assert!(player.web_is_active());
        assert!(player.state.lock().is_playing);
    }

    #[test]
    fn protected_audio_uses_the_same_quiet_volume_curve_and_sleep_fade() {
        let (_directory, _runtime, player) = player();
        player.state.lock().volume = 0.05;
        assert!((player.web_view().unwrap().volume - 0.0025).abs() < 0.00001);
        player.state.lock().volume = 0.5;
        player.output.set_sleep_gain(0.4);
        assert!((player.web_view().unwrap().volume - 0.1).abs() < 0.00001);
    }

    #[test]
    fn backend_migration_keeps_a_paused_queue_and_position_without_reusing_old_tickets() {
        let (_directory, _runtime, player) = player();
        {
            let mut state = player.state.lock();
            state.position_ms = 42000;
            state.is_playing = false;
        }
        let ids: Vec<_> = player
            .state
            .lock()
            .queue
            .iter()
            .map(|track| track.id)
            .collect();
        player.backend_moved();
        assert!(!player.web_is_active());
        assert!(player.resume_load().is_some());
        let state = player.state.lock();
        assert_eq!(state.current, Some(0));
        assert_eq!(state.position_ms, 42000);
        assert_eq!(
            state.queue.iter().map(|track| track.id).collect::<Vec<_>>(),
            ids
        );
        assert!(!state.is_playing);
    }
}
