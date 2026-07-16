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
}

const ALLOWED_HEADERS: [&str; 2] = ["user-agent", "referer"];

/// Gate every resolver output before it reaches a player backend.
///
/// Allows http/https/HLS always; DASH only when the backend declares support
/// (`allow_dash`); file/local paths never (TC-PLY-003/004/005). Header
/// policy: only `User-Agent` and `Referer` may accompany a source (FR-063).
pub fn validate_source_opts(
    source: &PlayableSource,
    now_unix: u64,
    allow_dash: bool,
) -> Result<(), SourceRejection> {
    // Distrust the declared protocol: classify from the URL as well.
    if source
        .url
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("file:")
    {
        return Err(SourceRejection::LocalPathDenied(source.url.clone()));
    }
    match source.protocol {
        Protocol::Http | Protocol::Https | Protocol::Hls => {}
        Protocol::Dash if allow_dash => {}
        Protocol::Dash => return Err(SourceRejection::UnsupportedProtocol("dash".into())),
        Protocol::File => return Err(SourceRejection::LocalPathDenied(source.url.clone())),
    }
    if let Some(expires_at) = source.expires_at {
        if expires_at <= now_unix {
            return Err(SourceRejection::Expired);
        }
    }
    for header in source.headers.keys() {
        if !ALLOWED_HEADERS.contains(&header.to_ascii_lowercase().as_str()) {
            return Err(SourceRejection::ForbiddenHeader(header.clone()));
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
        }
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
