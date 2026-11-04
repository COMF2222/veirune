#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// SoundCloud track URN, e.g. `soundcloud:tracks:123456`.
pub type TrackUrn = String;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Artwork {
    #[serde(rename = "150x150")]
    pub t150: Option<String>,
    #[serde(rename = "500x500")]
    pub t500: Option<String>,
}

impl Artwork {
    pub fn best(&self) -> Option<&str> {
        self.t500.as_deref().or(self.t150.as_deref())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: u64,
    pub username: String,
    pub permalink: Option<String>,
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub full_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub city: Option<String>,
    #[serde(default)]
    pub country_code: Option<String>,
    #[serde(default)]
    pub permalink_url: Option<String>,
    #[serde(default)]
    pub followers_count: u64,
    #[serde(default)]
    pub followings_count: u64,
    #[serde(default)]
    pub track_count: u64,
    #[serde(default)]
    pub public_playlists_count: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: u64,
    pub title: String,
    /// Duration in milliseconds (API field `duration`).
    #[serde(default, rename = "duration")]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub full_duration_ms: Option<u64>,
    pub artwork: Option<Artwork>,
    /// Current SoundCloud API artwork field. Older saved data may still use
    /// the nested `artwork` object above, so both formats remain readable.
    #[serde(default)]
    pub artwork_url: Option<String>,
    /// JSON endpoint containing the real waveform samples rendered by
    /// SoundCloud. Older cached tracks simply fall back to generated bars.
    #[serde(default)]
    pub waveform_url: Option<String>,
    pub user: Option<UserLite>,
    /// Release attribution supplied by the rights holder. SoundCloud's web
    /// player shows this artist name ahead of the uploader account name.
    #[serde(default)]
    pub publisher_metadata: Option<PublisherMetadata>,
    /// Artist override in the current public API schema. Some responses also
    /// expose the richer `publisher_metadata.artist`; both mean the same
    /// credit and both must beat the uploader account name.
    #[serde(default)]
    pub metadata_artist: Option<String>,
    #[serde(default)]
    pub playback_count: Option<u64>,
    #[serde(default)]
    pub favoritings_count: Option<u64>,
    #[serde(default)]
    pub likes_count: Option<u64>,
    #[serde(default)]
    pub comment_count: Option<u64>,
    #[serde(default)]
    pub streamable: bool,
    #[serde(default)]
    pub downloadable: bool,
    #[serde(default)]
    pub preview_start_ms: Option<u64>,
    #[serde(default)]
    pub preview_end_ms: Option<u64>,
    #[serde(default)]
    pub genre: Option<String>,
    #[serde(default)]
    pub tag_list: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub permalink_url: Option<String>,
    /// Access policy: `ALLOW`, `SNIP` (30s preview) or `BLOCK` (no off-platform stream).
    #[serde(default)]
    pub policy: Option<String>,
    #[serde(default)]
    pub access: Option<String>,
    /// Monetization: e.g. `SUBSCRIPTION` marks Go+-exclusive tracks.
    #[serde(default)]
    pub monetization_model: Option<String>,
    /// UI-only activity metadata attached while flattening `/me/feed`.
    /// SoundCloud's feed entry owns this value, not the track itself.
    #[serde(default, skip_serializing)]
    pub feed_reposted: bool,
}

impl Track {
    pub fn artwork_url(&self) -> Option<&str> {
        self.artwork
            .as_ref()
            .and_then(Artwork::best)
            .or(self.artwork_url.as_deref())
            .filter(|url| !url.is_empty())
    }

