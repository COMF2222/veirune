#![allow(dead_code)]

use super::dsp::{Eq10, Limiter};
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::{Condvar, Mutex};
use std::sync::Arc;

const SOUNDCLOUD_NATIVE_SAMPLE_RATE: u32 = 44_100;
const MAX_FALLBACK_SAMPLE_RATE: u32 = 48_000;

/// Keep the perceived-volume curve identical for native and CDM playback.
pub fn volume_gain(volume: f32) -> f32 {
    volume.clamp(0.0, 1.0).powi(2)
}

fn preferred_output_sample_rate(min: u32, max: u32) -> u32 {
    if (min..=max).contains(&SOUNDCLOUD_NATIVE_SAMPLE_RATE) {
        SOUNDCLOUD_NATIVE_SAMPLE_RATE
    } else {
        max.min(MAX_FALLBACK_SAMPLE_RATE).max(min)
    }
}

fn limit_if_needed(limiter: &mut Limiter, enabled: bool, left: f32, right: f32) -> (f32, f32) {
    if enabled {
        limiter.process(left, right)
    } else {
        (left, right)
    }
}

fn output_rate_rank(min: u32, max: u32) -> (bool, u32) {
    let selected = preferred_output_sample_rate(min, max);
    (
        selected < SOUNDCLOUD_NATIVE_SAMPLE_RATE,
        selected.abs_diff(SOUNDCLOUD_NATIVE_SAMPLE_RATE),
    )
}

fn output_config_rank(
    channels: u16,
    is_float: bool,
    min_rate: u32,
    max_rate: u32,
) -> (bool, bool, bool, u32, u16) {
    // The callback is f32, so a native float format comes first. Never let a
    // telephone-rate stereo mode beat a full-rate surround mode; once both
    // candidates are full-rate, exact stereo avoids Windows remapping.
    let (below_native_rate, distance_from_native) = output_rate_rank(min_rate, max_rate);
    (
        !is_float,
        below_native_rate,
        channels != 2,
        distance_from_native,
        channels,
    )
}

#[inline]
fn write_stereo_frame(frame: &mut [f32], left: f32, right: f32) {
    frame.fill(0.0);
    match frame {
        [] => {}
        [mono] => *mono = (left + right) * 0.5,
        [left_out, right_out, ..] => {
            *left_out = left;
            *right_out = right;
        }
    }
}

/// Shared playback state consumed by the cpal audio callback.
pub struct OutputState {
    tail: Option<FadeTail>,
    pub normalization: bool,
    recording_gain: f32,
    smooth_gain: f32,
    pub sleep_gain: f32,
    /// Interleaved stereo f32 samples at `source_rate`, produced by the decode thread.
    /// Consumed prefix is drained by the producer side; see `maybe_compact`.
    pub buffer: Vec<f32>,
    /// Consumed position in source frames **within `buffer`**.
    /// Fractional part is the resampler phase. Owned by the audio thread,
    /// read (never written) elsewhere under the same lock.
    pub pos_frames: f64,
    /// Frames of this track that `buffer` no longer holds: the skipped
    /// prefix at load time plus everything `maybe_compact` has drained.
    ///
    /// Without it the reported position would jump back to zero every time
    /// the buffer was compacted, which is exactly what froze the progress
    /// bar mid-track: `Player::run` only ever moved it forward.
    pub consumed_frames: u64,
    pub source_rate: u32,
    pub playing: bool,
    pub volume: f32,
    /// Playback rate. Resampling also shifts pitch; the UI labels this clearly.
    pub playback_speed: f32,
    /// Stereo balance, -1 hard left to 1 hard right. Applied with the volume.
    pub balance: f32,
    /// Fold both source channels to their average before volume/balance.
    pub mono: bool,
    pub eq_gains: [f32; 10],
    pub eq_enabled: bool,
    /// The equaliser's preamp in dB, a flat gain before its filters.
    pub eq_preamp_db: f32,
    pub limiter_threshold_db: f32,
    /// Bumped on every `start_track`; the audio thread resets EQ/limiter
    /// memories when it sees a new value (no clicks/pops leaking across tracks).
    pub track_seq: u64,
}

