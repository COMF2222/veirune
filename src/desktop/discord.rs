//! Non-blocking Discord Rich Presence worker.

use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Receiver, Sender};
use discord_rich_presence::{
    DiscordIpc, DiscordIpcClient,
    activity::{Activity, ActivityType, Assets, Button, Timestamps},
};

#[derive(Debug, Clone)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub artwork_url: Option<String>,
    pub track_url: Option<String>,
    pub duration_secs: i64,
    pub elapsed_secs: i64,
    pub playing: bool,
}

enum Command {
    Configure(Option<String>),
    Activity(NowPlaying),
    Clear,
    Shutdown,
}

fn next_retry_delay(current: Duration) -> Duration {
    Duration::from_secs(current.as_secs().saturating_mul(2).min(60))
}

const RETRY_INITIAL: Duration = Duration::from_secs(2);
const RETRY_TICK: Duration = Duration::from_secs(1);

struct WorkerState {
    client_id: Option<String>,
    client: Option<DiscordIpcClient>,
    latest: Option<NowPlaying>,
    retry_at: Instant,
    retry_delay: Duration,
}

/// Sends presence updates to a dedicated thread so Discord IPC never stalls egui.
pub struct Presence {
    tx: Sender<Command>,
}

impl Presence {
    pub fn spawn() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        thread::Builder::new()
            .name("fastcloud-discord".to_owned())
            .spawn(move || run(rx))
            .unwrap_or_else(|error| {
                log::warn!("Discord worker unavailable: {error}");
                thread::spawn(|| {})
            });
        Self { tx }
    }

    pub fn configure(&self, client_id: Option<String>) {
        let _ = self.tx.send(Command::Configure(client_id));
    }

    pub fn update(&self, track: NowPlaying) {
        let _ = self.tx.send(Command::Activity(track));
    }

    pub fn clear(&self) {
        let _ = self.tx.send(Command::Clear);
    }
}

impl Drop for Presence {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
    }
}

fn run(rx: Receiver<Command>) {
    let mut state = WorkerState {
        client_id: None,
        client: None,
        latest: None,
        retry_at: Instant::now(),
        retry_delay: RETRY_INITIAL,
    };
    loop {
        match rx.recv_timeout(RETRY_TICK) {
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                close(&mut state.client);
                break;
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Ok(command) => match command {
                Command::Configure(client_id) => {
                    close(&mut state.client);
                    state.client_id = client_id
                        .map(|id| id.trim().to_owned())
                        .filter(|id| !id.is_empty());
                    state.retry_at = Instant::now();
                    state.retry_delay = RETRY_INITIAL;
                }
                Command::Activity(track) => {
                    if track.playing && state.latest.as_ref().is_none_or(|previous| !previous.playing) {
                        state.retry_at = Instant::now();
                    }
                    state.latest = Some(track);
                    let failed = state
                        .client
                        .as_mut()
                        .zip(state.latest.as_ref())
                        .and_then(|(client, track)| set_activity(client, track).err());
                    if let Some(error) = failed {
                        log::warn!("Discord Rich Presence update failed: {error}");
                        close(&mut state.client);
                        schedule_retry(&mut state);
                    }
                }
                Command::Clear => {
                    state.latest = None;
                    if let Some(active) = state.client.as_mut() {
                        let _ = active.clear_activity();
                    }
                }
                Command::Shutdown => {
                    close(&mut state.client);
                    break;
                }
            },
        }

        if state.client.is_none()
            && state.client_id.is_some()
            && state.latest.as_ref().is_some_and(|track| track.playing)
            && Instant::now() >= state.retry_at
        {
            reconnect(&mut state);
        }
    }
}

fn reconnect(state: &mut WorkerState) {
    let Some(client_id) = state.client_id.as_deref() else {
        return;
    };
    let Some(track) = state.latest.as_ref() else {
        return;
    };
    let mut next = DiscordIpcClient::new(client_id);
    let result = next
        .connect()
        .map_err(|error| error.to_string())
        .and_then(|()| set_activity(&mut next, track));
    match result {
        Ok(()) => {
            state.client = Some(next);
            state.retry_delay = RETRY_INITIAL;
        }
        Err(error) => {
            log::warn!("Discord Rich Presence connection failed: {error}");
            let _ = next.close();
            schedule_retry(state);
        }
    }
}

fn schedule_retry(state: &mut WorkerState) {
    state.retry_at = Instant::now() + state.retry_delay;
    state.retry_delay = next_retry_delay(state.retry_delay);
}

fn close(client: &mut Option<DiscordIpcClient>) {
    if let Some(mut active) = client.take() {
        let _ = active.close();
    }
}

fn activity_for(track: &NowPlaying, now_ms: i64) -> Option<Activity<'_>> {
    // Discord can keep counting the activity session even without timestamps.
    // Clear the listening activity on pause instead of publishing a paused song.
    if !track.playing { return None; }
    let start = now_ms.saturating_sub(track.elapsed_secs.max(0).saturating_mul(1000));
    let large_image = track.artwork_url.as_deref().unwrap_or("fastcloud");
    let mut activity = Activity::new()
        .activity_type(ActivityType::Listening)
        .details(&track.title)
        .state(&track.artist)
        .assets(Assets::new().large_image(large_image))
        .timestamps(Timestamps::new().start(start)
            .end(start.saturating_add(track.duration_secs.max(0).saturating_mul(1000))));
    if let Some(url) = track.track_url.as_deref() {
        activity = activity.buttons(vec![Button::new("Listen on SoundCloud", url)]);
    }
    Some(activity)
}

fn set_activity(client: &mut DiscordIpcClient, track: &NowPlaying) -> Result<(), String> {
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default()
        .as_millis().min(i64::MAX as u128) as i64;
    match activity_for(track, now_ms) {
        Some(activity) => client.set_activity(activity),
        None => client.clear_activity(),
    }.map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_clears_the_activity_and_resume_uses_the_saved_position_in_milliseconds() {
        let mut track = NowPlaying { title:"Song".into(), artist:"Artist".into(), artwork_url:None,
            track_url:None, duration_secs:180, elapsed_secs:65, playing:false };
        assert!(activity_for(&track, 1_800_000_000_000).is_none());
        track.playing = true;
        let value = serde_json::to_value(activity_for(&track, 1_800_000_000_000).unwrap()).unwrap();
        assert_eq!(value["timestamps"]["start"], 1_799_999_935_000i64);
        assert_eq!(value["timestamps"]["end"], 1_800_000_115_000i64);
        assert_eq!(value["state"], "Artist");
    }

    #[test]
    fn retry_delay_doubles_until_the_one_minute_cap() {
        assert_eq!(
            next_retry_delay(std::time::Duration::from_secs(2)),
            std::time::Duration::from_secs(4)
        );
        assert_eq!(
            next_retry_delay(std::time::Duration::from_secs(40)),
            std::time::Duration::from_secs(60)
        );
    }
}
