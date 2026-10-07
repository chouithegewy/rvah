//! Resolver output is untrusted add-on data that reaches a native player
//! (mpv, Media3). Only http(s) network URLs may get through, for the main
//! source and for subtitles alike, and header values cannot smuggle extra
//! request lines.

use rvah_addon_sdk::{PlayableSource, Protocol, SubtitleTrack};
use rvah_player::{validate_source, SourceRejection};

fn source(url: &str) -> PlayableSource {
    PlayableSource {
        url: url.into(),
        protocol: Protocol::Https,
        headers: Default::default(),
        subtitles: vec![],
        quality: None,
        audio_lang: None,
        expires_at: None,
        drm: None,
    }
}

fn subtitle(url: &str) -> SubtitleTrack {
    SubtitleTrack {
        url: url.into(),
        lang: "en".into(),
        label: None,
        format: None,
    }
}

#[test]
fn declared_https_does_not_let_local_paths_through() {
    for url in ["/etc/passwd", "C:\\Users\\me\\secret.mkv", "  /tmp/x.mp4"] {
        assert!(
            matches!(
                validate_source(&source(url), 0),
                Err(SourceRejection::LocalPathDenied(_))
            ),
            "accepted {url}"
        );
    }
}

#[test]
fn non_http_schemes_are_refused() {
    // mpv/ffmpeg understand far more than http: these must never be reachable
    // from add-on output.
    for url in [
        "ftp://127.0.0.1/private",
        "edl://%3%foo",
        "av://lavfi:testsrc",
        "lavf://concat:/etc/passwd",
        "smb://nas/share/movie.mkv",
        "data:video/mp4;base64,AAAA",
        "https://",
    ] {
        assert!(validate_source(&source(url), 0).is_err(), "accepted {url}");
    }
}

#[test]
fn lan_and_loopback_http_sources_remain_playable() {
    // Local media servers are a legitimate add-on use; documented in
    // docs/security-review.md.
    for url in [
        "http://127.0.0.1:8096/videos/1/stream.mp4",
        "https://192.168.1.20/movie.m3u8",
    ] {
        assert!(validate_source(&source(url), 0).is_ok(), "refused {url}");
    }
}

#[test]
fn subtitle_urls_must_be_http() {
    for url in [
        "file:///etc/passwd",
        "/home/me/.ssh/id_ed25519",
        "ftp://x/en.srt",
    ] {
        let mut value = source("https://example.com/video.mp4");
        value.subtitles.push(subtitle(url));
        assert!(
            matches!(
                validate_source(&value, 0),
                Err(SourceRejection::UnsafeSubtitle(_))
            ),
            "accepted subtitle {url}"
        );
    }
    let mut ok = source("https://example.com/video.mp4");
    ok.subtitles
        .push(subtitle("https://cdn.example.com/en.vtt"));
    assert!(validate_source(&ok, 0).is_ok());
}

#[test]
fn header_values_cannot_inject_request_lines() {
    for value in [
        "https://example.com\r\nAuthorization: Bearer smuggled",
        "https://example.com\nX-Evil: 1",
        "agent\0",
    ] {
        let mut source = source("https://example.com/video.mp4");
        source.headers.insert("Referer".into(), value.into());
        assert!(
            matches!(
                validate_source(&source, 0),
                Err(SourceRejection::InvalidHeaderValue(_))
            ),
            "accepted header value {value:?}"
        );
    }
}
