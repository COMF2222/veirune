use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsRecord {
    pub id: u64,
    pub track_name: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub duration: Option<f64>,
    pub plain_lyrics: Option<String>,
    pub synced_lyrics: Option<String>,
    #[serde(default)]
    pub instrumental: bool,
    #[serde(default = "lrclib_source")]
    pub source: String,
    #[serde(default)]
    pub source_url: Option<String>,
}

fn lrclib_source() -> String {
    "LRCLIB".into()
}

pub(super) fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(6))
        .user_agent("Veirune/0.2 (https://github.com/COMF2222/veirune)")
        .build()
        .map_err(|_| "Could not start lyrics search".into())
}

async fn send_lrclib(
    request: impl Fn() -> reqwest::RequestBuilder,
) -> Result<reqwest::Response, String> {
    for attempt in 0..2 {
        let response = request()
            .send()
            .await
            .map_err(|_| "LRCLIB connection failed")?;
        if matches!(
            response.status(),
            reqwest::StatusCode::SERVICE_UNAVAILABLE | reqwest::StatusCode::TOO_MANY_REQUESTS
        ) && attempt == 0
        {
            let seconds = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(1)
                .clamp(1, 3);
            tokio::time::sleep(Duration::from_secs(seconds)).await;
            continue;
        }
        return Ok(response);
    }
    Err("LRCLIB unavailable".into())
}

pub(super) fn metadata_text(value: &str) -> String {
    let decoded = if value.contains('&') {
        // Decode character references without treating literal title characters
        // such as <3 as HTML elements.
        Html::parse_fragment(&value.replace('<', "&lt;"))
            .root_element()
            .text()
            .collect::<String>()
    } else {
        value.to_owned()
    };
    decoded
        .nfkc()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn comparable(value: &str) -> String {
    let normalized = metadata_text(value);
    let mut key = String::new();
    for ch in normalized.chars().flat_map(char::to_lowercase) {
        match ch {
            'ё' => key.push('е'),
            // Fold Latin accents without losing meaningful letters such as й
            // in Cyrillic names. NFKC also handles full-width/stylized letters.
            '\u{00c0}'..='\u{024f}' => {
                key.extend(ch.to_string().nfd().filter(|ch| ch.is_alphanumeric()))
            }
            _ if ch.is_alphanumeric() => key.push(ch),
            _ => {}
        }
    }
    key
}

fn feature_marker(value: &str) -> Option<(usize, usize)> {
    let lower = value.to_ascii_lowercase();
    [
        " feat. ",
        " feat ",
        " ft. ",
        " ft ",
        " featuring ",
        " w/ ",
        " (feat. ",
        " (feat ",
        " (ft. ",
        " (ft ",
        " (featuring ",
        " (w/ ",
    ]
    .iter()
    .filter_map(|marker| {
        lower
            .find(marker)
            .map(|start| (start, start + marker.len()))
    })
    .filter(|(start, end)| !value[..*start].trim().is_empty() && !value[*end..].trim().is_empty())
    .min_by_key(|(start, _)| *start)
}

fn feature_label(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    ["feat. ", "feat ", "ft. ", "ft ", "featuring ", "w/ "]
        .iter()
        .any(|marker| {
            lower
                .strip_prefix(marker)
                .is_some_and(|tail| !tail.trim().is_empty())
        })
}

fn bilingual_alias(value: &str) -> Option<(&str, &str)> {
    if let Some((name, alias)) = value.split_once('(')
        && let Some(alias) = alias.strip_suffix(')')
    {
        let scripts = |part: &str| {
            (
                part.chars().any(|ch| matches!(ch, '\u{0400}'..='\u{052f}')),
                part.chars().any(|ch| ch.is_ascii_alphabetic()),
            )
        };
        // Accept an explicit name in another script, not a different recording
        // or a translation/tribute annotation.
        let label = alias.to_lowercase();
        let annotation = promotional(alias)
            || !version_tags(alias).is_empty()
            || label.split(|ch: char| !ch.is_alphanumeric()).any(|word| {
                matches!(
                    word,
                    "tribute"
                        | "translation"
                        | "translations"
                        | "romanization"
                        | "romanizations"
                        | "fan"
                        | "трибьют"
                        | "перевод"
                        | "кавер"
                )
            });
        if !annotation
            && matches!(
                (scripts(name), scripts(alias)),
                ((true, false), (false, true)) | ((false, true), (true, false))
            )
        {
            return Some((name.trim(), alias.trim()));
        }
    }
    None
}

// SoundCloud display names can end in genre hashtags. Keep the original
// identity too; only remove complete trailing labels with a nonempty name.
fn without_artist_hashtags(value: &str) -> &str {
    let mut name = value.trim();
    while let Some((prefix, tag)) = name.rsplit_once(' ') {
        let Some(label) = tag.strip_prefix('#') else {
            break;
        };
        if prefix.trim().is_empty()
            || !label.starts_with(char::is_alphabetic)
            || !label
                .chars()
                .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '-'))
        {
            break;
        }
        name = prefix.trim_end();
    }
    name
}

// A compact "+guest" suffix is a common upload credit. Leave the exact title
// intact for timed lyrics and only try the base title for plain-text sources.
// Spaced additions ("+ reverb"), arithmetic and version labels stay untouched.
fn without_compact_guest(value: &str) -> Option<&str> {
    let (title, guest) = value.rsplit_once(" +")?;
    (!title.trim().is_empty()
        && guest.starts_with(char::is_alphabetic)
        && guest
            .chars()
            .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '-' | '\''))
        && version_tags(guest).is_empty())
        .then(|| title.trim_end())
}

fn artist_keys(value: &str) -> Vec<String> {
    let value = metadata_text(value);
    let mut keys = vec![comparable(&value)];
    if let Some((start, _)) = feature_marker(&value) {
        keys.push(comparable(&value[..start]));
    }
    let name = without_artist_hashtags(&value);
    keys.push(comparable(name));
    for variant in [value.as_str(), name] {
        if let Some((name, alias)) = bilingual_alias(variant) {
            keys.extend([comparable(name), comparable(alias)]);
        }
    }
    keys.retain(|key| !key.is_empty());
    keys
}

pub(super) fn same_artist(left: &str, right: &str) -> bool {
    if let (Some((_, left_end)), Some((_, right_end))) =
        (feature_marker(left), feature_marker(right))
        && comparable(&left[left_end..]) != comparable(&right[right_end..])
    {
        return false;
    }
    let right = artist_keys(right);
    artist_keys(left).iter().any(|key| right.contains(key))
}

fn promotional(value: &str) -> bool {
    let value = value.trim().to_lowercase();
    [
        "official",
        "audio",
        "video",
        "lyrics",
        "lyric video",
        "visualizer",
        "visualiser",
        "music video",
        "hq",
        "hd",
        "remaster",
    ]
    .iter()
    .any(|prefix| {
        value == *prefix
            || value
                .strip_prefix(prefix)
                .is_some_and(|tail| tail.starts_with(' '))
    }) || value.starts_with("prod.")
        || value.starts_with("prod ")
}

fn clean_title(value: &str) -> String {
    let value = metadata_text(value);
    let mut output = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        let close = match ch {
            '(' => ')',
            '[' => ']',
            '{' => '}',
            _ => {
                output.push(ch);
                continue;
            }
        };
        let mut part = String::new();
        let mut ended = false;
        for next in chars.by_ref() {
            if next == close {
                ended = true;
                break;
            }
            part.push(next);
        }
        if !ended
            || (!promotional(&part) && !feature_label(&part))
            || !version_tags(&part).is_empty()
        {
            output.push(ch);
            output.push_str(&part);
            if ended {
                output.push(close);
            }
        }
    }
    let mut output = output.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some((start, _)) = feature_marker(&output)
        && version_tags(&output) == version_tags(&output[..start])
    {
        output.truncate(start);
    }
    for suffix in [
        "official audio",
        "official video",
        "official music video",
        "lyric video",
    ] {
        if output.to_lowercase().ends_with(suffix) {
            output.truncate(output.len() - suffix.len());
            output = output
                .trim_end_matches([' ', '-', '–', '—', '|'])
                .to_owned();
        }
    }
    output.trim().to_owned()
}