impl Default for OutputState {
    fn default() -> Self {
        Self {
            tail: None, normalization: false, recording_gain: 1.0, smooth_gain: 1.0, sleep_gain: 1.0,
            buffer: Vec::new(),
            pos_frames: 0.0,
            consumed_frames: 0,
            source_rate: 44_100,
            playing: false,
            volume: 0.8,
            playback_speed: 1.0,
            balance: 0.0,
            mono: false,
            eq_gains: [0.0; 10],
            eq_enabled: false,
            eq_preamp_db: 0.0,
            limiter_threshold_db: -1.0,
            track_seq: 0,
        }
    }
}

struct FadeTail {
    buffer: Vec<f32>, pos: f64, rate: u32, duration_frames: f64, elapsed_frames: f64, gain: f32,
}

fn mixed_frame(state: &mut OutputState, device_rate: u32) -> (f32, f32) {
    let ratio = state.source_rate as f64 / device_rate.max(1) as f64 * f64::from(state.playback_speed);
    let target = if state.normalization { state.recording_gain } else { 1.0 };
    state.smooth_gain += (target - state.smooth_gain) * 0.002;
    let (l,r) = next_frame(state, ratio);
    let mut result = (l * state.smooth_gain, r * state.smooth_gain);
    if let Some(tail) = &mut state.tail {
        let index = tail.pos.floor() as usize;
        let amount = (tail.elapsed_frames / tail.duration_frames.max(1.0)).clamp(0.0,1.0) as f32;
        if index + 1 < tail.buffer.len() / 2 {
            let fraction = (tail.pos - index as f64) as f32;
            let old_l = tail.buffer[index*2] + (tail.buffer[(index+1)*2] - tail.buffer[index*2]) * fraction;
            let old_r = tail.buffer[index*2+1] + (tail.buffer[(index+1)*2+1] - tail.buffer[index*2+1]) * fraction;
            // Complementary linear ramps avoid the +3 dB sum of identical
            // signals produced by equal-power fades.
            result = (result.0 * amount + old_l * tail.gain * (1.0-amount), result.1 * amount + old_r * tail.gain * (1.0-amount));
        }
        tail.pos += tail.rate as f64 / device_rate.max(1) as f64 * f64::from(state.playback_speed);
        tail.elapsed_frames += f64::from(state.playback_speed);
        if tail.elapsed_frames >= tail.duration_frames { state.tail = None; }
    }
    (result.0 * state.sleep_gain, result.1 * state.sleep_gain)
}

impl OutputState {
    /// Where playback is in the *track*, in source frames.
    fn absolute_frames(&self) -> u64 {
        self.consumed_frames + self.pos_frames.floor().max(0.0) as u64
    }

    /// Per-channel gain for the current volume and balance.
    ///
    /// Winamp's balance attenuates the far channel rather than boosting the
    /// near one, so centre is unity and no setting can clip what was not
    /// already clipping.
    fn channel_gain(&self) -> (f32, f32) {
        let balance = self.balance.clamp(-1.0, 1.0);
        // A linear slider leaves the first few percent surprisingly loud.
        // Square the control value so the low end has useful precision, while
        // zero still produces exact silence.
        let gain = volume_gain(self.volume);
        let left = gain * (1.0 - balance.max(0.0));
        let right = gain * (1.0 + balance.min(0.0));
        (left, right)
    }

    fn channel_mode(&self, left: f32, right: f32) -> (f32, f32) {
        if self.mono {
            let mixed = (left + right) * 0.5;
            (mixed, mixed)
        } else {
            (left, right)
        }
    }
}

