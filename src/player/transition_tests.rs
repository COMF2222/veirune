use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct AacFixture {
    player: Arc<Player>,
    _session: Arc<crate::auth::Session>,
    _directory: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
    restart_status: Arc<std::sync::atomic::AtomicU16>,
    resolves: Arc<std::sync::atomic::AtomicUsize>,
    lost_tickets: Arc<std::sync::atomic::AtomicUsize>,
}

impl Drop for AacFixture {
    fn drop(&mut self) {
        self.player.shutdown();
        self.server.abort();
    }
}

#[tokio::test]
async fn an_hls_preview_is_skipped_instead_of_playing_thirty_seconds() {
    let fixture = aac_fixture().await;
    let player = &fixture.player;
    let track = {
        let mut state = player.state.lock();
        state.queue[0].access = Some("preview".into());
        state.queue[0].full_duration_ms = Some(180000);
        state.queue[0].clone()
    };
    player.load_track(track, 0);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let ready = { let state=player.state.lock(); state.current==Some(1) && !state.loading };
            if ready { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    let state = player.state.lock();
    assert!(state.is_playing && state.error.is_none());
    assert!(!state.preview_fallback);
    assert!(state.sample_rate > 0);
    assert_eq!(fixture.resolves.load(Ordering::Acquire), 2);
}

async fn aac_fixture() -> AacFixture {
    aac_fixture_with_playlist(None).await
}

async fn aac_fixture_with_playlist(playlist: Option<String>) -> AacFixture {
    let playlist = playlist.unwrap_or_else(|| String::from_utf8_lossy(include_bytes!("fixtures/aac-transition/index.m3u8")).into_owned());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let client = Arc::new(crate::api::ApiClient::new(None, false));
    let session = crate::auth::Session::with_test_user(crate::auth::AppCredentials {
        client_id: "test-client".into(), client_secret: String::new(),
        redirect_uri: "http://127.0.0.1:41317/callback".into(), server_url: Some(origin),
    }, client.clone()).await;
    let directory = tempfile::tempdir().unwrap();
    let player = Arc::new(Player::new(Arc::new(AudioOutput::silent([0.0; 10], 1.0)),
        client, Arc::new(AudioCache::disabled()), directory.path().join("settings.json"),
        tokio::runtime::Handle::current()));
    player.attach();
    player.set_audio_preferences(false, 8000, false);
    {
        let mut state = player.state.lock();
        state.queue = crate::demo::demo_tracks().into_iter().take(2).collect();
        state.queue[0].duration_ms = Some(30000);
        state.queue[0].full_duration_ms = None;
        state.queue[1].duration_ms = Some(16000);
        state.queue[1].full_duration_ms = None;
        state.order = vec![0, 1]; state.current = Some(0);
        state.is_playing = true; state.duration_ms = 30000;
    }
    let restart_status = Arc::new(std::sync::atomic::AtomicU16::new(0));
    let resolves = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let lost_tickets = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let status = restart_status.clone();
    let resolved = resolves.clone();
    let lost = lost_tickets.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0; 4096];
                let count = socket.read(&mut chunk).await.unwrap();
                if count == 0 { break; }
                request.extend_from_slice(&chunk[..count]);
                if request.windows(4).any(|part| part == b"\r\n\r\n") { break; }
                assert!(request.len() < 16384);
            }
            let text = String::from_utf8_lossy(&request);
            // Prefetch cancellation can close a new connection before sending
            // an HTTP request. It is not a failure of the audio path.
            let Some(line) = text.lines().next() else { continue; };
            let path = line.split_whitespace().nth(1).unwrap();
            let restart = status.load(Ordering::Acquire);
            if restart > 0 && path.starts_with(&format!("/v1/media/{}/", "a".repeat(43))) {
                lost.fetch_add(1, Ordering::AcqRel);
                let header = format!("HTTP/1.1 {restart} Expired\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}");
                let _ = socket.write_all(header.as_bytes()).await;
                continue;
            }
            let resolve = format!(r#"{{"playlist_path":"/v1/media/{}/index.m3u8","bitrate_kbps":160}}"#, if restart == 0 { "a".repeat(43) } else { "b".repeat(43) });
            let body: &[u8] = match path {
                "/v1/media/resolve" => {
                    assert!(text.to_ascii_lowercase().contains("authorization: oauth test-user-token"));
                    resolved.fetch_add(1, Ordering::AcqRel);
                    resolve.as_bytes()
                }
                path if path.ends_with("/index.m3u8") => playlist.as_bytes(),
                path if path.ends_with("/init.mp4") => include_bytes!("fixtures/aac-transition/init.mp4"),
                path if path.ends_with("/segment-0.m4s") => include_bytes!("fixtures/aac-transition/segment-0.m4s"),
                path if path.ends_with("/segment-1.m4s") => include_bytes!("fixtures/aac-transition/segment-1.m4s"),
                path if path.ends_with("/segment-2.m4s") => include_bytes!("fixtures/aac-transition/segment-2.m4s"),
                path if path.ends_with("/segment-3.m4s") => include_bytes!("fixtures/aac-transition/segment-3.m4s"),
                path if path.ends_with("/segment-4.m4s") => include_bytes!("fixtures/aac-transition/segment-4.m4s"),
                _ => panic!("Unexpected fixture request: {path}"),
            };
            let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            if socket.write_all(header.as_bytes()).await.is_ok() {
                let _ = socket.write_all(body).await;
            }
        }
    });
    AacFixture { player, _session: session, _directory: directory, server, restart_status, resolves, lost_tickets }
}