pub(super) fn version_tags(title: &str) -> Vec<String> {
    let lower = metadata_text(title).to_lowercase();
    let mut tags = lower
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| {
            matches!(
                *word,
                "remix"
                    | "slowed"
                    | "sped"
                    | "nightcore"
                    | "live"
                    | "acoustic"
                    | "cover"
                    | "instrumental"
                    | "karaoke"
                    | "edit"
                    | "extended"
            )
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    tags.sort();
    tags.dedup();
    tags
}

pub(super) fn candidate_pairs(artist: &str, title: &str) -> Vec<(String, String)> {
    let artist = metadata_text(artist);
    let artist = artist.as_str();
    let title = clean_title(title);
    let known_artist = comparable(artist);
    let separators = [" - ", " – ", " — ", " | ", "-", "–", "—", "|"];
    let mut candidates = Vec::new();
    // Uploads may put the credited artist on either side of the song name.
    // Use that credit before guessing an artist from an uploader's title.
    if !known_artist.is_empty() {
        for separator in separators {
            for (index, _) in title.match_indices(separator) {
                let prefix = title[..index].trim();
                let suffix = title[index + separator.len()..].trim();
                if prefix.is_empty() || suffix.is_empty() {
                    continue;
                }
                if comparable(prefix) == known_artist {
                    candidates.push((artist, suffix));
                }
                if comparable(suffix) == known_artist {
                    candidates.push((artist, prefix));
                }
            }
        }
    }
    if candidates.is_empty() && !known_artist.is_empty() {
        for (index, _) in title.char_indices().filter(|(_, ch)| ch.is_whitespace()) {
            let prefix = title[..index].trim();
            let suffix = title[index..].trim();
            if !prefix.is_empty() && !suffix.is_empty() {
                if comparable(prefix) == known_artist {
                    candidates.push((artist, suffix));
                }
                if comparable(suffix) == known_artist {
                    candidates.push((artist, prefix));
                }
            }
        }
    }
    if candidates.is_empty() {
        for separator in separators {
            if let Some((prefix, song)) = title.split_once(separator) {
                if !prefix.trim().is_empty()
                    && !song.trim().is_empty()
                    && prefix.chars().count() < 70
                {
                    candidates.push((prefix.trim(), song.trim()));
                    break;
                }
            }
        }
    }
    // Also retain the complete title: a separator can be part of a song name.
    if !artist.is_empty() && !title.is_empty() {
        candidates.push((artist, title.as_str()));
    }
    let mut pairs = Vec::new();
    for (performer, song) in candidates {
        if !pairs.iter().any(|pair: &(String, String)| {
            comparable(&pair.0) == comparable(performer) && comparable(&pair.1) == comparable(song)
        }) {
            pairs.push((performer.to_owned(), song.to_owned()));
        }
        let name = without_artist_hashtags(performer);
        if name != performer
            && !pairs.iter().any(|pair| {
                comparable(&pair.0) == comparable(name) && comparable(&pair.1) == comparable(song)
            })
        {
            pairs.push((name.to_owned(), song.to_owned()));
        }
        if let Some((start, _)) = feature_marker(performer) {
            let primary = performer[..start].trim();
            if !pairs.iter().any(|pair| {
                comparable(&pair.0) == comparable(primary)
                    && comparable(&pair.1) == comparable(song)
            }) {
                pairs.push((primary.to_owned(), song.to_owned()));
            }
        }
    }
    pairs
}

fn duration_difference(record: &LyricsRecord, duration_ms: u64) -> Option<f64> {
    record
        .duration
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .filter(|_| duration_ms > 1000)
        .map(|seconds| (seconds * 1000.0 - duration_ms as f64).abs())
}

fn match_score(
    record: &LyricsRecord,
    artist: &str,
    title: &str,
    duration_ms: u64,
    album: Option<&str>,
) -> u32 {
    let wanted_title = comparable(&clean_title(title));
    let wanted_artist = comparable(artist);
    if wanted_title.is_empty()
        || wanted_artist.is_empty()
        || comparable(&clean_title(&record.track_name)) != wanted_title
        || !same_artist(&record.artist_name, artist)
        || version_tags(&record.track_name) != version_tags(title)
    {
        return 0;
    }
    let difference = duration_difference(record, duration_ms);
    if difference.is_some_and(|value| value > 20_000.0) {
        return 0;
    }
    100 + difference.map_or(0, |value| {
        if value <= 3000.0 {
            20
        } else if value <= 8000.0 {
            10
        } else {
            0
        }
    }) + u32::from(album.is_some_and(|wanted| {
        record
            .album_name
            .as_deref()
            .is_some_and(|found| comparable(found) == comparable(wanted))
    })) * 5
}

fn usable(record: &LyricsRecord) -> bool {
    record.instrumental
        || record
            .plain_lyrics
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        || record
            .synced_lyrics
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn plain_from_lrc(value: &str) -> Option<String> {
    let mut lines = Vec::new();
    for row in value.lines() {
        let mut text = row.trim();
        let mut timed = false;
        while let Some(stamp) = text.strip_prefix('[').and_then(|rest| rest.split_once(']')) {
            let Some((minutes, seconds)) = stamp.0.split_once(':') else {
                break;
            };
            let (seconds, fraction) = seconds
                .split_once(['.', ':'])
                .map_or((seconds, None), |(seconds, fraction)| {
                    (seconds, Some(fraction))
                });
            if !(1..=3).contains(&minutes.len())
                || !minutes.bytes().all(|ch| ch.is_ascii_digit())
                || seconds.len() != 2
                || !seconds.bytes().all(|ch| ch.is_ascii_digit())
                || fraction.is_some_and(|part| {
                    !(1..=3).contains(&part.len()) || !part.bytes().all(|ch| ch.is_ascii_digit())
                })
            {
                break;
            }
            timed = true;
            text = stamp.1.trim_start();
        }
        // Ignore LRC metadata and untimed headers; retain actual timed words.
        if timed && !text.is_empty() {
            lines.push(text);
        }
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn select_record(
    records: Vec<LyricsRecord>,
    artist: &str,
    title: &str,
    duration_ms: u64,
    album: Option<&str>,
) -> Option<LyricsRecord> {
    records
        .into_iter()
        .map(|mut record| {
            // Reject timestamps for a different edit before ranking candidates.
            if !duration_difference(&record, duration_ms).is_some_and(|value| value <= 3000.0) {
                if record
                    .plain_lyrics
                    .as_deref()
                    .is_none_or(|text| text.trim().is_empty())
                {
                    record.plain_lyrics = record.synced_lyrics.as_deref().and_then(plain_from_lrc);
                }
                record.synced_lyrics = None;
            }
            record
        })
        .filter(usable)
        .map(|entry| {
            (
                match_score(&entry, artist, title, duration_ms, album),
                entry,
            )
        })
        .filter(|(score, _)| *score > 0)
        .max_by_key(|(score, entry)| (*score, entry.synced_lyrics.is_some()))
        .map(|(_, entry)| entry)
}

fn title_record(records: Vec<LyricsRecord>, title: &str, duration_ms: u64) -> Option<LyricsRecord> {
    let title = clean_title(title);
    // A title alone is weak evidence. Require a distinctive multi-word name,
    // a matching recording length, and a single performer among all matches.
    if title.split_whitespace().count() < 2 || comparable(&title).chars().count() < 8 {
        return None;
    }
    let matches: Vec<_> = records
        .into_iter()
        .filter(|record| {
            comparable(&clean_title(&record.track_name)) == comparable(&title)
                && version_tags(&record.track_name) == version_tags(&title)
                && !comparable(&record.artist_name).is_empty()
                && duration_difference(record, duration_ms)
                    .is_some_and(|difference| difference <= 3000.0)
        })
        .collect();
    let artist = matches.first()?.artist_name.clone();
    if matches
        .iter()
        .any(|record| !same_artist(&record.artist_name, &artist))
    {
        return None;
    }
    // Identity remains useful for another provider even if LRCLIB has metadata
    // but no actual lyrics. Prefer a usable record when available.
    select_record(matches.clone(), &artist, &title, duration_ms, None)
        .or_else(|| matches.into_iter().next())
}

async fn search_title(
    client: &reqwest::Client,
    title: &str,
    duration_ms: u64,
) -> Result<Option<LyricsRecord>, String> {
    let response = send_lrclib(|| {
        client
            .get("https://lrclib.net/api/search")
            .query(&[("track_name", title)])
    })
    .await?;
    if !response.status().is_success() {
        return Err(format!("LRCLIB HTTP {}", response.status()));
    }
    let records = response
        .json::<Vec<LyricsRecord>>()
        .await
        .map_err(|_| "Invalid LRCLIB response")?;
    Ok(title_record(records, title, duration_ms))
}

async fn exact(
    client: &reqwest::Client,
    artist: &str,
    title: &str,
    duration_ms: u64,
    album: Option<&str>,
) -> Result<Option<LyricsRecord>, String> {
    let seconds = (duration_ms / 1000).to_string();
    let response = send_lrclib(|| {
        client.get("https://lrclib.net/api/get").query(&[
            ("artist_name", artist),
            ("track_name", title),
            ("duration", seconds.as_str()),
            ("album_name", album.unwrap_or("")),
        ])
    })
    .await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!("LRCLIB HTTP {}", response.status()));
    }
    let record = response
        .json::<LyricsRecord>()
        .await
        .map_err(|_| "Invalid LRCLIB response")?;
    Ok(select_record(
        vec![record],
        artist,
        title,
        duration_ms,
        album,
    ))
}

async fn search_structured(
    client: &reqwest::Client,
    artist: &str,
    title: &str,
    duration_ms: u64,
    album: Option<&str>,
) -> Result<Option<LyricsRecord>, String> {
    let response = send_lrclib(|| {
        client
            .get("https://lrclib.net/api/search")
            .query(&[("artist_name", artist), ("track_name", title)])
    })
    .await?;
    if !response.status().is_success() {
        return Err(format!("LRCLIB HTTP {}", response.status()));
    }
    let entries = response
        .json::<Vec<LyricsRecord>>()
        .await
        .map_err(|_| "Invalid LRCLIB response")?;
    Ok(select_record(entries, artist, title, duration_ms, album))
}

static MB_REQUEST: tokio::sync::Mutex<Option<tokio::time::Instant>> =
    tokio::sync::Mutex::const_new(None);

fn valid_isrc(raw: &str) -> Option<String> {
    let value = raw.replace(['-', ' '], "").to_uppercase();
    let bytes = value.as_bytes();
    (bytes.len() == 12
        && bytes[..2].iter().all(u8::is_ascii_alphabetic)
        && bytes[2..5].iter().all(u8::is_ascii_alphanumeric)
        && bytes[5..].iter().all(u8::is_ascii_digit))
    .then_some(value)
}

fn recording_identity(data: &Value, title: &str, duration_ms: u64) -> Option<(String, String)> {
    let mut identities = Vec::new();
    for recording in data.get("recordings")?.as_array()?.iter().take(25) {
        let Some(song) = recording.get("title").and_then(Value::as_str) else {
            continue;
        };
        let Some(credits) = recording.get("artist-credit").and_then(Value::as_array) else {
            continue;
        };
        let artist = credits
            .iter()
            .filter_map(|credit| {
                credit
                    .get("name")
                    .or_else(|| credit.get("artist")?.get("name"))
                    .and_then(Value::as_str)
            })
            .collect::<Vec<_>>()
            .join(" & ");
        if artist.is_empty() || version_tags(song) != version_tags(title) {
            continue;
        }
        if duration_ms > 1000
            && recording
                .get("length")
                .and_then(Value::as_u64)
                .is_some_and(|length| length.abs_diff(duration_ms) > 8000)
        {
            continue;
        }
        if !identities.iter().any(|(a, t): &(String, String)| {
            comparable(a) == comparable(&artist) && comparable(t) == comparable(song)
        }) {
            identities.push((artist, song.to_owned()));
        }
    }
    // A reused ISRC must not silently choose between different recordings.
    (identities.len() == 1).then(|| identities.remove(0))
}

async fn identify_isrc(
    client: &reqwest::Client,
    isrc: &str,
    title: &str,
    duration_ms: u64,
) -> Option<(String, String)> {
    let code = valid_isrc(isrc)?;
    {
        let mut last = MB_REQUEST.lock().await;
        if let Some(previous) = *last {
            tokio::time::sleep_until(previous + Duration::from_secs(1)).await;
        }
        *last = Some(tokio::time::Instant::now());
    }
    let response = client
        .get(format!("https://musicbrainz.org/ws/2/isrc/{code}"))
        .query(&[("fmt", "json"), ("inc", "artist-credits")])
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    recording_identity(&response.json::<Value>().await.ok()?, title, duration_ms)
}

async fn lyrics_ovh(
    client: &reqwest::Client,
    artist: &str,
    title: &str,
) -> Result<Option<LyricsRecord>, String> {
    let mut url =
        reqwest::Url::parse("https://api.lyrics.ovh/v1/").map_err(|_| "Invalid lyrics URL")?;
    url.path_segments_mut()
        .map_err(|_| "Invalid lyrics URL")?
        .push(artist)
        .push(title);
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "lyrics.ovh connection failed")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!("lyrics.ovh HTTP {}", response.status()));
    }
    let data = response
        .json::<Value>()
        .await
        .map_err(|_| "Invalid lyrics.ovh response")?;
    let Some(plain) = data
        .get("lyrics")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
    else {
        return Ok(None);
    };
    Ok(Some(LyricsRecord {
        id: 0,
        track_name: title.into(),
        artist_name: artist.into(),
        album_name: None,
        duration: None,
        plain_lyrics: Some(plain.into()),
        synced_lyrics: None,
        instrumental: false,
        source: "lyrics.ovh".into(),
        source_url: Some("https://lyrics.ovh/".into()),
    }))
}