    pub fn urn(&self) -> String {
        format!("soundcloud:tracks:{}", self.id)
    }
    pub fn artist(&self) -> &str {
        self.publisher_metadata
            .as_ref()
            .and_then(|metadata| metadata.artist.as_deref())
            .map(str::trim)
            .filter(|artist| !artist.is_empty())
            .or_else(|| {
                self.metadata_artist
                    .as_deref()
                    .map(str::trim)
                    .filter(|artist| !artist.is_empty())
            })
            .or_else(|| self.user.as_ref().map(|u| u.username.as_str()))
            .unwrap_or("")
    }
    pub fn effective_duration_ms(&self) -> u64 {
        self.full_duration_ms.or(self.duration_ms).unwrap_or(0)
    }
    /// Normalized access policy (`ALLOW` when the API omits it).
    pub fn policy_name(&self) -> &str {
        self.access
            .as_deref()
            .or(self.policy.as_deref())
            .unwrap_or("ALLOW")
    }
    /// Track is restricted by the rightsholder: no off-platform stream.
    pub fn is_blocked(&self) -> bool {
        matches!(
            self.policy_name().to_ascii_lowercase().as_str(),
            "block" | "blocked"
        )
    }
    /// Only a 30-second snippet is streamable.
    pub fn is_snippet(&self) -> bool {
        [self.access.as_deref(), self.policy.as_deref()].into_iter().flatten()
            .any(|value| matches!(value.to_ascii_lowercase().as_str(), "snip" | "preview"))
    }
    /// Go+-exclusive release (needs a Go+ subscription on the user account).
    pub fn is_go_only(&self) -> bool {
        self.monetization_model
            .as_deref()
            .is_some_and(|m| m.eq_ignore_ascii_case("SUBSCRIPTION"))
    }
    /// Short badge for the UI: `GO+`, `PREVIEW`, `BLOCKED` or `None`.
    pub fn access_badge(&self) -> Option<&'static str> {
        if self.is_blocked() {
            Some("BLOCKED")
        } else if self.is_snippet() {
            Some("PREVIEW")
        } else if self.is_go_only() {
            Some("GO+")
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PublisherMetadata {
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub album_title: Option<String>,
    /// Stable recording identifier, when supplied by the rights holder.
    #[serde(default)]
    pub isrc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserLite {
    pub id: u64,
    pub username: String,
    pub permalink: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebProfile {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub service: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Storefront {
    #[serde(default)]
    pub track_urn: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub link: String,
    #[serde(default)]
    pub link_title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub image_url: Option<String>,
    #[serde(default)]
    pub price: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub id: u64,
    pub title: String,
    pub artwork: Option<Artwork>,
    /// Current SoundCloud API artwork field; `artwork` keeps compatibility
    /// with the older nested representation persisted by Fastcloud.
    #[serde(default)]
    pub artwork_url: Option<String>,
    pub user: Option<UserLite>,
    #[serde(default)]
    pub track_count: Option<u64>,
    #[serde(default)]
    pub genre: Option<String>,
    #[serde(default)]
    pub tag_list: Option<String>,
    #[serde(default)]
    pub likes_count: Option<u64>,
    #[serde(default, alias = "duration")]
    pub duration_ms: Option<u64>,
    #[serde(default, deserialize_with = "null_default")]
    pub is_album: bool,
    /// Newer responses may identify albums by type instead of setting the
    /// legacy boolean. Keep both so the Albums tab follows SoundCloud.
    #[serde(default)]
    pub playlist_type: Option<String>,
    #[serde(default)]
    pub set_type: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub permalink_url: Option<String>,
    /// Embedded when `show_tracks=true`; used immediately for cover fallback
    /// while the dedicated playlist-track page is still loading.
    #[serde(default, deserialize_with = "playlist_tracks")]
    pub tracks: Vec<Track>,
    /// UI-only activity metadata attached while flattening `/me/feed`.
    #[serde(default, skip_serializing)]
    pub feed_reposted: bool,
}

fn null_default<'de, D: serde::Deserializer<'de>, T: Deserialize<'de> + Default>(
    deserializer: D,
) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

fn playlist_tracks<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<Track>, D::Error> {
    let rows = Option::<Vec<serde_json::Value>>::deserialize(deserializer)?.unwrap_or_default();
    // List responses may contain only track IDs or null entries. Their cover
    // fallback is optional, so one partial embedded track must not hide the
    // entire playlist collection.
    Ok(rows
        .into_iter()
        .filter_map(|row| serde_json::from_value(row).ok())
        .collect())
}

impl Playlist {
    pub fn artwork_url(&self) -> Option<&str> {
        self.artwork
            .as_ref()
            .and_then(Artwork::best)
            .or(self.artwork_url.as_deref())
            .or_else(|| self.tracks.iter().find_map(Track::artwork_url))
            .filter(|url| !url.is_empty())
    }

    pub fn urn(&self) -> String {
        format!("soundcloud:playlists:{}", self.id)
    }

    pub fn is_album(&self) -> bool {
        self.is_album
            || self
                .playlist_type
                .as_deref()
                .is_some_and(|kind| kind.eq_ignore_ascii_case("album"))
            || self
                .set_type
                .as_deref()
                .is_some_and(|kind| kind.eq_ignore_ascii_case("album"))
    }
}

#[cfg(test)]
mod track_and_playlist_tests {
    use super::*;

    #[test]
    fn release_artist_wins_over_uploader() {
        let track: Track = serde_json::from_str(
            r#"{
                "id": 69,
                "title": "blackpill",
                "user": {"id": 1, "username": "eclipse media", "permalink": null, "avatar_url": null},
                "publisher_metadata": {"artist": "RULE69"}
            }"#,
        )
        .unwrap();
        assert_eq!(track.artist(), "RULE69");
    }

    #[test]
    fn uploader_is_the_artist_fallback() {
        let track: Track = serde_json::from_str(
            r#"{"id":1,"title":"Song","user":{"id":2,"username":"Uploader","permalink":null,"avatar_url":null}}"#,
        )
        .unwrap();
        assert_eq!(track.artist(), "Uploader");
    }

    #[test]
    fn current_metadata_artist_beats_uploader() {
        let track: Track = serde_json::from_str(
            r#"{"id":69,"title":"blackpill","metadata_artist":"RULE69","user":{"id":1,"username":"eclipse media"}}"#,
        )
        .unwrap();
        assert_eq!(track.artist(), "RULE69");
    }

    #[test]
    fn album_type_is_recognised_in_both_api_shapes() {
        let current: Playlist =
            serde_json::from_str(r#"{"id":1,"title":"A","playlist_type":"album"}"#).unwrap();
        let legacy: Playlist =
            serde_json::from_str(r#"{"id":2,"title":"B","is_album":true}"#).unwrap();
        assert!(current.is_album());
        assert!(legacy.is_album());
    }

    #[test]
    fn partial_embedded_tracks_do_not_hide_playlists() {
        let collection: Collection<Playlist> = serde_json::from_str(
            r#"{"collection":[{"id":1,"title":"A","tracks":[{"id":5},null],"is_album":null},{"id":2,"title":"B","tracks":null}],"next_href":null}"#,
        )
        .unwrap();
        assert_eq!(collection.collection.len(), 2);
        assert!(
            collection
                .collection
                .iter()
                .all(|item| item.tracks.is_empty())
        );
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Comment {
    pub id: u64,
    pub body: String,
    pub created_at: Option<String>,
    pub user: Option<UserLite>,
    pub timestamp_ms: Option<u64>,
}

fn comment_id<E: serde::de::Error>(
    id: Option<u64>,
    urn: Option<&str>,
    kind: &str,
) -> Result<u64, E> {
    if let Some(id) = id {
        return Ok(id);
    }
    let prefix = format!("soundcloud:{kind}:");
    urn.and_then(|urn| urn.strip_prefix(prefix.as_str()))
        .and_then(|id| id.parse().ok())
        .ok_or_else(|| E::custom("SoundCloud identifier is missing or invalid"))
}

fn comment_timestamp<E: serde::de::Error>(value: serde_json::Value) -> Result<Option<u64>, E> {
    use serde_json::Value;
    let number = match value {
        Value::Null => return Ok(None),
        Value::String(value) if value.trim().is_empty() => return Ok(None),
        Value::String(value) => value.trim().parse::<f64>().ok(),
        Value::Number(value) => value.as_f64(),
        _ => None,
    }
    .ok_or_else(|| E::custom("Invalid comment timestamp"))?;
    if !number.is_finite() || number >= u64::MAX as f64 {
        return Err(E::custom("Invalid comment timestamp"));
    }
    // Untimed comments may carry a negative sentinel; it is not a seek target.
    Ok((number >= 0.0).then(|| number.round() as u64))
}

impl<'de> Deserialize<'de> for Comment {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Author {
            id: Option<u64>,
            urn: Option<String>,
            username: Option<String>,
            permalink: Option<String>,
            avatar_url: Option<String>,
        }
        #[derive(Deserialize)]
        struct Wire {
            id: Option<u64>,
            urn: Option<String>,
            body: String,
            created_at: Option<String>,
            user: Option<Author>,
            #[serde(default, alias = "timestamp")]
            timestamp_ms: serde_json::Value,
        }
        let wire = Wire::deserialize(deserializer)?;
        let user = wire
            .user
            .filter(|author| author.id.is_some() || author.urn.is_some())
            .map(|author| {
                Ok::<_, D::Error>(UserLite {
                    id: comment_id(author.id, author.urn.as_deref(), "users")?,
                    username: author.username.unwrap_or_else(|| "SoundCloud user".into()),
                    permalink: author.permalink,
                    avatar_url: author.avatar_url,
                })
            })
            .transpose()?;
        Ok(Self {
            id: comment_id(wire.id, wire.urn.as_deref(), "comments")?,
            body: wire.body,
            created_at: wire.created_at,
            user,
            timestamp_ms: comment_timestamp(wire.timestamp_ms)?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Me {
    pub id: u64,
    pub username: String,
    pub permalink: Option<String>,
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub followers_count: Option<u64>,
}

/// Standard SoundCloud collection response (linked partitioning).
///
/// Deserialises from either shape the API can answer with: the paginated
/// object, and the bare array it returns when `linked_partitioning` is absent.
/// Everything we send sets it (see [`crate::api::paginated::PARTITIONING`]) —
/// but a route that ignores the parameter would otherwise fail to parse at all,
/// and a list that reads "the request failed" is much worse than one without a
/// next page.
#[derive(Debug, Clone, Serialize)]
pub struct Collection<T> {
    pub collection: Vec<T>,
    pub next_href: Option<String>,
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Collection<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// The two shapes, as they come off the wire. Untagged, so serde tries
        /// the object first and falls back to the array.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire<T> {
            Paginated {
                collection: Vec<T>,
                #[serde(default)]
                next_href: Option<String>,
            },
            /// The deprecated shape: everything at once, no cursor.
            Bare(Vec<T>),
        }

        Ok(match Wire::deserialize(deserializer)? {
            Wire::Paginated {
                collection,
                next_href,
            } => Self {
                collection,
                next_href,
            },
            Wire::Bare(collection) => Self {
                collection,
                next_href: None,
            },
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LikeEntry {
    pub created_at: Option<String>,
    #[serde(flatten)]
    pub target: LikeTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LikeTarget {
    Track(Box<Track>),
    Playlist(Box<Playlist>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEntry {
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default, rename = "type")]
    pub activity_type: Option<String>,
    #[serde(rename = "origin")]
    pub target: StreamTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum StreamTarget {
    Playlist(Box<Playlist>),
    Track(Box<Track>),
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_snip_policy_is_recognized_even_with_stale_playable_access() {
        let track: super::Track = serde_json::from_value(serde_json::json!({
            "id":1,"title":"Song","access":"playable","policy":"SNIP"
        })).unwrap();
        assert!(track.is_snippet());
    }

    use super::*;

    #[test]
    fn feed_reads_origin_and_distinguishes_tracks_from_playlists() {
        let feed: Collection<StreamEntry> = serde_json::from_str(
            r#"{"collection":[
            {"type":"track:repost","origin":{"kind":"track","id":1,"title":"Song"}},
            {"type":"playlist","origin":{"kind":"playlist","id":2,"title":"Set"}}
        ]}"#,
        )
        .unwrap();
        assert_eq!(
            feed.collection[0].activity_type.as_deref(),
            Some("track:repost")
        );
        assert!(matches!(&feed.collection[0].target, StreamTarget::Track(t) if t.id == 1));
        assert!(matches!(&feed.collection[1].target, StreamTarget::Playlist(p) if p.id == 2));
    }

    #[test]
    fn public_api_access_values_drive_playback_badges() {
        let preview: Track =
            serde_json::from_str(r#"{"id":1,"title":"Preview","access":"preview"}"#).unwrap();
        let blocked: Track =
            serde_json::from_str(r#"{"id":2,"title":"Blocked","access":"blocked"}"#).unwrap();
        assert_eq!(preview.access_badge(), Some("PREVIEW"));
        assert_eq!(blocked.access_badge(), Some("BLOCKED"));
    }

    #[test]
    fn public_api_duration_and_comment_timestamp_are_not_lost() {
        let list: Playlist =
            serde_json::from_str(r#"{"id":1,"title":"Set","duration":90000}"#).unwrap();
        let comment: Comment =
            serde_json::from_str(r#"{"id":1,"body":"Nice","timestamp":1200}"#).unwrap();
        assert_eq!(list.duration_ms, Some(90000));
        assert_eq!(comment.timestamp_ms, Some(1200));
    }

    #[test]
    fn comments_accept_current_urn_and_string_timestamp_schema() {
        let page: Collection<Comment> = serde_json::from_str(
            r#"{"collection":[
            {"urn":"soundcloud:comments:42","body":"Nice","timestamp":"1200.5",
             "user":{"urn":"soundcloud:users:7","username":"Listener"}},
            {"id":43,"body":"Untimed","timestamp":-1,"user":null},
            {"id":44,"body":"Untimed too","timestamp":null,"user":{}}
        ],"next_href":null}"#,
        )
        .unwrap();
        assert_eq!(page.collection.len(), 3);
        assert_eq!(page.collection[0].id, 42);
        assert_eq!(page.collection[0].timestamp_ms, Some(1201));
        assert_eq!(page.collection[0].user.as_ref().unwrap().id, 7);
        assert!(page.collection[1].timestamp_ms.is_none());
        assert!(page.collection[2].user.is_none());
        for input in [
            r#"{"body":"missing identifier"}"#,
            r#"{"id":1,"body":"Bad time","timestamp":"bad"}"#,
        ] {
            assert!(serde_json::from_str::<Comment>(input).is_err());
        }
    }

    const TRACK_JSON: &str = r#"{
        "id": 123,
        "title": "Song",
        "duration": 210000,
        "streamable": true,
        "user": {"id": 5, "username": "Artist"}
    }"#;

    #[test]
    fn parse_minimal_track() {
        let t: Track = serde_json::from_str(TRACK_JSON).unwrap();
        assert_eq!(t.id, 123);
        assert_eq!(t.artist(), "Artist");
        assert_eq!(t.effective_duration_ms(), 210000);
        assert_eq!(t.urn(), "soundcloud:tracks:123");
    }

    #[test]
    fn public_track_artwork_url_is_preserved() {
        let track: Track = serde_json::from_str(
            r#"{"id":1,"title":"Song","artwork_url":"https://i1.sndcdn.com/art.jpg"}"#,
        )
        .unwrap();

        assert_eq!(track.artwork_url(), Some("https://i1.sndcdn.com/art.jpg"));
    }

    #[test]
    fn playlist_cover_falls_back_to_its_first_track() {
        let playlist: Playlist = serde_json::from_str(
            r#"{"id":2,"title":"Set","tracks":[{"id":1,"title":"Song","artwork_url":"https://i1.sndcdn.com/art.jpg"}]}"#,
        )
        .unwrap();
        assert_eq!(
            playlist.artwork_url(),
            Some("https://i1.sndcdn.com/art.jpg")
        );
    }

    #[test]
    fn public_playlist_artwork_url_is_preserved() {
        let playlist: Playlist = serde_json::from_str(
            r#"{"id":1,"title":"Set","artwork_url":"https://i1.sndcdn.com/set.jpg"}"#,
        )
        .unwrap();

        assert_eq!(
            playlist.artwork_url(),
            Some("https://i1.sndcdn.com/set.jpg")
        );
    }

    #[test]
    fn parse_collection() {
        let json = r#"{"collection":[{"id":1,"title":"a"}],"next_href":"https://x"}"#;
        let c: Collection<Track> = serde_json::from_str(json).unwrap();
        assert_eq!(c.collection.len(), 1);
        assert!(c.next_href.is_some());
    }

    /// The API answers a **bare array** when `linked_partitioning` is absent,
    /// and every list in the app failed to parse because of it. We now always
    /// ask for pagination, but a route that ignores the parameter must still
    /// read rather than reporting "the request failed".
    #[test]
    fn a_collection_reads_from_either_shape() {
        // The paginated object, with and without a cursor.
        let c: Collection<Track> =
            serde_json::from_str(r#"{"collection":[{"id":1,"title":"a"}]}"#).unwrap();
        assert_eq!(c.collection.len(), 1);
        assert_eq!(c.next_href, None, "no cursor means no next page");

        // The deprecated bare array: same rows, and no next page to follow.
        let c: Collection<Track> =
            serde_json::from_str(r#"[{"id":1,"title":"a"},{"id":2,"title":"b"}]"#).unwrap();
        assert_eq!(c.collection.len(), 2);
        assert_eq!(c.collection[1].id, 2);
        assert_eq!(c.next_href, None);

        // Empty, in both shapes: a list with nothing in it, not an error.
        for json in [r#"{"collection":[]}"#, "[]"] {
            let c: Collection<Track> = serde_json::from_str(json).unwrap();
            assert!(c.collection.is_empty(), "{json}");
        }

        // …and something that is neither is still an error, so a mistyped
        // response does not quietly become an empty list.
        assert!(serde_json::from_str::<Collection<Track>>(r#"{"tracks":[]}"#).is_err());
        assert!(serde_json::from_str::<Collection<Track>>("null").is_err());
    }

    #[test]
    fn parse_like_entry() {
        let json = r#"{"created_at":"2024-01-01","id":9,"title":"liked"}"#;
        let e: LikeEntry = serde_json::from_str(json).unwrap();
        assert!(matches!(e.target, LikeTarget::Track(_)));
    }
}
