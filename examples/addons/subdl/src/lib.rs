//! SubDL Subtitles add-on for RVAH (ABI v0.1 wasm guest).
//!
//! Finds subtitles for the film or episode playing on
//! [SubDL](https://subdl.com), those made for the release playing first
//! (same release group, resolution, source…), plain ones before SDH. Each is
//! one subtitle file the player loads as it is, from dl.subdl.com; a season
//! pack's file for the episode is picked out by its number. Needs your SubDL
//! API key (free: 2,000 searches a day).

pub mod subdl;

use rvah_addon_sdk::guest::{read_op, Host, Op, Response};
use rvah_addon_sdk::{AddonError, AddonErrorCode};

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn dispatch(input: &[u8], host: &dyn Host) -> Result<Response, AddonError> {
    match read_op(input)? {
        Op::Subtitles { request } => subdl::subtitles(host, &request).map(Response::Subtitles),
        other => Err(AddonError::new(
            AddonErrorCode::Internal,
            format!("not an operation this add-on declares: {other:?}"),
        )),
    }
}

rvah_addon_sdk::export_guest!(dispatch);

#[cfg(test)]
mod tests {
    use super::*;
    use rvah_addon_sdk::guest::ScriptedHost;
    use rvah_addon_sdk::{SourceKind, SubtitleFormat, SubtitleRequest, SubtitleTrack};
    use serde_json::{json, Value};

    const KEY: &str = "k-0123456789abcdefghijklmnopqrst";
    const SEARCH: &str = "https://api.subdl.com/api/v1/subtitles";

    fn host() -> ScriptedHost {
        let mut host = ScriptedHost::default();
        host.secrets.insert("api_key".into(), KEY.into());
        host
    }

    fn episode() -> SubtitleRequest {
        SubtitleRequest {
            kind: SourceKind::Episode,
            imdb_id: Some("tt0903747".into()),
            title: "Acme Corp".into(),
            year: Some(2008),
            season: Some(1),
            episode: Some(2),
            languages: vec!["en".into()],
            release_name: Some("Acme.Corp.S01E02.1080p.WEB-DL.DDP5.1.H.264-FLUX".into()),
        }
    }

    fn file(name: &str, season: u32, episode: Option<u32>, id: &str, hi: bool) -> Value {
        json!({
            "file_n_id": id, "name": name, "release_name": name.trim_end_matches(".srt"),
            "season": season, "episode": episode, "language": "EN", "hi": hi, "format": "srt",
            "size": 43_070, "md5": "ac8f644f36a96f9547a969bf68b7571b",
            "url": format!("/subtitle/page{id}/{id}?api_key={KEY}")
        })
    }

    fn upload(
        release: &str,
        season: u32,
        episode: Option<u32>,
        hi: bool,
        files: Vec<Value>,
    ) -> Value {
        json!({
            "release_name": release, "name": format!("{release}.zip"), "lang": "English", "author": "someone",
            "url": format!("/subtitle/1-2.zip?api_key={KEY}"), "subtitlePage": "/s/info/x",
            "season": season, "episode": episode, "language": "EN", "framerate": 0, "fps": null, "hi": hi,
            "episode_from": null, "episode_end": 0, "full_season": episode.is_none(), "unpack_files": files
        })
    }

    fn answer(subtitles: Vec<Value>) -> Value {
        json!({
            "status": true, "results": [{ "sd_id": 1, "name": "Acme Corp", "type": "tv", "imdb_id": "tt0903747" }],
            "subtitles": subtitles, "totalPages": 1, "currentPage": 1
        })
    }

    fn found(tracks: &[SubtitleTrack]) -> Vec<(&str, &str)> {
        tracks
            .iter()
            .map(|t| (t.url.as_str(), t.label.as_deref().unwrap_or_default()))
            .collect()
    }

