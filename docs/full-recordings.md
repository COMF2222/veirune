# Full playable recording alternatives

## Licensed web playback (desktop 0.4.30 / Android 0.1.11)

The Tauri player and Android Media3 player can request `/v1/media/web-resolve`
when a public recording has no full official API stream. Known previews try
this before the slower alternate-upload search. The broker verifies the current
listener and official track metadata, then resolves the same public recording
from SoundCloud's website. Private, blocked, SNIP-only web recordings and
incomplete media playlists are rejected. An unavailable track is skipped;
there is no new per-track retry loop and no preview presented as a full song.

Only broker-relative, four-hour playback capabilities reach the renderer.
Windows uses Hls.js and the existing WebView2 CDM in the main window; Android
uses Widevine in its existing MediaSessionService. macOS selects the CBC
rendition and SoundCloud's FairPlay certificate/SPC/CKC envelope. It still needs
real Mac playback verification; a Windows WebView2 result does not prove
WKWebView/FairPlay compatibility. No external or hidden playback window is
created. The standalone test harness is outside the product and is not bundled.

All audio and opaque CDM messages go through the configured broker. Content
keys are never extracted or persisted. Encrypted segments share the existing
bounded cache across listeners; license requests are never cached. After actual
playback starts, one bounded background worker scans that artist profile's
public preview tracks, paginating their catalog and warming media segments.
Jobs are deduplicated for 24 hours, queue at most four artists, stop after twenty
minutes or one quarter of cache capacity (at most 2 GiB), and check access
revocation throughout. Artist URNs work even when the official API omits IDs.
Existing cache size/free-space limits and download concurrency remain in force.

The original queue identity, lyrics, feedback and listening statistics remain.
Pause, seeking, speed, repeat, A–B looping and sleep controls drive the same
native queue. The desktop CDM route does not pass through Rust PCM processing:
EQ, normalization, the visualizer and crossfade are not implemented for that
route. Ordinary tracks retain native audio processing. Protected streams are
not offered as reusable offline files on Android.

Verification: 127.025-second Wanted! source resolved through the new broker,
both CTR/CBC renditions fetched, FairPlay certificate fetched, encrypted cache
hits verified. The production renderer hook passed WebView2 pause/resume,
seek beyond 40 seconds, source replacement and cleanup in an isolated harness.
Rust queue/lifecycle and broker authorization/cache tests also pass. The Android
APK is built/tested/linted; Honor playback and Mac playback remain device checks.

## Compatible alternate uploads

When SoundCloud exposes only a preview, playback resolves the original track
through `/v1/media/resolve`. The backend checks the source and searches the
**official API** for public full recordings with matching artist/title or ISRC,
duration and version markers. A remix, cover or excerpt is not an acceptable
substitute. A source without enough metadata can remain a preview.

The backend saves the source → selected recording URNs in its media SQLite DB.
Saved matches survive restarts and are shared across listeners. Metadata and
permissions are checked with the current listener's token before every use.
The full recording uses the existing bounded server HLS segment cache; listeners
and direct plays of the same upload reuse its audio rather than duplicate it.
Tokens and signed audio URLs are never saved in the correspondence table.

The client keeps the original library track, likes, queue identity and lyrics.
The HLS playlist supplies the actual playback duration, preview badge and seek
range. This also applies when preparing the next track for crossfade. A first
preview lookup has a longer timeout; ordinary tracks retain their normal timeout.
Older servers continue to work. If no verified full stream is available, the
recording is skipped rather than playing an unrelated song or an excerpt.

Spotify and Yandex Music imports search only `access=playable` tracks and reject
snippet/non-streamable matches. A public uploader can qualify when the credited
artist appears in the title and duration/version checks also agree.

Deploy the backend changes before distributing the new client. The checked
backend deploy script runs the existing migrations and backups; the media-cache
correspondence table initializes automatically. No additional secrets or paid
recognition service are required. Actual catalog coverage depends on available
public recordings and reliable metadata, not a promised 100% match rate.

Current API search rows can identify recordings using a track URN without a
numeric id. Matching validates either form and rejects conflicting identifiers.
A SNIP/PREVIEW policy also triggers full-recording lookup when the access field
is missing or stale; blocked or private sources remain excluded.
