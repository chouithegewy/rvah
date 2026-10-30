use rvah_addon_sdk::{PlayableSource, Protocol};

/// Why a resolved source was refused by the host (FR-060, FR-063).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceRejection {
    #[error("unsupported protocol: {0}")]
    UnsupportedProtocol(String),
    #[error("source URL has expired")]
    Expired,
    #[error("forbidden header: {0}")]
    ForbiddenHeader(String),
    #[error("local paths are not playable: {0}")]
    LocalPathDenied(String),
    #[error("DRM is not supported: {0}")]
    UnsupportedDrm(String),
    #[error("subtitle URL is not an http(s) URL: {0}")]
    UnsafeSubtitle(String),
    #[error("header value contains control characters: {0}")]
    InvalidHeaderValue(String),
    #[error("never_block entry is not a host name: {0}")]
    InvalidNeverBlock(String),
    #[error("add-on {0} did not declare permission to open web pages")]
    WebNotDeclared(String),
}

/// What the host can play beyond plain http(s) and HLS.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SourcePolicy {
    /// The backend plays DASH manifests.
    pub dash: bool,
    /// The host opens web sources in its web player (Android TV).
    pub web: bool,
}

/// The most hosts a web source may ask its player never to block.
const MAX_NEVER_BLOCK: usize = 16;