fn append_lyrics(element: ElementRef<'_>, output: &mut String) {
    let value = element.value();
    if value.attr("data-exclude-from-selection") == Some("true")
        || value
            .attr("class")
            .is_some_and(|class| class.contains("LyricsHeader"))
        || matches!(value.name(), "script" | "style")
    {
        return;
    }
    if value.name() == "br" {
        output.push('\n');
        return;
    }
    for child in element.children() {
        if let scraper::Node::Text(text) = child.value() {
            output.push_str(&text.text);
        } else if let Some(child) = ElementRef::wrap(child) {
            append_lyrics(child, output);
        }
    }
}

fn genius_text(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    let mut output = String::new();
    for format in ["[data-lyrics-container=\"true\"]", "div.lyrics"] {
        let selector = Selector::parse(format).ok()?;
        for container in document.select(&selector) {
            if !output.is_empty() {
                output.push('\n');
            }
            append_lyrics(container, &mut output);
        }
        if !output.trim().is_empty() {
            break;
        }
    }
    let output = output.trim();
    (!output.is_empty() && output.len() <= 64_000).then(|| output.to_owned())
}

struct GeniusCandidate {
    id: u64,
    title: String,
    artist: String,
    url: reqwest::Url,
}

pub(super) fn genius_title_keys(value: &str) -> Vec<String> {
    let value = clean_title(value);
    // Genius can append an editorial * after a bilingual title. Keep this
    // convention local to Genius; LRCLIB recording/timing checks stay exact.
    let value = value.trim_end_matches(['*', ' ']);
    let mut keys = vec![comparable(value)];
    if let Some(title) = without_compact_guest(value) {
        keys.push(comparable(title));
    }
    if let Some((name, alias)) = bilingual_alias(value) {
        keys.extend([comparable(name), comparable(alias)]);
    }
    keys.retain(|key| !key.is_empty());
    keys
}

fn genius_artist_matches(song: &Value, wanted: &str) -> bool {
    let primary = song
        .pointer("/primary_artist/name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if same_artist(primary, wanted) {
        return true;
    }
    let mut credits = vec![primary];
    for key in ["primary_artists", "featured_artists"] {
        if let Some(artists) = song.get(key).and_then(Value::as_array) {
            credits.extend(
                artists
                    .iter()
                    .filter_map(|artist| artist.get("name").and_then(Value::as_str)),
            );
        }
    }
    let wanted = metadata_text(wanted);
    for separator in [" & ", " x ", ", ", " and "] {
        let parts: Vec<_> = wanted.split(separator).map(str::trim).collect();
        if parts.len() > 1
            && parts.iter().all(|part| {
                !part.is_empty() && credits.iter().any(|credit| same_artist(credit, part))
            })
        {
            return true;
        }
    }
    credits.iter().any(|credit| same_artist(credit, &wanted))
}