    #[test]
    fn an_episode_gets_its_files_the_release_playing_first() {
        let mut host = host();
        host.route(
            &format!("{SEARCH}?api_key={KEY}&imdb_id=tt0903747&type=tv&season_number=1&episode_number=2&languages=EN&subs_per_page=30&unpack=1"),
            200,
            &answer(vec![
                // A season pack: only episode 2's file.
                upload("Acme Corp, Season 1 1080p BluRay", 1, None, false, vec![
                    file("01 Pilot.en.srt", 0, Some(1), "p1", false),
                    file("02 Cat's in the Bag.en.srt", 0, Some(2), "p2", false),
                ]),
                // Made for the release playing, but for the deaf and hard of hearing.
                upload("Acme.Corp.S01E02.1080p.WEB-DL.DDP5.1.H.264-FLUX", 1, Some(2), true, vec![
                    file("Acme.Corp.S01E02.1080p.WEB-DL.DDP5.1.H.264-FLUX.srt", 1, None, "f2", true),
                ]),
                // The same release, plain.
                upload("Acme.Corp.S01E02.WEB-DL-FLUX", 1, Some(2), false, vec![
                    file("Acme.Corp.S01E02.WEB-DL-FLUX.srt", 1, Some(2), "g2", false),
                ]),
                // Another season's pack.
                upload("Acme Corp, Season 2", 2, None, false, vec![file("02.srt", 0, Some(2), "s2", false)]),
            ]),
        );
        let tracks = subdl::subtitles(&host, &episode()).unwrap();
        assert_eq!(
            found(&tracks),
            [
                (
                    "https://dl.subdl.com/subtitle/pagef2/f2",
                    "Acme.Corp.S01E02.1080p.WEB-DL.DDP5.1.H.264-FLUX · SDH"
                ),
                (
                    "https://dl.subdl.com/subtitle/pageg2/g2",
                    "Acme.Corp.S01E02.WEB-DL-FLUX"
                ),
                (
                    "https://dl.subdl.com/subtitle/pagep2/p2",
                    "Acme Corp, Season 1 1080p BluRay"
                ),
            ]
        );
        assert!(tracks
            .iter()
            .all(|t| t.lang == "en" && t.format == Some(SubtitleFormat::SubRip)));
        assert!(
            tracks.iter().all(|t| !t.url.contains(KEY)),
            "the key stays in the add-on"
        );
    }

    #[test]
    fn a_film_without_an_imdb_id_is_found_by_title_and_skips_split_files() {
        let mut host = host();
        host.route(
            &format!("{SEARCH}?api_key={KEY}&film_name=Coyote%20vs.%20Acme&year=2026&type=movie&languages=EN%2CFR&subs_per_page=30&unpack=1"),
            200,
            &answer(vec![
                upload("Coyote.vs.Acme.2026.DVDRip", 0, None, false, vec![
                    file("CD1.srt", 0, None, "c1", false),
                    file("CD2.srt", 0, None, "c2", false),
                ]),
                upload("Coyote.vs.Acme.2026.1080p.WEBRip", 0, None, false, vec![
                    json!({ "name": "a.sub", "format": "sub", "url": "/subtitle/x/sub" }),
                ]),
                upload("Coyote.vs.Acme.2026.720p.WEB", 0, None, false, vec![
                    json!({ "name": "fr.srt", "language": "FR", "format": "srt", "url": "/subtitle/y/fr" }),
                ]),
            ]),
        );
        let request = SubtitleRequest {
            kind: SourceKind::Movie,
            imdb_id: None,
            title: "Coyote vs. Acme".into(),
            year: Some(2026),
            season: None,
            episode: None,
            languages: vec!["en".into(), "fr".into()],
            release_name: None,
        };
        let tracks = subdl::subtitles(&host, &request).unwrap();
        assert_eq!(
            found(&tracks),
            [(
                "https://dl.subdl.com/subtitle/y/fr",
                "Coyote.vs.Acme.2026.720p.WEB"
            )]
        );
        assert_eq!(tracks[0].lang, "fr");
    }

    #[test]
    fn nothing_found_is_no_subtitles() {
        let mut host = host();
        host.route(
            &format!("{SEARCH}?api_key={KEY}&imdb_id=tt0903747&type=tv&season_number=1&episode_number=2&languages=EN&subs_per_page=30&unpack=1"),
            200,
            &json!({ "status": false, "error": "can't find movie or tv" }),
        );
        assert!(subdl::subtitles(&host, &episode()).unwrap().is_empty());
    }

    #[test]
    fn the_key_is_asked_for_when_missing_or_refused() {
        let err = subdl::subtitles(&ScriptedHost::default(), &episode()).unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Auth);
        assert!(err.user_visible);
        let mut host = host();
        host.route(
            &format!("{SEARCH}?api_key={KEY}&imdb_id=tt0903747&type=tv&season_number=1&episode_number=2&languages=EN&subs_per_page=30&unpack=1"),
            403,
            &json!({ "status": false, "statusCode": 403, "error": "not_authorized" }),
        );
        let err = subdl::subtitles(&host, &episode()).unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Auth);
    }

    #[test]
    fn dispatch_speaks_the_wire_format() {
        let op = json!({ "op": "subtitles", "request": {
            "kind": "movie", "title": "Film", "languages": ["en"]
        } });
        let out = dispatch(&serde_json::to_vec(&op).unwrap(), &ScriptedHost::default());
        // No key: an error, but through the subtitles path.
        assert_eq!(out.unwrap_err().code, AddonErrorCode::Auth);
        let err = dispatch(br#"{"op":"details","item_id":"x"}"#, &host()).unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Internal);
    }
}