/// Handle for pushing decoded samples into the cpal callback.
///
/// `_stream` is `None` for a [silent][Self::silent] output: the app still
/// runs, browses and queues without a sound card (headless sessions, a
/// machine with no default device), it just makes no noise.
pub struct AudioOutput {
    state: Arc<Mutex<OutputState>>,
    cond: Arc<Condvar>,
    _stream: Option<cpal::Stream>,
    device_sample_rate: u32,
    /// Half a second of post-EQ, pre-volume audio for the visualisers.
    tap: Arc<crate::vis::AudioTap>,
    /// Whether a real device is attached.
    audible: bool,
}

impl AudioOutput {
    /// An output with no device: everything works except the sound.
    ///
    /// Used when no default output device can be opened, so the window still
    /// comes up and says so, instead of the app refusing to start.
    pub fn silent(default_gains: [f32; 10], default_volume: f32) -> Self {
        Self {
            state: Arc::new(Mutex::new(OutputState {
                eq_gains: default_gains,
                volume: default_volume,
                ..Default::default()
            })),
            cond: Arc::new(Condvar::new()),
            _stream: None,
            device_sample_rate: 48_000,
            tap: crate::vis::AudioTap::new(),
            audible: false,
        }
    }

    /// Whether audio can actually be heard (a device was opened).
    pub fn audible(&self) -> bool {
        self.audible
    }

    pub fn open(default_gains: [f32; 10], default_volume: f32) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .context("no audio output device")?;
        let ranges: Vec<_> = device
            .supported_output_configs()
            .context("query output configs")?
            .collect();
        let range = *ranges
            .iter()
            .filter(|c| c.channels() >= 2)
            .min_by_key(|c| {
                output_config_rank(
                    c.channels(),
                    c.sample_format().is_float(),
                    c.min_sample_rate(),
                    c.max_sample_rate(),
                )
            })
            .context("no stereo output config")?;
        let device_rate =
            preferred_output_sample_rate(range.min_sample_rate(), range.max_sample_rate());
        let config = range.with_sample_rate(device_rate);
        let output_channels = usize::from(config.channels());
        log::info!(
            "audio output: device={}, channels={}, rate={} Hz, format={:?}",
            device,
            output_channels,
            config.sample_rate(),
            config.sample_format()
        );

        let state = Arc::new(Mutex::new(OutputState {
            eq_gains: default_gains,
            volume: default_volume,
            ..Default::default()
        }));
        let cond = Arc::new(Condvar::new());
        let tap = crate::vis::AudioTap::new();

        let st = state.clone();
        let config_rate = config.sample_rate();
        let mut eq = Eq10::new(config_rate as f32, default_gains);
        let mut limiter = Limiter::new(config_rate as f32, -1.0);
        let mut last_thresh = -1.0f32;
        let mut last_seq: u64 = 0;
        let stream_tap = tap.clone();
        // Reused between callbacks so the audio thread never allocates.
        let mut tapped: Vec<f32> = Vec::new();

        let stream_config = config.config();
        let stream = device.build_output_stream(
            stream_config,
            move |data: &mut [f32], _| {
                let mut state = st.lock();
                // New track: drop filter memories so the previous song's
                // tail doesn't click/pop into the next one.
                if state.track_seq != last_seq {
                    last_seq = state.track_seq;
                    eq.reset();
                    limiter = Limiter::new(config_rate as f32, state.limiter_threshold_db);
                    last_thresh = state.limiter_threshold_db;
                    stream_tap.clear();
                }
                if eq.gains() != state.eq_gains {
                    eq.set_gains(state.eq_gains);
                }
                if (eq.preamp_db() - state.eq_preamp_db).abs() > f32::EPSILON {
                    eq.set_preamp_db(state.eq_preamp_db);
                }
                if (state.limiter_threshold_db - last_thresh).abs() > f32::EPSILON {
                    last_thresh = state.limiter_threshold_db;
                    limiter.set_threshold_db(last_thresh);
                }
                if !state.playing {
                    // Paused: silence without advancing (seek/position stay).
                    data.fill(0.0);
                    return;
                }
                let (gain_l, gain_r) = state.channel_gain();
                tapped.clear();
                for frame in data.chunks_mut(output_channels) {
                    let (l, r) = mixed_frame(&mut state, config_rate);
                    let (mut l, mut r) = if state.eq_enabled {
                        eq.process_stereo(l, r)
                    } else {
                        (l, r)
                    };
                    (l, r) = state.channel_mode(l, r);
                    // Tap post-EQ and pre-volume: the equalizer shapes what
                    // the bars show, the volume knob never moves them.
                    tapped.push(l);
                    tapped.push(r);
                    l *= gain_l;
                    r *= gain_r;
                    let (l, r) = limit_if_needed(&mut limiter, state.eq_enabled || state.normalization || state.tail.is_some(), l, r);
                    write_stereo_frame(frame, l, r);
                }
                drop(state);
                stream_tap.push(&tapped, 1.0);
            },
            |err| log::error!("audio output error: {err}"),
            None,
        )?;
        stream.play()?;