/// A plain host name: two or more dot-separated labels of letters, digits
/// and inner hyphens, 1–63 characters each, the last not all digits. No
/// scheme, port, path, wildcard, bare top-level name or numeric address:
/// entries match by suffix, so `com` would unblock every `.com` host.
fn is_host_name(entry: &str) -> bool {
    let labels: Vec<&str> = entry.split('.').collect();
    entry.len() <= 253
        && labels.len() >= 2
        && labels
            .last()
            .is_some_and(|tld| !tld.bytes().all(|b| b.is_ascii_digit()))
        && labels.iter().all(|label| {
            (1..=63).contains(&label.len())
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

const ALLOWED_HEADERS: [&str; 2] = ["user-agent", "referer"];

/// Accept only absolute http(s) URLs with a host. Everything a native player
/// could otherwise open (bare paths, `file:`, ffmpeg/mpv pseudo-protocols
/// such as `edl://` or `lavf://`) is refused; bare paths and drive letters
/// are reported as local paths.
fn check_network_url(raw: &str) -> Result<(), SourceRejection> {
    let local = || SourceRejection::LocalPathDenied(raw.to_string());
    let url = url::Url::parse(raw.trim()).map_err(|_| local())?;
    match url.scheme() {
        // The URL parser already requires a host for http(s).
        "http" | "https" => Ok(()),
        "file" => Err(local()),
        // `C:\path` parses as scheme "c".
        scheme if scheme.len() == 1 => Err(local()),
        scheme => Err(SourceRejection::UnsupportedProtocol(scheme.to_string())),
    }
}

/// [`validate_source_with`] for a host without web sources.
pub fn validate_source_opts(
    source: &PlayableSource,
    now_unix: u64,
    allow_dash: bool,
) -> Result<(), SourceRejection> {
    validate_source_with(
        source,
        now_unix,
        SourcePolicy {
            dash: allow_dash,
            web: false,
        },
    )
}

/// Gate every resolver output before it reaches a player backend.
///
/// Allows http/https/HLS always; DASH only when the backend declares support;
/// web pages only on a host with a web player, over https and with no
/// headers; file/local paths never (TC-PLY-003/004/005). Header policy:
/// only `User-Agent` and `Referer` may accompany a source (FR-063).
pub fn validate_source_with(
    source: &PlayableSource,
    now_unix: u64,
    policy: SourcePolicy,
) -> Result<(), SourceRejection> {
    if let Some(drm) = source.drm {
        return Err(SourceRejection::UnsupportedDrm(
            format!("{drm:?}").to_ascii_lowercase(),
        ));
    }
    // Distrust the declared protocol: the URL itself must be http(s).
    check_network_url(&source.url)?;
    match source.protocol {
        Protocol::Http | Protocol::Https | Protocol::Hls => {}
        Protocol::Dash if policy.dash => {}
        Protocol::Dash => return Err(SourceRejection::UnsupportedProtocol("dash".into())),
        Protocol::File => return Err(SourceRejection::LocalPathDenied(source.url.clone())),
        Protocol::Web if policy.web => {
            if !source
                .url
                .trim()
                .to_ascii_lowercase()
                .starts_with("https://")
            {
                return Err(SourceRejection::UnsupportedProtocol("web over http".into()));
            }
            if let Some(header) = source.headers.keys().next() {
                return Err(SourceRejection::ForbiddenHeader(header.clone()));
            }
        }
        Protocol::Web => return Err(SourceRejection::UnsupportedProtocol("web".into())),
        Protocol::Veilid => {
            return Err(SourceRejection::UnsupportedProtocol(
                "veilid requires the browser host transport".into(),
            ));
        }
    }
    if !source.never_block.is_empty() && source.protocol != Protocol::Web {
        return Err(SourceRejection::InvalidNeverBlock(
            "never_block is for web sources".into(),
        ));
    }
    if source.never_block.len() > MAX_NEVER_BLOCK {
        return Err(SourceRejection::InvalidNeverBlock(format!(
            "{} entries (at most {MAX_NEVER_BLOCK})",
            source.never_block.len()
        )));
    }
    if let Some(bad) = source.never_block.iter().find(|e| !is_host_name(e)) {
        return Err(SourceRejection::InvalidNeverBlock(bad.clone()));
    }
    if let Some(expires_at) = source.expires_at {
        if expires_at <= now_unix {
            return Err(SourceRejection::Expired);
        }
    }
    for (header, value) in &source.headers {
        if !ALLOWED_HEADERS.contains(&header.to_ascii_lowercase().as_str()) {
            return Err(SourceRejection::ForbiddenHeader(header.clone()));
        }
        if value.chars().any(char::is_control) {
            return Err(SourceRejection::InvalidHeaderValue(header.clone()));
        }
    }
    for track in &source.subtitles {
        if check_network_url(&track.url).is_err() {
            return Err(SourceRejection::UnsafeSubtitle(track.url.clone()));
        }
    }
    Ok(())
}

/// [`validate_source_opts`] with the conservative default (no DASH).
pub fn validate_source(source: &PlayableSource, now_unix: u64) -> Result<(), SourceRejection> {
    validate_source_opts(source, now_unix, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn source(protocol: Protocol, url: &str) -> PlayableSource {
        PlayableSource {
            url: url.into(),
            protocol,
            headers: BTreeMap::new(),
            subtitles: vec![],
            quality: None,
            audio_lang: None,
            expires_at: None,
            drm: None,
            never_block: Vec::new(),
        }
    }

    fn web(url: &str) -> PlayableSource {
        source(Protocol::Web, url)
    }

    const WEB: SourcePolicy = SourcePolicy {
        dash: false,
        web: true,
    };

    #[test]
    fn web_needs_the_policy() {
        assert!(matches!(
            validate_source_with(&web("https://ex.com/p"), 0, SourcePolicy::default()),
            Err(SourceRejection::UnsupportedProtocol(_))
        ));
        assert!(validate_source_with(&web("https://ex.com/p"), 0, WEB).is_ok());
    }

    #[test]
    fn web_must_be_https_and_headerless() {
        assert!(validate_source_with(&web("http://ex.com/p"), 0, WEB).is_err());
        let mut s = web("https://ex.com/p");
        s.headers.insert("Referer".into(), "https://ex.com".into());
        assert!(matches!(
            validate_source_with(&s, 0, WEB),
            Err(SourceRejection::ForbiddenHeader(_))
        ));
    }

    #[test]
    fn never_block_must_be_plain_hosts() {
        let mut s = web("https://ex.com/p");
        s.never_block = vec!["cdn.example.net".into(), "Example.ORG".into()];
        assert!(validate_source_with(&s, 0, WEB).is_ok());
        for bad in [
            "https://x.com",
            "x.com/a",
            "*.x.com",
            "x.com:443",
            "",
            "-x.com",
            "x..com",
            // A bare top-level name or a numeric address would unblock a
            // whole namespace by suffix.
            "com",
            "localhost",
            "10.0.0.1",
        ] {
            s.never_block = vec![bad.into()];
            assert!(
                matches!(
                    validate_source_with(&s, 0, WEB),
                    Err(SourceRejection::InvalidNeverBlock(_))
                ),
                "{bad}"
            );
        }
        s.never_block = (0..17).map(|i| format!("h{i}.example.com")).collect();
        assert!(validate_source_with(&s, 0, WEB).is_err());
        let mut hls = source(Protocol::Hls, "https://ex.com/a.m3u8");
        hls.never_block = vec!["cdn.example.net".into()];
        assert!(validate_source_with(&hls, 0, WEB).is_err());
    }

    #[test]
    fn validator_accepts_http_https_hls() {
        assert!(validate_source(&source(Protocol::Https, "https://ex.com/a.mp4"), 0).is_ok());
        assert!(validate_source(&source(Protocol::Http, "http://ex.com/a.mp4"), 0).is_ok());
        assert!(validate_source(&source(Protocol::Hls, "https://ex.com/a.m3u8"), 0).is_ok());
    }

    #[test]
    fn validator_rejects_unsupported_protocol() {
        // TC-PLY-004: DASH deferred in MVP; file always denied.
        assert_eq!(
            validate_source(&source(Protocol::Dash, "https://ex.com/a.mpd"), 0),
            Err(SourceRejection::UnsupportedProtocol("dash".into()))
        );
        assert!(matches!(
            validate_source(&source(Protocol::File, "file:///etc/passwd"), 0),
            Err(SourceRejection::LocalPathDenied(_))
        ));
        // A lying protocol field does not smuggle a file URL through.
        assert!(matches!(
            validate_source(&source(Protocol::Https, "file:///etc/passwd"), 0),
            Err(SourceRejection::LocalPathDenied(_))
        ));
    }

    #[test]
    fn validator_allows_dash_only_with_backend_support() {
        let s = source(Protocol::Dash, "https://ex.com/a.mpd");
        assert_eq!(
            validate_source_opts(&s, 0, false),
            Err(SourceRejection::UnsupportedProtocol("dash".into()))
        );
        assert!(validate_source_opts(&s, 0, true).is_ok());
        // File stays denied regardless of capability.
        let f = source(Protocol::File, "file:///etc/passwd");
        assert!(validate_source_opts(&f, 0, true).is_err());
    }

    #[test]
    fn validator_rejects_expired() {
        // TC-PLY-005: expires_at <= now is stale.
        let mut s = source(Protocol::Https, "https://ex.com/a.mp4");
        s.expires_at = Some(1_000);
        assert_eq!(validate_source(&s, 1_000), Err(SourceRejection::Expired));
        assert_eq!(validate_source(&s, 2_000), Err(SourceRejection::Expired));
        assert!(validate_source(&s, 999).is_ok());
    }

    #[test]
    fn validator_rejects_forbidden_header_allows_ua_referer() {
        // TC-PLY-003 / FR-063: only user-agent and referer pass, any case.
        let mut s = source(Protocol::Https, "https://ex.com/a.mp4");
        s.headers.insert("User-Agent".into(), "rvah/0.1".into());
        s.headers.insert("Referer".into(), "https://ex.com".into());
        assert!(validate_source(&s, 0).is_ok());

        s.headers.insert("Cookie".into(), "session=abc".into());
        assert_eq!(
            validate_source(&s, 0),
            Err(SourceRejection::ForbiddenHeader("Cookie".into()))
        );
    }
}