#[tokio::test]
async fn aac_preparation_decodes_multiple_fragments_and_can_continue_after_commit() {
    let fixture = aac_fixture().await;
    let next = fixture.player.state.lock().queue[1].clone();
    let mut prepared = tokio::time::timeout(Duration::from_secs(5), fixture.player.prepare_next(next, 8000))
        .await.unwrap().expect("AAC preparation must retain the init header across HLS fragments");
    assert!(prepared.decoder.segments_fetched >= 3);
    assert!(prepared.samples.len() as u64 / 2 * 1000 / u64::from(prepared.rate) >= 11000);
    assert!(prepared.samples.chunks_exact(2).any(|frame| frame[1].abs() > 0.05));
    let mut continuation = Vec::new();
    while prepared.decoder.inner.next_packet(&mut continuation).unwrap() {}
    let bytes = prepared.decoder.prefetch.as_ref().unwrap().queue.lock().await.recv().await.unwrap().unwrap();
    prepared.decoder.feed(&bytes);
    while prepared.decoder.inner.next_packet(&mut continuation).unwrap() {}
    assert!(continuation.len() > prepared.rate as usize * 2);
    assert!(!fixture.server.is_finished(), "fixture server must not have panicked");
}

#[tokio::test]
async fn delayed_autoplay_batch_resumes_after_real_eof_but_not_after_manual_pause() {
    for paused in [false,true] {
        let fixture = aac_fixture().await;
        let player = &fixture.player;
        let next = player.state.lock().queue[1].clone();
        {
            let mut state = player.state.lock();
            state.queue.truncate(1); state.order = vec![0]; state.duration_ms = 1000;
        }
        player.decoder.lock().exhausted = true;
        player.output.start_track(vec![0.2;44100*2],44100,true,0);
        let runner = tokio::spawn(player.clone().run());
        player.output.render_test_audio(48000);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(player.state.lock().current,Some(0));
        if paused { player.pause(); }
        player.enqueue(vec![next],false);
        if paused {
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert_eq!(player.state.lock().current,Some(0));
            assert!(!player.state.lock().is_playing);
        } else {
            tokio::time::timeout(Duration::from_secs(5),async {
                loop {
                    let ready = { let state = player.state.lock(); state.current == Some(1) && !state.loading };
                    if ready { break; }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.expect("An asynchronously appended track must resume the exhausted queue");
            assert!(player.output.render_test_audio(4800).iter().any(|sample|sample.abs()>0.05));
        }
        player.shutdown();
        tokio::time::timeout(Duration::from_secs(2),runner).await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn player_loop_automatically_crossfades_into_real_aac_audio_for_eight_seconds() {
    let fixture = aac_fixture().await;
    let player = &fixture.player;
    let outgoing: Vec<f32> = (0..44100 * 12).flat_map(|_| [0.4, 0.0]).collect();
    player.output.start_track(outgoing, 44100, true, 18000);
    let runner = tokio::spawn(player.clone().run());
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if player.next_track.lock().as_ref().is_some_and(|next| next.prepared.is_some()) { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("Real HLS audio must be ready before the fade boundary");
    assert_eq!(player.state.lock().current, Some(0));
    player.output.render_test_audio(48000 * 4);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if player.state.lock().current == Some(1) { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("The player loop must start the fade automatically");

    let mixed = player.output.render_test_audio(48000 * 8);
    let channel_level = |start: usize, end: usize, channel: usize| {
        let sum = mixed[start * 2..end * 2].chunks_exact(2)
            .map(|frame| frame[channel].powi(2) as f64).sum::<f64>();
        (sum / (end - start) as f64).sqrt()
    };
    assert!(channel_level(0, 48000, 0) > 0.35, "outgoing track remains audible at the start");
    assert!(channel_level(48000 * 3, 48000 * 4, 0) > 0.19, "outgoing track remains audible halfway through");
    assert!(channel_level(48000 * 3, 48000 * 4, 1) > 0.05, "decoded AAC overlaps the outgoing track");
    assert!(channel_level(48000 * 7, 48000 * 8, 0) < 0.04, "outgoing track fades out");
    assert!(channel_level(48000 * 7, 48000 * 8, 1) > 0.12, "incoming AAC fades in");
    assert_eq!(player.state.lock().current, Some(1));
    assert!(player.output.position_ms() >= 7999);
    player.shutdown();
    tokio::time::timeout(Duration::from_secs(2), runner).await.unwrap().unwrap();
    assert!(!fixture.server.is_finished(), "fixture server must not have panicked");
}


#[tokio::test]
async fn a_lost_audio_link_skips_to_the_next_track_without_retrying_the_failed_track() {
    for status in [401, 410] {
        let fixture = aac_fixture().await;
        let player = &fixture.player;
        player.set_audio_preferences(false, 0, false);
        player.state.lock().queue[0].duration_ms = Some(16000);
        let track = player.state.lock().queue[0].clone();
        player.load_track(track.clone(), 4000);
        tokio::time::timeout(Duration::from_secs(5), async {
            while player.state.lock().loading {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        assert!(player.state.lock().error.is_none());
        fixture.restart_status.store(status, Ordering::Release);
        // Replace any successfully prefetched tail so this exercise cannot
        // depend on whether the fixture happened to download ahead quickly.
        {
            let mut slot = player.decoder.lock();
            slot.prefetch.take();
            let remaining = slot.playlist.as_ref().unwrap().segments[slot.segments_fetched..].to_vec();
            slot.prefetch = Some(player.hls.prefetch(slot.urn.clone(), remaining, &player.rt));
        }
        let runner = tokio::spawn(player.clone().run());
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let ready = fixture.resolves.load(Ordering::Acquire) >= 2 && !player.state.lock().loading;
                if ready { break; }
                player.output.render_test_audio(4800);
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        }).await.expect("A failed audio link must advance to the next track");
        assert!(fixture.lost_tickets.load(Ordering::Acquire) > 0);
        {
            let state = player.state.lock();
            assert_eq!(state.current, Some(1));
            assert_ne!(state.queue[state.current.unwrap()].id, track.id);
            assert!(state.is_playing && state.error.is_none(), "{:?}", state.error);
            assert_eq!(fixture.resolves.load(Ordering::Acquire), 2, "Each track gets one load attempt");
        }
        assert!(player.output.render_test_audio(4800).iter().any(|sample| sample.abs() > 0.05));
        assert!(!fixture.server.is_finished());
        player.shutdown();
        tokio::time::timeout(Duration::from_secs(2), runner).await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn full_recording_for_a_preview_keeps_library_identity_and_seeks_past_thirty_seconds() {
    let mut playlist = String::from("#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MAP:URI=\"init.mp4\"\n");
    for index in 0..12 { playlist.push_str(&format!("#EXTINF:4,\nsegment-{}.m4s\n", index % 4)); }
    playlist.push_str("#EXT-X-ENDLIST\n");
    let fixture = aac_fixture_with_playlist(Some(playlist)).await;
    let player = &fixture.player;
    player.set_audio_preferences(false, 0, false);
    let track = {
        let mut state = player.state.lock();
        state.queue[0].access = Some("preview".into());
        state.queue[0].duration_ms = Some(30000);
        state.queue[0].full_duration_ms = Some(48000);
        state.queue[0].clone()
    };
    player.load_track(track.clone(), 35000);
    tokio::time::timeout(Duration::from_secs(5), async {
        while player.state.lock().loading { tokio::time::sleep(Duration::from_millis(10)).await; }
    }).await.unwrap();
    {
        let state = player.state.lock();
        assert!(state.error.is_none(), "{:?}", state.error);
        assert!(!state.preview_fallback);
        assert_eq!(state.duration_ms, 48000);
        assert_eq!(state.queue[state.current.unwrap()].id, track.id);
        assert!(state.is_playing);
    }
    assert!(player.output.position_ms() >= 35000);
    assert!(player.output.render_test_audio(4800).iter().any(|sample| sample.abs() > 0.05));
    assert!(!fixture.server.is_finished());
}