        Ok(Self {
            state,
            cond,
            _stream: Some(stream),
            device_sample_rate: config_rate,
            tap,
            audible: true,
        })
    }

    /// The visualisers' audio tap (post-EQ, pre-volume).
    pub fn tap(&self) -> &Arc<crate::vis::AudioTap> {
        &self.tap
    }

    /// Replace the sample buffer (new track). `samples` are interleaved
    /// stereo at `rate`, already starting at `start_ms` of the track.
    pub fn start_track(&self, samples: Vec<f32>, rate: u32, playing: bool, start_ms: u64) {
        let mut s = self.state.lock();
        let rate = rate.max(1);
        s.recording_gain = super::level::recording_gain(&samples);
        s.smooth_gain = if s.normalization { s.recording_gain } else { 1.0 };
        s.tail = None;
        s.buffer = samples;
        s.pos_frames = 0.0;
        s.consumed_frames = start_ms * u64::from(rate) / 1000;
        s.source_rate = rate;
        s.playing = playing;
        s.track_seq = s.track_seq.wrapping_add(1);
        self.cond.notify_all();
    }

    /// Preserve only the buffered outgoing tail; all further decoding belongs
    /// to the incoming recording. Source rates are resampled independently.
    pub fn start_crossfade(&self, samples: Vec<f32>, rate: u32, fade_ms: u64) {
        let mut state = self.state.lock();
        maybe_compact(&mut state);
        let frames = ((fade_ms * u64::from(state.source_rate)) / 1000) as usize;
        let old = state.buffer[..state.buffer.len().min((frames + 2) * 2)].to_vec();
        let tail = FadeTail { buffer: old, pos: state.pos_frames, rate: state.source_rate,
            duration_frames: fade_ms as f64 * f64::from(self.device_sample_rate) / 1000.0,
            elapsed_frames: 0.0, gain: state.smooth_gain };
        state.recording_gain = super::level::recording_gain(&samples);
        state.smooth_gain = if state.normalization { state.recording_gain } else { 1.0 };
        state.buffer = samples; state.source_rate = rate.max(1); state.pos_frames = 0.0; state.consumed_frames = 0;
        state.tail = if fade_ms > 0 { Some(tail) } else { None };
        state.track_seq = state.track_seq.wrapping_add(1);
        self.cond.notify_all();
    }

    pub fn set_normalization(&self, enabled: bool) { self.state.lock().normalization = enabled; }
    pub fn set_sleep_gain(&self, gain: f32) { self.state.lock().sleep_gain = gain.clamp(0.0,1.0); }
    pub fn sleep_gain(&self) -> f32 { self.state.lock().sleep_gain }

    /// Append decoded samples for gapless continuation.
    /// Drains the consumed prefix first so long HLS sessions stay bounded
    /// (a few tens of seconds max instead of the whole track).
    pub fn append_samples(&self, samples: &[f32], rate: u32) {
        let mut s = self.state.lock();
        if s.source_rate == 0 {
            s.source_rate = rate.max(1);
        }
        maybe_compact(&mut s);
        s.buffer.extend_from_slice(samples);
        self.cond.notify_all();
    }

    /// How much decoded audio is held in memory, in bytes.
    pub fn buffer_bytes(&self) -> usize {
        self.state.lock().buffer.len() * std::mem::size_of::<f32>()
    }

    pub fn set_playing(&self, playing: bool) {
        self.state.lock().playing = playing;
        self.cond.notify_all();
    }

    pub fn set_volume(&self, volume: f32) {
        self.state.lock().volume = volume.clamp(0.0, 1.0);
    }

    pub fn set_playback_speed(&self, speed: f32) {
        self.state.lock().playback_speed = speed.clamp(0.5, 2.0);
    }

    /// Stereo balance, -1 hard left to 1 hard right.
    pub fn set_balance(&self, balance: f32) {
        self.state.lock().balance = balance.clamp(-1.0, 1.0);
    }

    pub fn set_mono(&self, mono: bool) {
        self.state.lock().mono = mono;
    }

    pub fn set_eq(&self, enabled: bool, gains: [f32; 10]) {
        let mut s = self.state.lock();
        s.eq_enabled = enabled;
        s.eq_gains = gains;
    }

    /// The equaliser's preamp, in dB.
    pub fn set_eq_preamp_db(&self, preamp_db: f32) {
        self.state.lock().eq_preamp_db =
            preamp_db.clamp(-super::dsp::EQ_RANGE_DB, super::dsp::EQ_RANGE_DB);
    }

    /// Seek to a position given in track milliseconds.
    ///
    /// Only what the buffer still holds can be reached: a seek before the
    /// drained prefix, or past the tail, clamps. The player reloads the
    /// stream for anything further (see `Player::seek_ms`).
    pub fn seek_ms(&self, ms: u64) {
        let mut s = self.state.lock();
        s.tail = None;
        let rate = u64::from(s.source_rate.max(1));
        let target = ms * rate / 1000;
        let total = s.buffer.len() as u64 / 2;
        let offset = target.saturating_sub(s.consumed_frames);
        s.pos_frames = offset.min(total.saturating_sub(1)) as f64;
        self.cond.notify_all();
    }

    /// Whether `ms` is inside the buffer the output still holds.
    pub fn can_seek_to(&self, ms: u64) -> bool {
        let s = self.state.lock();
        let rate = u64::from(s.source_rate.max(1));
        let target = ms * rate / 1000;
        let total = s.buffer.len() as u64 / 2;
        target >= s.consumed_frames && target < s.consumed_frames + total
    }

    /// Current playback position in ms (at source rate).
    pub fn position_ms(&self) -> u64 {
        let s = self.state.lock();
        s.absolute_frames() * 1000 / u64::from(s.source_rate.max(1))
    }

    pub fn buffered_ms(&self) -> u64 {
        let s = self.state.lock();
        let total = s.buffer.len() as u64 / 2;
        let played = s.pos_frames.floor().max(0.0) as u64;
        total.saturating_sub(played) * 1000 / u64::from(s.source_rate.max(1))
    }

    /// Where the buffer ends, in track milliseconds. This is the point a
    /// producer should synthesize or decode from next.
    pub fn buffered_until_ms(&self) -> u64 {
        let s = self.state.lock();
        let total = s.buffer.len() as u64 / 2;
        (s.consumed_frames + total) * 1000 / u64::from(s.source_rate.max(1))
    }

    pub fn is_playing(&self) -> bool {
        self.state.lock().playing
    }

    /// Block until data is available or the timeout elapses.
    pub fn underrun_wait(&self, timeout: std::time::Duration) -> bool {
        let mut s = self.state.lock();
        loop {
            let total = s.buffer.len() / 2;
            let played = s.pos_frames.floor().max(0.0) as usize;
            let avail = total.saturating_sub(played);
            if avail > 0 || !s.playing {
                return true;
            }
            let result = self.cond.wait_for(&mut s, timeout);
            if result.timed_out() {
                return false;
            }
        }
    }

    pub fn device_sample_rate(&self) -> u32 {
        self.device_sample_rate
    }

    #[cfg(test)]
    pub(crate) fn render_test_audio(&self, frames: usize) -> Vec<f32> {
        let mut state = self.state.lock();
        let mut samples = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            let frame = if state.playing { mixed_frame(&mut state, self.device_sample_rate) } else { (0.0, 0.0) };
            samples.extend_from_slice(&[frame.0, frame.1]);
        }
        samples
    }
}

