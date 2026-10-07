use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Kind of content a [`MediaItem`] represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentType {
    Movie,
    Episode,
    Series,
    Season,
    Clip,
    Other,
}

/// Artwork references for list items and detail views.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artwork {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poster: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fanart: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb: Option<String>,
}

/// A navigable directory entry: selecting it lists `path`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectoryItem {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub artwork: Artwork,
    /// Provider-defined path passed back to `list` when the user enters it.
    pub path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub badges: Vec<String>,
}

/// A playable media entry: selecting it resolves sources by `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaItem {
    pub id: String,
    pub content_type: ContentType,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_secs: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synopsis: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<f32>,
    #[serde(default)]
    pub artwork: Artwork,
    /// Provider-specific external IDs, e.g. `{"imdb": "tt0111161"}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_ids: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageItem {
    Directory(DirectoryItem),
    Media(MediaItem),
}

/// One page of catalog or search results.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub items: Vec<PageItem>,
    /// Opaque cursor for the next page; `None` means last page (FR-024).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_next: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_hint: Option<u64>,
}

/// Streaming protocol of a resolved source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Http,
    Https,
    Hls,
    Dash,
    File,
}

/// Subtitle container/codec hint. Existing add-ons may omit this and hosts
/// will infer the format from the URL extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleFormat {
    WebVtt,
    SubRip,
    Ttml,
    Ssa,
    Ass,
}

impl SubtitleFormat {
    pub fn infer(url: &str) -> Option<Self> {
        let path = url.split(['?', '#']).next().unwrap_or(url);
        match path.rsplit_once('.')?.1.to_ascii_lowercase().as_str() {
            "vtt" => Some(Self::WebVtt),
            "srt" => Some(Self::SubRip),
            "ttml" | "xml" | "dfxp" => Some(Self::Ttml),
            "ssa" => Some(Self::Ssa),
            "ass" => Some(Self::Ass),
            _ => None,
        }
    }
}

/// Protected sources are represented explicitly so the clear-stream
/// milestone fails closed instead of passing DRM material to a player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrmScheme {
    Widevine,
    PlayReady,
    FairPlay,
    Other,
}

impl Protocol {
    /// Classify a URL by scheme and well-known manifest extensions.
    pub fn from_url(url: &str) -> Option<Protocol> {
        let (scheme, rest) = url.split_once("://")?;
        let path = rest.split(['?', '#']).next().unwrap_or(rest);
        match scheme.to_ascii_lowercase().as_str() {
            "file" => Some(Protocol::File),
            "http" | "https" => {
                if path.to_ascii_lowercase().ends_with(".m3u8") {
                    Some(Protocol::Hls)
                } else if path.to_ascii_lowercase().ends_with(".mpd") {
                    Some(Protocol::Dash)
                } else if scheme.eq_ignore_ascii_case("https") {
                    Some(Protocol::Https)
                } else {
                    Some(Protocol::Http)
                }
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubtitleTrack {
    pub url: String,
    pub lang: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<SubtitleFormat>,
}

impl SubtitleTrack {
    pub fn effective_format(&self) -> Option<SubtitleFormat> {
        self.format.or_else(|| SubtitleFormat::infer(&self.url))
    }
}

/// A fully resolved, host-validated candidate the player can load (FR-060).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayableSource {
    pub url: String,
    pub protocol: Protocol,
    /// Request headers; the host permits only user-agent/referer (FR-063).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtitles: Vec<SubtitleTrack>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_lang: Option<String>,
    /// Unix seconds after which the URL is no longer valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// Present only for protected streams. RVAH currently rejects every DRM
    /// scheme with a clear unsupported-DRM error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drm: Option<DrmScheme>,
}

/// Outcome of resolving a media item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ResolveResult {
    Playables(Vec<PlayableSource>),
    AuthRequired { provider_hint: String },
    Unavailable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AccountStatus {
    SignedOut,
    SignedIn { display_name: Option<String> },
    Expired,
}

/// A search filter, e.g. `{key: "year", value: "1999"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    pub key: String,
    pub value: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_page() -> Page {
        Page {
            items: vec![
                PageItem::Directory(DirectoryItem {
                    id: "dir-1".into(),
                    title: "Movies".into(),
                    subtitle: None,
                    artwork: Artwork::default(),
                    path: "/movies".into(),
                    badges: vec!["new".into()],
                }),
                PageItem::Media(MediaItem {
                    id: "m-1".into(),
                    content_type: ContentType::Movie,
                    title: "Example".into(),
                    year: Some(1999),
                    season: None,
                    episode: None,
                    runtime_secs: Some(7200),
                    synopsis: Some("A film.".into()),
                    rating: Some(8.1),
                    artwork: Artwork {
                        poster: Some("https://img.example/p.jpg".into()),
                        ..Default::default()
                    },
                    provider_ids: BTreeMap::from([("imdb".into(), "tt123".into())]),
                }),
            ],
            cursor_next: Some("page-2".into()),
            total_hint: Some(42),
        }
    }

    #[test]
    fn page_roundtrips_through_json() {
        let page = sample_page();
        let json = serde_json::to_string(&page).unwrap();
        let back: Page = serde_json::from_str(&json).unwrap();
        assert_eq!(page, back);
    }

    #[test]
    fn resolve_result_roundtrips() {
        let rr = ResolveResult::Playables(vec![PlayableSource {
            url: "https://cdn.example/v.m3u8".into(),
            protocol: Protocol::Hls,
            headers: BTreeMap::from([("User-Agent".into(), "rvah".into())]),
            subtitles: vec![SubtitleTrack {
                url: "https://cdn.example/v.srt".into(),
                lang: "en".into(),
                label: None,
                format: None,
            }],
            quality: Some("1080p".into()),
            audio_lang: Some("en".into()),
            expires_at: Some(1_800_000_000),
            drm: None,
        }]);
        let json = serde_json::to_string(&rr).unwrap();
        assert_eq!(rr, serde_json::from_str::<ResolveResult>(&json).unwrap());

        let auth = ResolveResult::AuthRequired {
            provider_hint: "example".into(),
        };
        let json = serde_json::to_string(&auth).unwrap();
        assert!(json.contains("auth_required"));
        assert_eq!(auth, serde_json::from_str::<ResolveResult>(&json).unwrap());
    }

    #[test]
    fn protocol_from_url() {
        assert_eq!(
            Protocol::from_url("https://a.example/x.mp4"),
            Some(Protocol::Https)
        );
        assert_eq!(
            Protocol::from_url("http://a.example/x.mp4"),
            Some(Protocol::Http)
        );
        assert_eq!(
            Protocol::from_url("https://a.example/master.m3u8?tok=1"),
            Some(Protocol::Hls)
        );
        assert_eq!(
            Protocol::from_url("https://a.example/man.mpd"),
            Some(Protocol::Dash)
        );
        assert_eq!(
            Protocol::from_url("file:///tmp/x.mp4"),
            Some(Protocol::File)
        );
        assert_eq!(Protocol::from_url("rtmp://a.example/live"), None);
        assert_eq!(Protocol::from_url("not-a-url"), None);
    }
}