fn genius_metadata_matches(song: &Value, artist: &str, title: &str) -> bool {
    let found_title = song
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let found_keys = genius_title_keys(found_title);
    let matches_title = |wanted: &str| {
        version_tags(found_title) == version_tags(wanted)
            && genius_title_keys(wanted)
                .iter()
                .any(|key| found_keys.contains(key))
    };
    if genius_artist_matches(song, artist) && matches_title(title) {
        return true;
    }
    let Some(credit) = song.pointer("/primary_artist/name").and_then(Value::as_str) else {
        return false;
    };
    // An uploader is not necessarily the performer. Accept a credit embedded
    // at either end of the upload title only after Genius corroborates both
    // that exact performer and the remaining song name.
    candidate_pairs(credit, title)
        .iter()
        .any(|(performer, name)| {
            comparable(name) != comparable(&clean_title(title))
                && genius_artist_matches(song, performer)
                && matches_title(name)
        })
}

// Complete JSON traversal before awaiting a page request. Only owned candidates
// cross await points, keeping the command future compatible with Tauri's Send bound.
fn genius_candidates(data: &Value, artist: &str, title: &str) -> Vec<GeniusCandidate> {
    let mut candidates = Vec::new();
    let mut hits: Vec<&Value> = Vec::new();
    if let Some(sections) = data.pointer("/response/sections").and_then(Value::as_array) {
        hits.extend(
            sections
                .iter()
                .filter(|section| section.get("type").and_then(Value::as_str) == Some("song"))
                .filter_map(|section| section.get("hits").and_then(Value::as_array))
                .flatten(),
        );
    }
    if let Some(direct) = data.pointer("/response/hits").and_then(Value::as_array) {
        hits.extend(
            direct
                .iter()
                .filter(|hit| hit.get("type").and_then(Value::as_str) == Some("song")),
        );
    }
    for hit in hits.into_iter().take(20) {
        let Some(song) = hit.get("result") else {
            continue;
        };
        let found_title = song
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let found_artist = song
            .pointer("/primary_artist/name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !genius_metadata_matches(song, artist, title)
            || song.get("lyrics_state").and_then(Value::as_str) != Some("complete")
            || song.get("instrumental").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let Some(url) = song
            .get("url")
            .and_then(Value::as_str)
            .and_then(|url| reqwest::Url::parse(url).ok())
        else {
            continue;
        };
        if url.scheme() != "https" || url.host_str() != Some("genius.com") {
            continue;
        }
        if candidates
            .iter()
            .any(|candidate: &GeniusCandidate| candidate.url == url)
        {
            continue;
        }
        candidates.push(GeniusCandidate {
            id: song.get("id").and_then(Value::as_u64).unwrap_or(0),
            title: found_title.into(),
            artist: found_artist.into(),
            url,
        });
    }
    candidates
}

async fn genius_search_candidates(
    client: &reqwest::Client,
    artist: &str,
    title: &str,
    query: &str,
) -> Result<Vec<GeniusCandidate>, String> {
    let response = client
        .get("https://genius.com/api/search/song")
        .header(reqwest::header::USER_AGENT, GENIUS_USER_AGENT)
        .query(&[("q", query)])
        .send()
        .await
        .map_err(|_| "Genius connection failed")?;
    if !response.status().is_success() {
        return Err(format!("Genius HTTP {}", response.status()));
    }
    Ok(genius_candidates(
        &response
            .json::<Value>()
            .await
            .map_err(|_| "Invalid Genius response")?,
        artist,
        title,
    ))
}

async fn genius(
    client: &reqwest::Client,
    artist: &str,
    title: &str,
) -> Result<Option<LyricsRecord>, String> {
    // Compact separators such as "Song-ARTIST" confuse Genius's search.
    // They are search punctuation; matching still validates the original name.
    let query = metadata_text(&format!("{artist} {title}")).replace(['-', '–', '—', '|'], " ");
    let mut candidates = genius_search_candidates(client, artist, title, &query).await?;
    if candidates.is_empty() {
        let title_query = metadata_text(title).replace(['-', '–', '—', '|'], " ");
        if title_query != query {
            // Avoid letting a reuploader's nickname hide an explicit credit.
            // The same strict metadata matcher also validates this search.
            candidates = genius_search_candidates(client, artist, title, &title_query).await?;
        }
    }
    let mut failed = false;
    for candidate in candidates.into_iter().take(3) {
        let response = client
            .get(candidate.url.clone())
            .header(reqwest::header::USER_AGENT, GENIUS_USER_AGENT)
            .send()
            .await;
        let Ok(mut response) = response else {
            failed = true;
            continue;
        };
        if !response.status().is_success() {
            failed = true;
            continue;
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Genius page unavailable")?
        {
            if bytes.len() + chunk.len() > 2_000_000 {
                return Err("Genius page is too large".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let text = String::from_utf8(bytes).map_err(|_| "Invalid Genius page")?;
        if let Some(plain) = genius_text(&text) {
            return Ok(Some(LyricsRecord {
                id: candidate.id,
                track_name: candidate.title,
                artist_name: candidate.artist,
                album_name: None,
                duration: None,
                plain_lyrics: Some(plain),
                synced_lyrics: None,
                instrumental: false,
                source: "Genius".into(),
                source_url: Some(candidate.url.to_string()),
            }));
        }
    }
    if failed {
        Err("Genius page unavailable".into())
    } else {
        Ok(None)
    }
}

pub(super) const GENIUS_USER_AGENT: &str =
    "Mozilla/5.0 (compatible; Veirune/0.2; +https://github.com/COMF2222/veirune)";

fn plain_fallback_pairs(pairs: &[(String, String)]) -> Vec<(String, String)> {
    let mut result = Vec::new();
    for (artist, title) in pairs.iter().take(3) {
        let mut plain_title = title.clone();
        for _ in 0..4 {
            let span = plain_title.char_indices().find_map(|(start, ch)| {
                let close = match ch {
                    '(' => ')',
                    '[' => ']',
                    _ => return None,
                };
                let offset = plain_title[start + 1..].find(close)?;
                let end = start + 1 + offset;
                matches!(
                    comparable(&plain_title[start + 1..end]).as_str(),
                    "rockversion"
                        | "рокверсия"
                        | "slowed"
                        | "slowedreverb"
                        | "slowedandreverb"
                        | "spedup"
                        | "speedup"
                        | "nightcore"
                        | "nightcoreversion"
                )
                .then_some(start..end + 1)
            });
            let Some(span) = span else { break };
            plain_title.replace_range(span, " ");
        }
        for suffix in [
            " - slowed + reverb",
            " - slowed",
            " - sped up",
            " - nightcore",
            " - rock version",
        ] {
            if plain_title.to_ascii_lowercase().ends_with(suffix) {
                plain_title.truncate(plain_title.len() - suffix.len());
                break;
            }
        }
        let plain_title = clean_title(&plain_title);
        let plain_title = without_compact_guest(&plain_title)
            .unwrap_or(&plain_title)
            .to_owned();
        if comparable(&plain_title) != comparable(title) && !plain_title.is_empty() {
            result.push((artist.clone(), plain_title));
        }
        result.push((artist.clone(), title.clone()));
    }
    result.dedup();
    result.truncate(4);
    result
}

async fn lrclib_lookup(
    client: &reqwest::Client,
    pairs: &[(String, String)],
    duration_ms: u64,
    album: Option<&str>,
) -> Result<Option<LyricsRecord>, String> {
    let mut answered = false;
    for (performer, song) in pairs.iter().take(3) {
        match exact(client, performer, song, duration_ms, album).await {
            Ok(Some(record)) => return Ok(Some(record)),
            Ok(None) => answered = true,
            Err(_) => {}
        }
        // A different artist spelling or an uploader's name must not prevent
        // discovering the recording by its exact title and duration.
        let (structured, by_title) = tokio::join!(
            search_structured(client, performer, song, duration_ms, album),
            search_title(client, song, duration_ms),
        );
        for result in [structured, by_title] {
            match result {
                Ok(Some(record)) => return Ok(Some(record)),
                Ok(None) => answered = true,
                Err(_) => {}
            }
        }
    }
    if answered {
        Ok(None)
    } else {
        Err("LRCLIB unavailable".into())
    }
}

async fn fallback_sources(
    client: &reqwest::Client,
    pairs: &[(String, String)],
) -> Result<Option<LyricsRecord>, String> {
    let mut requests = tokio::task::JoinSet::new();
    let plain_pairs = plain_fallback_pairs(pairs);
    for (performer, song) in &plain_pairs {
        let (client, performer, song) = (client.clone(), performer.clone(), song.clone());
        requests.spawn(async move { genius(&client, &performer, &song).await });
    }
    for (performer, song) in &plain_pairs {
        if !pairs.contains(&(performer.clone(), song.clone())) {
            // Explicit speed/arrangement labels can share words with the base
            // recording. Passing no duration deliberately removes all timing.
            let (client, performer, song) = (client.clone(), performer.clone(), song.clone());
            requests
                .spawn(async move { search_structured(&client, &performer, &song, 0, None).await });
        }
        // This provider cannot distinguish recording versions.
        if version_tags(song).is_empty() {
            let (client, performer, song) = (client.clone(), performer.clone(), song.clone());
            requests.spawn(async move { lyrics_ovh(&client, &performer, &song).await });
        }
    }
    let mut answered = false;
    let mut failed = false;
    while let Some(result) = requests.join_next().await {
        match result {
            Ok(Ok(Some(record))) => return Ok(Some(record)),
            Ok(Ok(None)) => answered = true,
            _ => failed = true,
        }
    }
    if failed {
        Err("Some lyrics sources are temporarily unavailable".into())
    } else if answered {
        Ok(None)
    } else {
        Err("Lyrics sources are temporarily unavailable".into())
    }
}

async fn lookup_sources(
    artist: &str,
    title: &str,
    duration_ms: u64,
    album: Option<&str>,
    isrc: Option<&str>,
) -> Result<Option<LyricsRecord>, String> {
    let client = client()?;
    let mut pairs = candidate_pairs(artist, title);
    if let Some(code) = isrc {
        if let Some(identity) = identify_isrc(&client, code, title, duration_ms).await {
            pairs.insert(0, identity);
        }
    }
    if pairs.is_empty() {
        return Ok(None);
    }
    // Reserve time for the remaining services even if LRCLIB is slow. Start
    // fallback requests together so lyrics.ovh cannot starve Genius of time.
    let primary = tokio::time::timeout(
        Duration::from_secs(14),
        lrclib_lookup(&client, &pairs, duration_ms, album),
    )
    .await;
    let mut answered = matches!(primary, Ok(Ok(None)));
    if let Ok(Ok(Some(record))) = primary {
        if usable(&record) {
            return Ok(Some(record));
        }
        answered = true;
        pairs.insert(0, (record.artist_name, record.track_name));
    }
    match fallback_sources(&client, &pairs).await {
        Ok(Some(record)) => Ok(Some(record)),
        Ok(None) if answered => Ok(None),
        Ok(None) => Err("LRCLIB is temporarily unavailable".into()),
        Err(error) => Err(error),
    }
}

pub async fn lookup(
    artist: String,
    title: String,
    duration_ms: u64,
    album: Option<String>,
    isrc: Option<String>,
) -> Result<Option<LyricsRecord>, String> {
    let result = tokio::time::timeout(
        Duration::from_secs(40),
        lookup_sources(
            &artist,
            &title,
            duration_ms,
            album.as_deref(),
            isrc.as_deref(),
        ),
    )
    .await
    .map_err(|_| "Lyrics search timed out; try again")?;
    if let Ok(Some(record)) = &result { crate::lyrics_search::remember(record); }
    result
}

pub async fn search(query: String) -> Result<Vec<LyricsRecord>, String> {
    let query = query.trim();
    if query.chars().count() < 2 {
        return Ok(Vec::new());
    }
    let client = client()?;
    let response = send_lrclib(|| {
        client
            .get("https://lrclib.net/api/search")
            .query(&[("q", query)])
    })
    .await?;
    if !response.status().is_success() {
        return Err(format!("LRCLIB HTTP {}", response.status()));
    }
    let mut entries = response
        .json::<Vec<LyricsRecord>>()
        .await
        .map_err(|_| "Invalid LRCLIB response")?;
    entries.retain(usable);
    entries.truncate(20);
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_future_satisfies_the_tauri_send_bound() {
        fn assert_send<T: Send>(_: T) {}
        // Constructing the future does not poll it or contact any service.
        assert_send(lookup("Artist".into(), "Song".into(), 180_000, None, None));
    }

    #[test]
    fn unicode_names_match_without_collapsing_distinct_cyrillic_letters() {
        assert_eq!(comparable("Beyonce\u{301}"), comparable("Beyoncé"));
        assert_eq!(comparable("Beyonce"), comparable("Beyoncé"));
        assert_eq!(comparable("𝗟𝗶𝗹 𝗣𝗲𝗲𝗽"), comparable("Lil Peep"));
        assert_eq!(comparable("Семён"), comparable("Семен"));
        assert_ne!(comparable("Май"), comparable("Маи"));
        assert_eq!(metadata_text("ＷＡＲＬＯＲＤ"), "WARLORD");
    }

    #[test]
    fn feature_credits_do_not_hide_the_main_title_or_artist() {
        assert_eq!(
            clean_title("lovely (feat. Khalid) [Official Audio]"),
            "lovely"
        );
        assert_eq!(clean_title("Song ft. Guest"), "Song");
        assert_eq!(clean_title("Song (feat. Guest) (Remix)"), "Song (Remix)");
        let found = candidate_pairs("Post Malone ft. Swae Lee", "Sunflower");
        assert!(found.contains(&("Post Malone".into(), "Sunflower".into())));
        assert!(same_artist("Post Malone", "Post Malone ft. Swae Lee"));
        assert!(!same_artist(
            "Post Malone ft. First",
            "Post Malone ft. Second"
        ));
        assert!(!same_artist(
            "Post Malone Tribute",
            "Post Malone ft. Swae Lee"
        ));
    }

    #[test]
    fn feature_cleanup_keeps_recording_version_checks() {
        assert_eq!(
            match_score(
                &record("Song (Remix)", "Artist", 180.0),
                "Artist ft. Guest",
                "Song (feat. Guest)",
                180_000,
                None
            ),
            0
        );
        assert_eq!(
            clean_title("Song feat. Guest (Remix)"),
            "Song feat. Guest (Remix)"
        );
    }

    #[test]
    fn joint_genius_credits_must_include_every_requested_artist() {
        let song = serde_json::json!({"primary_artist":{"name":"Main"},
            "primary_artists":[{"name":"Main"}], "featured_artists":[{"name":"Guest"}]});
        assert!(genius_artist_matches(&song, "Main & Guest"));
        assert!(genius_artist_matches(&song, "Guest, Main"));
        assert!(!genius_artist_matches(&song, "Main & Unrelated"));
        assert!(!genius_artist_matches(&song, "Earth, Wind & Fire"));
        let band = serde_json::json!({"primary_artist":{"name":"Earth, Wind & Fire"}});
        assert!(genius_artist_matches(&band, "Earth, Wind & Fire"));
        assert!(!genius_artist_matches(&band, "Fire"));
    }

    #[test]
    fn white_wine_upload_matches_explicit_joint_credits() {
        let title = "white wine w/ lil tracy (prod. nedarb)";
        assert_eq!(clean_title(title), "white wine");
        assert!(candidate_pairs("Lil Peep", title).contains(&("Lil Peep".into(), "white wine".into())));
        let song = serde_json::json!({"title":"White Wine", "primary_artist":{"name":"Lil Peep & Lil Tracy"},
            "primary_artists":[{"name":"Lil Peep"},{"name":"Lil Tracy"}]});
        assert!(genius_metadata_matches(&song, "Lil Peep", title));
        assert!(genius_metadata_matches(&song, "Lil Tracy", "White Wine"));
        assert!(!genius_metadata_matches(&song, "Unrelated", title));
        assert!(!genius_metadata_matches(&song, "Lil Peep", "White Wine w/ Lil Tracy (Remix)"));
        assert_eq!(clean_title("Song (w/ Guest) (Remix)"), "Song (Remix)");
        assert_eq!(clean_title("Dance with Me"), "Dance with Me");
    }

    #[tokio::test]
    #[ignore = "contacts public lyrics services"]
    async fn reported_white_wine_upload_finds_synced_lyrics() {
        let record = lookup("Lil Peep".into(), "white wine w/ lil tracy (prod. nedarb)".into(), 156_000, None, None)
            .await.unwrap().expect("White Wine lyrics");
        assert_eq!(comparable(&record.track_name), "whitewine");
        assert!(record.synced_lyrics.as_deref().is_some_and(|text| !text.is_empty()));
    }

    #[test]
    fn direct_genius_hits_and_legacy_pages_are_supported() {
        let data = serde_json::json!({"response":{"hits":[{"type":"song","result":{
            "id":1,"title":"Song","primary_artist":{"name":"Artist"},
            "lyrics_state":"complete","url":"https://genius.com/artist-song-lyrics"
        }}]}});
        assert_eq!(genius_candidates(&data, "Artist", "Song").len(), 1);
        assert_eq!(
            genius_text("<div class=\"lyrics\"><p>First<br>Second</p></div>").as_deref(),
            Some("First\nSecond")
        );
        assert!(genius_text("<html>Access denied</html>").is_none());
    }

    #[test]
    fn speed_edits_can_use_plain_base_lyrics_without_reusing_timestamps() {
        let pairs = vec![("Artist".into(), "Song (slowed + reverb)".into())];
        let base = plain_fallback_pairs(&pairs);
        assert_eq!(base[0], ("Artist".into(), "Song".into()));
        let found = select_record(
            vec![record("Song", "Artist", 180.0)],
            &base[0].0,
            &base[0].1,
            0,
            None,
        )
        .unwrap();
        assert!(found.plain_lyrics.is_some());
        assert!(found.synced_lyrics.is_none());
        for title in ["Song (Remix)", "Song (Cover)", "Song (Live)"] {
            let original = vec![("Artist".into(), title.into())];
            assert_eq!(plain_fallback_pairs(&original), original);
        }
    }

    #[tokio::test]
    #[ignore = "contacts public lyrics services"]
    async fn diverse_metadata_finds_live_lyrics() {
        let samples = [
            (
                "Reuploads",
                "Lil Peep - Star Shopping (Official Audio)",
                141_000,
                "Lil Peep",
            ),
            (
                "Billie Eilish feat. Khalid",
                "lovely (feat. Khalid)",
                200_000,
                "Billie Eilish",
            ),
            (
                "Post Malone ft. Swae Lee",
                "Sunflower (Official Audio)",
                158_000,
                "Post Malone",
            ),
            ("Beyonce", "Halo", 264_000, "Beyoncé"),
            ("THE WEEKND", "Blinding Lights [HD]", 202_000, "The Weeknd"),
            ("𝗠𝘂𝘀𝗲", "Ｕｐｒｉｓｉｎｇ", 305_000, "Muse"),
        ];
        for (artist, title, duration, expected_artist) in samples {
            let found = lookup(artist.into(), title.into(), duration, None, None)
                .await
                .unwrap_or_else(|error| panic!("{artist} / {title}: {error}"))
                .unwrap_or_else(|| panic!("{artist} / {title}: no lyrics"));
            assert!(usable(&found) && !found.instrumental, "{artist} / {title}");
            assert!(
                same_artist(&found.artist_name, expected_artist),
                "{artist} / {title}"
            );
            println!(
                "{artist} / {title}: {} (timed={})",
                found.source,
                found.synced_lyrics.is_some()
            );
        }
    }

    #[test]
    fn genius_candidates_own_metadata_and_reject_unrelated_results() {
        let candidates = {
            let hit = |id, title, artist, url| {
                serde_json::json!({"result": {
                    "id":id,"title":title,"primary_artist":{"name":artist},
                    "url":url,"lyrics_state":"complete","instrumental":false,
                }})
            };
            let data = serde_json::json!({"response":{"sections":[
                {"type":"artist","hits":[hit(1,"Song","Artist","https://genius.com/artist")]},
                {"type":"song","hits":[
                    hit(2,"Song","Artist","https://genius.com/artist-song-lyrics"),
                    hit(3,"Song","Another artist","https://genius.com/another-song-lyrics"),
                    hit(4,"Song (Remix)","Artist","https://genius.com/remix-lyrics"),
                    hit(5,"Song","Artist","https://example.com/lyrics")
                ]}
            ]}});
            genius_candidates(&data, "Artist", "Song")
        };
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, 2);
        assert_eq!(candidates[0].title, "Song");
        assert_eq!(candidates[0].artist, "Artist");
        assert_eq!(
            candidates[0].url.as_str(),
            "https://genius.com/artist-song-lyrics"
        );
    }
    #[test]
    fn bilingual_genius_titles_match_both_names_and_keep_identity_checks() {
        let hit = |id, title, artist| {
            serde_json::json!({"type":"song","result":{
                "id":id,"title":title,"primary_artist":{"name":artist},
                "url":format!("https://genius.com/test-{id}-lyrics"),
                "lyrics_state":"complete"
            }})
        };
        let data = serde_json::json!({"response":{"hits":[
            hit(10411513, "ты причина (You're The Reason)*", "CUPSIZE"),
            hit(2, "ты причина (You're The Reason)*", "Other Artist"),
            hit(3, "ты причина (You're The Reason Remix)*", "CUPSIZE"),
            hit(4, "ты причина (English Translation)*", "CUPSIZE"),
            hit(5, "ты причина (Live)*", "CUPSIZE"),
            hit(6, "ты причина (другая песня)*", "CUPSIZE")
        ]}});
        for title in ["ты причина&#x20;", "You're the Reason"] {
            let found = genius_candidates(&data, "&#x43;UPSIZE", title);
            assert_eq!(
                found.iter().map(|entry| entry.id).collect::<Vec<_>>(),
                vec![10411513]
            );
        }
        assert!(genius_candidates(&data, "CUPSIZE", "ты причина (Remix)").is_empty());
        assert!(genius_candidates(&data, "CUPSIZE", "другая песня").is_empty());
    }

    #[test]
    fn reported_meteli_matches_bilingual_credits_with_upload_annotations() {
        // Titles and artist names verified on the supplied SoundCloud/Genius pages.
        let song = serde_json::json!({
            "title": "метели (snowstorms)",
            "primary_artist": {"name": "империя эмо (empireofemo)"}
        });
        assert!(genius_metadata_matches(
            &song, "империя эмо #emo", "метели +голодный"
        ));
        assert!(candidate_pairs("империя эмо #emo", "метели +голодный")
            .contains(&("империя эмо".into(), "метели +голодный".into())));
        let pairs = candidate_pairs("империя эмо #emo", "метели +голодный");
        assert!(plain_fallback_pairs(&pairs)
            .contains(&("империя эмо".into(), "метели".into())));
        for (artist, title) in [
            ("Другой артист #emo", "метели +голодный"),
            ("империя эмо #emo", "другая песня +голодный"),
            ("империя эмо #emo", "метели +голодный (Remix)"),
        ] {
            assert!(!genius_metadata_matches(&song, artist, title));
        }
        // Guest shorthand must not relax timed recording identity.
        assert_eq!(clean_title("метели +голодный"), "метели +голодный");
        for title in ["Song + reverb", "Song +Live", "1+1", "Song +", "Song + 2"] {
            assert_eq!(genius_title_keys(title), vec![comparable(title)]);
        }
        assert!(!same_artist("Artist", "Artist #1"));
        assert_eq!(without_artist_hashtags("#Artist"), "#Artist");
        assert!(!same_artist("Artist", "Artist #tag Extra"));
        assert_eq!(without_artist_hashtags("Artist #"), "Artist #");
        assert_eq!(without_artist_hashtags("Artist #emo #rock"), "Artist");
        assert_eq!(
            match_score(&record("метели", "империя эмо", 120.0),
                "империя эмо #emo", "метели +голодный", 120_000, None),
            0
        );
    }

    #[tokio::test]
    #[ignore = "contacts the public Genius service"]
    async fn reported_meteli_finds_live_genius_lyrics() {
        let found = lookup("империя эмо #emo".into(), "метели +голодный".into(), 0, None, None)
            .await.unwrap().expect("the reported upload should find its Genius lyrics");
        assert_eq!(found.id, 12446975);
        assert_eq!(found.source_url.as_deref(),
            Some("https://genius.com/Empireofemo-snowstorms-lyrics"));
        assert!(found.plain_lyrics.as_deref().is_some_and(|text| !text.trim().is_empty()));
        assert!(found.synced_lyrics.is_none());
    }

    #[tokio::test]
    #[ignore = "contacts the public Genius service"]
    async fn reported_cupsize_reason_finds_live_genius_lyrics() {
        let found = genius(&client().unwrap(), "CUPSIZE", "ты причина")
            .await
            .unwrap()
            .expect("the bilingual Genius title should match the Russian title");
        assert_eq!(found.id, 10411513);
        assert_eq!(
            found.source_url.as_deref(),
            Some("https://genius.com/Cupsize-youre-the-reason-lyrics")
        );
        assert!(
            found
                .plain_lyrics
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
        );
        assert!(found.synced_lyrics.is_none());
    }

    #[test]
    fn genius_confirms_embedded_performer_instead_of_an_uploader_name() {
        let song = serde_json::json!({"title":"Хотите — верьте, или нет (Believe It or Not)",
            "primary_artist":{"name":"CUPSIZE"}});
        for title in [
            "ХОТИТЕ ВЕРЬТЕ ИЛИ НЕТ-CUPSIZE",
            "CUPSIZE-ХОТИТЕ ВЕРЬТЕ ИЛИ НЕТ",
        ] {
            assert!(genius_metadata_matches(
                &song,
                "психиатрическая больница&#x20;",
                title
            ));
        }
        for title in [
            "ХОТИТЕ ВЕРЬТЕ ИЛИ НЕТ-Other Artist",
            "Другая песня-CUPSIZE",
            "ХОТИТЕ ВЕРЬТЕ ИЛИ НЕТ (Remix)-CUPSIZE",
            "ХОТИТЕ ВЕРЬТЕ ИЛИ НЕТ",
        ] {
            assert!(!genius_metadata_matches(
                &song,
                "психиатрическая больница",
                title
            ));
        }
        let unrelated = serde_json::json!({"title":"Part Two","primary_artist":{"name":"Song"}});
        assert!(!genius_metadata_matches(
            &unrelated,
            "Artist",
            "Songbook - Part Two"
        ));
    }

    #[tokio::test]
    #[ignore = "contacts public lyrics services"]
    async fn reported_cupsize_believe_upload_finds_live_lyrics() {
        let found = lookup(
            "психиатрическая больница".into(),
            "ХОТИТЕ ВЕРЬТЕ ИЛИ НЕТ-CUPSIZE".into(),
            49_554,
            None,
            None,
        )
        .await
        .unwrap()
        .expect("the performer suffix should be confirmed by Genius");
        assert_eq!(found.source, "Genius");
        assert_eq!(found.id, 10007993);
        assert_eq!(
            found.source_url.as_deref(),
            Some("https://genius.com/Cupsize-believe-it-or-not-lyrics")
        );
        assert!(
            found
                .plain_lyrics
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
        );
        assert!(found.synced_lyrics.is_none());
    }

    fn record(title: &str, artist: &str, seconds: f64) -> LyricsRecord {
        LyricsRecord {
            id: 1,
            track_name: title.into(),
            artist_name: artist.into(),
            album_name: None,
            duration: Some(seconds),
            plain_lyrics: Some("Synthetic test text".into()),
            synced_lyrics: Some("[00:00.00]Synthetic test text".into()),
            instrumental: false,
            source: lrclib_source(),
            source_url: None,
        }
    }
    #[test]
    fn uploader_prefix_and_promotional_labels_are_removed() {
        assert_eq!(
            clean_title("we think too much (prod. nedarb)"),
            "we think too much"
        );
        assert_eq!(
            candidate_pairs("Uploader", "Lil Peep - Star Shopping (Official Audio)")[0],
            ("Lil Peep".into(), "Star Shopping".into())
        );
    }
    #[test]
    fn character_references_and_compact_upload_credits_are_normalized() {
        assert_eq!(
            candidate_pairs("&#x4A;ESUS", "Никому Неизвестный Голос&#x20;")[0],
            ("JESUS".into(), "Никому Неизвестный Голос".into())
        );
        assert_eq!(
            candidate_pairs("JESUS", "Никому Неизвестный Голос JESUS")[0],
            ("JESUS".into(), "Никому Неизвестный Голос".into())
        );
        assert_eq!(
            candidate_pairs("$KIMO888", "Джизус-WARLORD(rock version)")[0],
            ("Джизус".into(), "WARLORD(rock version)".into())
        );
        assert_eq!(
            metadata_text("Love <3 &amp; music&nbsp;"),
            "Love <3 & music"
        );
    }
    #[test]
    fn a_unique_title_and_matching_length_can_resolve_a_different_artist_credit() {
        let mut matching = record("Никому неизвестный голос", "Джизус", 144.0);
        matching.id = 12681071;
        let found = title_record(vec![matching], "Никому Неизвестный Голос", 144_235).unwrap();
        assert_eq!(found.id, 12681071);
        assert!(found.synced_lyrics.is_some());
    }
    #[test]
    fn title_discovery_rejects_ambiguous_artists_wrong_lengths_and_versions() {
        let title = "A distinctive song name";
        assert!(
            title_record(
                vec![
                    record(title, "First", 144.0),
                    record(title, "Second", 144.0)
                ],
                title,
                144_000
            )
            .is_none()
        );
        assert!(title_record(vec![record(title, "First", 180.0)], title, 144_000).is_none());
        assert!(
            title_record(
                vec![record(title, "First", 144.0)],
                "A distinctive song name (Remix)",
                144_000
            )
            .is_none()
        );
        assert!(title_record(vec![record(title, "First", 144.0)], title, 0).is_none());
        assert!(title_record(vec![record("Stay", "First", 144.0)], "Stay", 144_000).is_none());
    }
    #[test]
    fn title_discovery_can_supply_metadata_for_another_provider_without_lyrics() {
        let mut metadata = record("A distinctive song name", "Credited artist", 144.0);
        metadata.plain_lyrics = None;
        metadata.synced_lyrics = None;
        let found = title_record(vec![metadata], "A distinctive song name", 144_000).unwrap();
        assert_eq!(found.artist_name, "Credited artist");
        assert!(!usable(&found));
    }
    #[test]
    fn bilingual_genius_credits_match_but_unrelated_artists_and_translations_do_not() {
        let hit = |artist, title| {
            serde_json::json!({"result": {
                "id":1,"title":title,"primary_artist":{"name":artist},
                "url":"https://genius.com/Dzhizus-warlord-lyrics", "lyrics_state":"complete",
            }})
        };
        let data = serde_json::json!({"response":{"sections":[{"type":"song","hits":[
            hit("Джизус (Dzhizus)", "WARLORD"),
            hit("Genius English Translations", "Джизус (Dzhizus) - WARLORD (English Translation)"),
            hit("Джизус (Tribute)", "WARLORD"),
        ]}]}});
        assert_eq!(genius_candidates(&data, "Джизус", "WARLORD").len(), 1);
        assert_eq!(genius_candidates(&data, "Dzhizus", "WARLORD").len(), 1);
    }
    #[test]
    fn rock_arrangements_use_base_names_only_for_untimed_providers() {
        let pairs = candidate_pairs("$KIMO888", "Джизус-WARLORD(rock version)");
        assert_eq!(pairs[0].1, "WARLORD(rock version)");
        assert_eq!(
            plain_fallback_pairs(&pairs)[0],
            ("Джизус".into(), "WARLORD".into())
        );
        let remix = vec![("Artist".into(), "Song (Remix)".into())];
        assert_eq!(plain_fallback_pairs(&remix), remix);
        let unicode = vec![("Artist".into(), "İstanbul (ROCK VERSION)".into())];
        assert_eq!(plain_fallback_pairs(&unicode)[0].1, "İstanbul");
    }
    #[tokio::test]
    #[ignore = "contacts public lyrics services"]
    async fn reported_jesus_track_finds_live_lrclib_lyrics_with_cyrillic_credit() {
        let found = lookup(
            "JESUS".into(),
            "Никому Неизвестный Голос".into(),
            144_235,
            Some("TEEN SOUL".into()),
            None,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(found.source, "LRCLIB");
        assert_eq!(found.id, 12681071);
        assert!(found.synced_lyrics.is_some());
    }
    #[tokio::test]
    #[ignore = "contacts public lyrics services"]
    async fn reported_rock_upload_finds_live_genius_plain_lyrics() {
        let found = lookup(
            "$KIMO888".into(),
            "Джизус-WARLORD(rock version)".into(),
            122_279,
            None,
            None,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(comparable(&found.track_name), "warlord");
        assert!(same_artist(&found.artist_name, "Джизус"));
        assert!(
            found
                .plain_lyrics
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
        );
        assert!(found.synced_lyrics.is_none());
        // Any matching source may win the concurrent fallback. Also verify
        // the actual Genius page independently rather than requiring it to win.
        let found = genius(&client().unwrap(), "Джизус", "WARLORD")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.source, "Genius");
        assert_eq!(
            found.source_url.as_deref(),
            Some("https://genius.com/Dzhizus-warlord-lyrics")
        );
        assert!(
            found
                .plain_lyrics
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
        );
        assert!(found.synced_lyrics.is_none());
    }
    #[test]
    fn credited_artist_suffix_is_recognized_before_an_inferred_prefix() {
        for title in [
            "оригами - CUPSIZE",
            "оригами – CUPSIZE",
            "оригами — CUPSIZE",
            "оригами | CUPSIZE",
            "  оригами  -  Cupsize (Official Audio)  ",
        ] {
            assert_eq!(
                candidate_pairs(" cupsize ", title),
                vec![
                    ("cupsize".into(), "оригами".into()),
                    ("cupsize".into(), clean_title(title)),
                ],
                "{title}"
            );
        }
    }
    #[test]
    fn artist_and_song_names_can_contain_other_separators() {
        for title in [
            "Artist - Duo - Song - Part Two",
            "Song - Part Two - Artist - Duo",
        ] {
            assert_eq!(
                candidate_pairs("Artist - Duo", title)[0],
                ("Artist - Duo".into(), "Song - Part Two".into()),
                "{title}"
            );
        }
    }
    #[test]
    fn an_unknown_suffix_is_not_assumed_to_be_the_artist() {
        let pairs = candidate_pairs("Artist", "Song - Part Two");
        assert!(pairs.contains(&("Artist".into(), "Song - Part Two".into())));
        assert!(!pairs.contains(&("Part Two".into(), "Song".into())));
    }
    #[test]
    fn artist_suffix_keeps_recording_version_checks() {
        let (artist, title) =
            candidate_pairs("cupsize", "оригами (slowed + reverb) - CUPSIZE").remove(0);
        assert_eq!(title, "оригами (slowed + reverb)");
        assert_eq!(
            match_score(
                &record("оригами", "CUPSIZE", 144.0),
                &artist,
                &title,
                144_000,
                None
            ),
            0
        );
    }
    #[test]
    fn suffix_lookup_selects_the_matching_recording() {
        let mut matching = record("оригами", "CUPSIZE", 144.0);
        matching.id = 26462302;
        let records = vec![
            matching,
            record("оригами", "CUPSIZE", 30.0),
            record("оригами", "Another artist", 144.0),
            record("оригами (Remix)", "CUPSIZE", 144.0),
        ];
        let found = candidate_pairs("cupsize", "оригами - CUPSIZE")
            .iter()
            .find_map(|(artist, title)| {
                select_record(records.clone(), artist, title, 144_000, None)
            })
            .expect("the original recording should match");
        assert_eq!(found.id, 26462302);
        assert!(found.synced_lyrics.is_some());
    }
    #[tokio::test]
    #[ignore = "contacts the public LRCLIB service"]
    async fn artist_suffix_lookup_finds_live_lrclib_lyrics() {
        let found = lookup(
            "cupsize".into(),
            "оригами - CUPSIZE".into(),
            144_000,
            None,
            None,
        )
        .await
        .expect("lyrics lookup should succeed")
        .expect("LRCLIB should have this recording");
        assert_eq!(found.source, "LRCLIB");
        assert_eq!(comparable(&found.artist_name), "cupsize");
        assert_eq!(comparable(&found.track_name), "оригами");
        assert!(
            found
                .synced_lyrics
                .as_deref()
                .is_some_and(|lyrics| !lyrics.trim().is_empty())
        );
    }
    #[test]
    fn related_titles_and_artists_are_not_the_same_song() {
        assert_eq!(
            match_score(
                &record("Love Me Again", "Artist", 180.0),
                "Artist",
                "Love Me",
                180_000,
                None
            ),
            0
        );
        assert_eq!(
            match_score(
                &record("Song", "Artist Tribute", 180.0),
                "Artist",
                "Song",
                180_000,
                None
            ),
            0
        );
    }
    #[test]
    fn remixes_and_speed_changes_are_preserved() {
        assert_eq!(
            clean_title("Song (Official Remix)"),
            "Song (Official Remix)"
        );
        assert_eq!(
            clean_title("Song (slowed + reverb) [Official Audio]"),
            "Song (slowed + reverb)"
        );
        assert_eq!(
            match_score(
                &record("Song", "Artist", 180.0),
                "Artist",
                "Song (Remix)",
                180_000,
                None
            ),
            0
        );
    }
    #[test]
    fn wrong_duration_is_rejected_and_other_edit_timestamps_are_removed() {
        assert!(
            select_record(
                vec![record("Song", "Artist", 260.0)],
                "Artist",
                "Song",
                180_000,
                None
            )
            .is_none()
        );
        let found = select_record(
            vec![record("Song", "Artist", 190.0)],
            "Artist",
            "Song",
            180_000,
            None,
        )
        .unwrap();
        assert!(found.synced_lyrics.is_none());
        assert!(found.plain_lyrics.is_some());
    }
    #[test]
    fn matching_duration_wins_over_another_album() {
        let found = select_record(
            vec![
                record("Song", "Artist", 180.0),
                record("Song", "Artist", 190.0),
            ],
            "Artist",
            "Song",
            180_000,
            None,
        )
        .unwrap();
        assert!(found.synced_lyrics.is_some());
    }
    #[test]
    fn synced_only_records_keep_words_when_timing_is_unsuitable() {
        let mut timed = record("Song", "Artist", 190.0);
        timed.plain_lyrics = None;
        timed.synced_lyrics = Some(
            "[ar:Artist]\n[ti:Song]\n[00:01.25][00:05.50]First\n[00:07:125]Second\n[00:09] [Chorus]\n[00:11.00]\n[not:a timestamp]Discard".into(),
        );
        let found = select_record(vec![timed.clone()], "Artist", "Song", 180_000, None).unwrap();
        assert_eq!(
            found.plain_lyrics.as_deref(),
            Some("First\nSecond\n[Chorus]")
        );
        assert!(found.synced_lyrics.is_none());
        assert!(select_record(vec![timed.clone()], "Artist", "Song", 100_000, None).is_none());
        assert!(select_record(vec![timed], "Other Artist", "Song", 180_000, None).is_none());
        assert!(plain_from_lrc("[ar:Artist]\n[ti:Song]").is_none());
    }
    #[test]
    fn isrc_validation_and_reused_identifier() {
        assert!(valid_isrc("bad/code").is_none());
        assert_eq!(
            valid_isrc("GB-AYE-92-00070").as_deref(),
            Some("GBAYE9200070")
        );
        let data = serde_json::json!({"recordings":[{"title":"One","length":180000,"artist-credit":[{"name":"Artist"}]},{"title":"Two","length":180000,"artist-credit":[{"name":"Artist"}]}]});
        assert!(recording_identity(&data, "upload", 180_000).is_none());
    }
    #[test]
    fn isrc_fixes_an_arbitrary_uploader_title() {
        let data = serde_json::json!({"recordings":[{"title":"Song","length":180000,"artist-credit":[{"name":"Artist"}]}]});
        assert_eq!(
            recording_identity(&data, "upload 42", 180_000),
            Some(("Artist".into(), "Song".into()))
        );
    }
    #[test]
    fn genius_preserves_lines_and_entities_and_skips_controls() {
        let html = r#"<div data-lyrics-container="true"><div class="LyricsHeader_test">Header</div>[Verse]<br>One &amp; <a href="/">two</a><br><span data-exclude-from-selection="true">Controls</span>Three</div><div data-lyrics-container="true">Four<br>Five</div>"#;
        assert_eq!(
            genius_text(html).as_deref(),
            Some("[Verse]\nOne & two\nThree\nFour\nFive")
        );
        assert!(genius_text("<html><body>Verify your browser</body></html>").is_none());
    }
}