/// Drop fully-consumed frames from the front of the buffer.
/// Keeps the frame under the resampler phase (interpolation reads
/// frame `floor(pos)` and `floor(pos)+1`), drops everything before it and
/// remembers how much went, so the reported position stays absolute.
/// Called on the producer side, never on the audio thread.
fn maybe_compact(s: &mut OutputState) {
    let done = s.pos_frames.floor().max(0.0) as usize;
    if done == 0 {
        return;
    }
    let drop_samples = done.saturating_mul(2).min(s.buffer.len());
    if drop_samples == 0 {
        return;
    }
    let dropped_frames = drop_samples / 2;
    s.buffer.drain(..drop_samples);
    s.pos_frames -= dropped_frames as f64;
    s.consumed_frames += dropped_frames as u64;
    if s.pos_frames < 0.0 {
        s.pos_frames = 0.0;
    }
}

/// Frame-aligned linear-interpolation resampling pull.
/// `pos` is in source frames; the sample index stays even, so stereo
/// channels can never swap when source and device rates differ.
#[inline]
fn next_frame(state: &mut OutputState, sr_ratio: f64) -> (f32, f32) {
    let total_frames = state.buffer.len() / 2;
    if state.source_rate == 0 || total_frames < 2 {
        return (0.0, 0.0);
    }
    let pos = state.pos_frames;
    if !pos.is_finite() || pos < 0.0 {
        state.pos_frames = 0.0;
        return (0.0, 0.0);
    }
    let i = pos.floor() as usize;
    // Need frame i and frame i+1 for interpolation.
    if i + 1 >= total_frames {
        // Starved: output silence (decode thread will append more).
        return (0.0, 0.0);
    }
    let frac = (pos - i as f64) as f32;
    let b = &state.buffer;
    let l = b[i * 2] + (b[(i + 1) * 2] - b[i * 2]) * frac;
    let r = b[i * 2 + 1] + (b[(i + 1) * 2 + 1] - b[i * 2 + 1]) * frac;
    state.pos_frames = pos + sr_ratio;
    (l, r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossfade_blends_rates_without_exceeding_identical_source_level() {
        let output = AudioOutput::silent([0.0;10],1.0);
        output.start_track(vec![0.5; 44100 * 2],44100,true,0);
        output.start_crossfade(vec![0.5; 48000 * 2],48000,1000);
        let mut state = output.state.lock();
        for _ in 0..48000 { let (l,r) = mixed_frame(&mut state,48000); assert!(l <= 0.5001 && r <= 0.5001); }
        assert!(state.tail.is_none());
        assert_eq!(state.source_rate,48000);
    }

    #[test]
    fn eight_second_crossfade_keeps_both_tracks_audible_and_finishes_on_the_incoming_track() {
        let output = AudioOutput::silent([0.0; 10], 1.0);
        let outgoing: Vec<f32> = (0..44100 * 10).flat_map(|_| [0.4, 0.0]).collect();
        let incoming: Vec<f32> = (0..48000 * 11).flat_map(|_| [0.0, 0.6]).collect();
        output.start_track(outgoing, 44100, true, 0);
        output.seek_ms(2000);
        output.start_crossfade(incoming, 48000, 8000);
        let mut state = output.state.lock();
        let first = mixed_frame(&mut state, 48000);
        assert!((first.0 - 0.4).abs() < 0.0001 && first.1.abs() < 0.0001);
        for _ in 1..48000 * 4 { mixed_frame(&mut state, 48000); }
        let middle = mixed_frame(&mut state, 48000);
        assert!((middle.0 - 0.2).abs() < 0.0001 && (middle.1 - 0.3).abs() < 0.0001);
        for _ in 48000 * 4 + 1..48000 * 8 { mixed_frame(&mut state, 48000); }
        let last = mixed_frame(&mut state, 48000);
        assert!(last.0.abs() < 0.0001 && (last.1 - 0.6).abs() < 0.0001);
        assert!(state.tail.is_none());
    }

    #[test]
    fn state_defaults() {
        let s = OutputState::default();
        assert_eq!(s.eq_gains.len(), 10);
        assert_eq!(s.source_rate, 44_100);
        assert!(!s.playing);
        assert_eq!(s.pos_frames, 0.0);
    }

    #[test]
    fn output_rate_prefers_native_soundcloud_rate_when_supported() {
        assert_eq!(preferred_output_sample_rate(44_100, 48_000), 44_100);
    }

    #[test]
    fn output_rate_uses_supported_fallback_when_44100_is_unavailable() {
        assert_eq!(preferred_output_sample_rate(48_000, 96_000), 48_000);
    }

    #[test]
    fn limiter_is_transparent_when_equalizer_is_disabled() {
        let mut limiter = Limiter::new(44_100.0, -12.0);

        let output = limit_if_needed(&mut limiter, false, 1.0, -1.0);

        assert_eq!(output, (1.0, -1.0));
    }

    #[test]
    fn output_config_prefers_stereo_over_multichannel_float() {
        assert!(
            output_config_rank(2, true, 48_000, 48_000)
                < output_config_rank(6, true, 48_000, 48_000)
        );
    }

    #[test]
    fn output_config_prefers_full_rate_surround_over_telephone_rate_stereo() {
        assert!(
            output_config_rank(6, true, 44_100, 44_100) < output_config_rank(2, true, 8_000, 8_000)
        );
    }

    #[test]
    fn output_rate_rank_rejects_telephone_quality_when_full_rate_exists() {
        assert!(output_rate_rank(44_100, 44_100) < output_rate_rank(8_000, 8_000));
        assert!(output_rate_rank(48_000, 48_000) < output_rate_rank(8_000, 8_000));
    }

    #[test]
    fn stereo_frame_zeros_unused_surround_channels() {
        let mut frame = [9.0; 6];

        write_stereo_frame(&mut frame, 0.25, -0.5);

        assert_eq!(frame, [0.25, -0.5, 0.0, 0.0, 0.0, 0.0]);
    }

    fn state_with(frames: &[(f32, f32)]) -> OutputState {
        let mut buffer = Vec::with_capacity(frames.len() * 2);
        for &(l, r) in frames {
            buffer.push(l);
            buffer.push(r);
        }
        OutputState {
            buffer,
            ..OutputState::default()
        }
    }

    #[test]
    fn resample_advances_one_frame_per_unit_ratio() {
        let mut s = state_with(&[(0.0, 10.0), (1.0, 11.0), (2.0, 12.0)]);
        assert_eq!(next_frame(&mut s, 1.0), (0.0, 10.0));
        assert_eq!(s.pos_frames, 1.0);
        assert_eq!(next_frame(&mut s, 1.0), (1.0, 11.0));
        assert_eq!(s.pos_frames, 2.0);
    }

    #[test]
    fn resample_uneven_ratio_never_swaps_channels() {
        // 44.1k -> 48k: left stays ~0..2, right stays ~10..12 the whole way.
        let mut s = state_with(&[
            (0.0, 10.0),
            (1.0, 11.0),
            (2.0, 12.0),
            (3.0, 13.0),
            (4.0, 14.0),
        ]);
        for _ in 0..4 {
            let (l, r) = next_frame(&mut s, 44_100.0 / 48_000.0);
            assert!((0.0..=4.0).contains(&l), "left leaked into right: {l}");
            assert!((10.0..=14.0).contains(&r), "right leaked into left: {r}");
        }
    }

    #[test]
    fn resample_starved_tail_is_silence_without_advancing() {
        let mut s = state_with(&[(0.5, -0.5)]);
        assert_eq!(next_frame(&mut s, 1.0), (0.0, 0.0));
        assert_eq!(s.pos_frames, 0.0);
    }

    #[test]
    fn compact_drops_consumed_prefix_and_keeps_phase() {
        let mut s = state_with(&[(0.0, 0.0), (1.0, 1.0), (2.0, 2.0), (3.0, 3.0), (4.0, 4.0)]);
        s.pos_frames = 2.25;
        maybe_compact(&mut s);
        assert_eq!(s.buffer, vec![2.0, 2.0, 3.0, 3.0, 4.0, 4.0]);
        assert!((s.pos_frames - 0.25).abs() < 1e-9);
        // Interpolation still reads the kept frames.
        let (l, _) = next_frame(&mut s, 1.0);
        assert!((l - 2.25).abs() < 1e-5);
    }

    /// Compacting must not move the *track* position: the two frames it
    /// dropped are remembered, so `absolute_frames` keeps counting up.
    /// Getting this wrong froze the progress bar the moment the first HLS
    /// segment was drained.
    #[test]
    fn the_reported_position_survives_compaction() {
        let mut s = state_with(&[(0.0, 0.0), (1.0, 1.0), (2.0, 2.0), (3.0, 3.0), (4.0, 4.0)]);
        s.pos_frames = 3.0;
        assert_eq!(s.absolute_frames(), 3);
        maybe_compact(&mut s);
        assert_eq!(s.consumed_frames, 3);
        assert_eq!(s.pos_frames, 0.0);
        assert_eq!(s.absolute_frames(), 3, "the track position moved");
        // …and it keeps going forward from there.
        next_frame(&mut s, 1.0);
        assert_eq!(s.absolute_frames(), 4);
    }

    /// A track resumed mid-way reports where it really is, not zero.
    #[test]
    fn a_resumed_track_reports_its_offset() {
        let mut s = state_with(&[(0.0, 0.0), (1.0, 1.0)]);
        // Same arithmetic `start_track` does for a 30 s offset at 44.1 kHz.
        s.consumed_frames = 30 * 44_100;
        assert_eq!(
            s.absolute_frames() * 1000 / u64::from(s.source_rate),
            30_000
        );
    }

    /// Balance attenuates the far channel and leaves the near one alone, so
    /// centre is exactly the volume and nothing is ever boosted into clipping.
    #[test]
    fn balance_only_attenuates() {
        let mut s = OutputState {
            volume: 1.0,
            ..OutputState::default()
        };
        assert_eq!(s.channel_gain(), (1.0, 1.0), "centre is unity");
        s.balance = -1.0;
        assert_eq!(s.channel_gain(), (1.0, 0.0), "hard left silences right");
        s.balance = 1.0;
        assert_eq!(s.channel_gain(), (0.0, 1.0), "hard right silences left");
        s.balance = 0.5;
        let (left, right) = s.channel_gain();
        assert_eq!((left, right), (0.5, 1.0));
        // Volume still scales both, and out-of-range balance clamps.
        s.volume = 0.5;
        s.balance = -4.0;
        assert_eq!(s.channel_gain(), (0.25, 0.0));
        for gain in [s.channel_gain().0, s.channel_gain().1] {
            assert!((0.0..=1.0).contains(&gain), "gain {gain} can clip");
        }
    }

    #[test]
    fn low_volume_is_quiet_and_zero_is_silent() {
        let mut s = OutputState::default();
        s.volume = 0.1;
        let (left, right) = s.channel_gain();
        assert!((left - 0.01).abs() < f32::EPSILON);
        assert!((right - 0.01).abs() < f32::EPSILON);
        s.volume = 0.0;
        assert_eq!(s.channel_gain(), (0.0, 0.0));
    }

    #[test]
    fn mono_folds_channels_without_changing_level() {
        let mut s = OutputState::default();
        assert_eq!(s.channel_mode(0.75, -0.25), (0.75, -0.25));
        s.mono = true;
        assert_eq!(s.channel_mode(0.75, -0.25), (0.25, 0.25));
    }
}
