//! Prowlarr Sources add-on for RVAH (ABI v0.1 wasm guest).
//!
//! Finds releases of a film or an episode through your own Prowlarr
//! (`GET /api/v1/search`), searching by IMDb id and falling back to the
//! title. For an episode it asks for the whole season, keeping that
//! episode's releases and season packs, since a debrid add-on can play an
//! episode out of a pack. Only torrents with an info hash are kept, best
//! resolution first, then most seeders. Which indexers Prowlarr searches is
//! up to you; this add-on ships none.

pub mod prowlarr;

use rvah_addon_sdk::guest::{read_op, Host, Op, Response};
use rvah_addon_sdk::{AddonError, AddonErrorCode};

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn dispatch(input: &[u8], host: &dyn Host) -> Result<Response, AddonError> {
    match read_op(input)? {
        Op::Sources { request } => prowlarr::sources(host, &request).map(Response::Releases),
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
    use rvah_addon_sdk::{SourceKind, SourceRequest};
    use serde_json::{json, Value};

    const URL: &str = "https://prowlarr.example.com";
    const KEY: &str = "0123456789abcdef0123456789abcdef";

    fn host() -> ScriptedHost {
        let mut host = ScriptedHost::default();
        host.secrets.insert("api_key".into(), KEY.into());
        host.settings.insert("url".into(), URL.into());
        host
    }

    fn found(title: &str, hash: Option<&str>, seeders: u32) -> Value {
        json!({
            "guid": format!("https://indexer.example/{title}"), "age": 3, "size": 4_294_967_296u64,
            "indexerId": 7, "indexer": "Example", "title": title, "sortTitle": title.to_lowercase(),
            "imdbId": 133093, "tmdbId": 603, "publishDate": "2026-10-01T00:00:00Z",
            "downloadUrl": "https://prowlarr.example.com/7/download?link=abc",
            "infoUrl": "https://indexer.example/t/1", "infoHash": hash, "seeders": seeders,
            "leechers": 3, "protocol": "torrent", "categories": [{ "id": 2000, "name": "Movies" }]
        })
    }

    fn movie() -> SourceRequest {
        SourceRequest {
            kind: SourceKind::Movie,
            imdb_id: Some("tt0133093".into()),
            title: "The Matrix".into(),
            year: Some(1999),
            season: None,
            episode: None,
        }
    }

    fn episode() -> SourceRequest {
        SourceRequest {
            kind: SourceKind::Episode,
            imdb_id: Some("tt0944947".into()),
            title: "Acme Corp".into(),
            year: None,
            season: Some(1),
            episode: Some(2),
        }
    }

    fn search_url(query: &str, kind: &str, categories: u32) -> String {
        format!("{URL}/api/v1/search?query={query}&type={kind}&categories={categories}&limit=75")
    }

    fn names(releases: &[rvah_addon_sdk::Release]) -> Vec<&str> {
        releases.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn a_film_is_searched_by_imdb_id_and_sorted_by_resolution_then_seeders() {
        let mut host = host();
        let mut nzb = found("The.Matrix.1999.1080p.NZB", Some(&"e".repeat(40)), 0);
        nzb["protocol"] = json!("usenet");
        host.route(
            &search_url("%7BImdbId%3Att0133093%7D", "movie", 2000),
            200,
            &json!([
                found("The.Matrix.1999.720p.BluRay", Some(&"a".repeat(40)), 900),
                found(
                    "The.Matrix.1999.2160p.UHD.BluRay.REMUX",
                    Some(&"b".repeat(40)),
                    40
                ),
                found("The.Matrix.1999.1080p.BluRay", Some(&"c".repeat(40)), 500),
                found("The.Matrix.1999.1080p.WEB", Some(&"d".repeat(40)), 800),
                // No info hash: nothing a debrid service can add.
                found("The.Matrix.1999.1080p.NoHash", None, 999),
                nzb,
            ]),
        );
        let releases = prowlarr::sources(&host, &movie()).unwrap();
        assert_eq!(
            names(&releases),
            [
                "The.Matrix.1999.2160p.UHD.BluRay.REMUX",
                "The.Matrix.1999.1080p.WEB",
                "The.Matrix.1999.1080p.BluRay",
                "The.Matrix.1999.720p.BluRay",
            ]
        );
        let first = &releases[0];
        assert_eq!(first.info_hash, "b".repeat(40));
        assert_eq!(first.size_bytes, Some(4 << 30));
        assert_eq!(first.seeders, Some(40));
        assert_eq!(first.origin.as_deref(), Some("Example"));
        let sent = &host.requests()[0];
        assert_eq!(sent.headers["x-api-key"], KEY);
    }

    #[test]
    fn an_episode_keeps_its_releases_and_season_packs() {
        let mut host = host();
        host.route(
            &search_url("%7BImdbId%3Att0944947%7D%7BSeason%3A1%7D", "tvsearch", 5000),
            200,
            &json!([
                found("Acme.Corp.S01E01.1080p.WEB", Some(&"1".repeat(40)), 100),
                found("Acme.Corp.S01E02.1080p.WEB", Some(&"2".repeat(40)), 90),
                found("Acme.Corp.S01.1080p.BluRay", Some(&"3".repeat(40)), 50),
                found(
                    "Acme Corp Season 1 Complete 720p",
                    Some(&"4".repeat(40)),
                    10
                ),
                found("Acme.Corp.S01E01E02.720p", Some(&"5".repeat(40)), 5),
                found("Acme.Corp.S02.1080p", Some(&"6".repeat(40)), 70),
            ]),
        );
        let releases = prowlarr::sources(&host, &episode()).unwrap();
        assert_eq!(
            names(&releases),
            [
                "Acme.Corp.S01E02.1080p.WEB",
                "Acme.Corp.S01.1080p.BluRay",
                "Acme Corp Season 1 Complete 720p",
                "Acme.Corp.S01E01E02.720p",
            ]
        );
    }

    #[test]
    fn nothing_by_id_falls_back_to_the_title() {
        let mut host = host();
        host.route(
            &search_url("%7BImdbId%3Att0133093%7D", "movie", 2000),
            200,
            &json!([]),
        );
        host.route(
            &search_url("The%20Matrix%201999", "search", 2000),
            200,
            &json!([found("The Matrix 1999 1080p", Some(&"f".repeat(40)), 1)]),
        );
        let releases = prowlarr::sources(&host, &movie()).unwrap();
        assert_eq!(names(&releases), ["The Matrix 1999 1080p"]);

        let mut host = self::host();
        host.route(
            &search_url("%7BImdbId%3Att0944947%7D%7BSeason%3A1%7D", "tvsearch", 5000),
            200,
            &json!([]),
        );
        host.route(
            &search_url("Acme%20Corp%20S01", "search", 5000),
            200,
            &json!([]),
        );
        assert!(prowlarr::sources(&host, &episode()).unwrap().is_empty());
        assert_eq!(host.urls().len(), 2);
    }

    #[test]
    fn a_magnet_link_gives_the_hash_when_the_field_is_missing() {
        let mut host = host();
        let mut magnet = found("The.Matrix.1999.1080p", None, 3);
        magnet["magnetUrl"] = json!(format!(
            "magnet:?xt=urn:btih:{}&dn=The.Matrix",
            "A".repeat(40)
        ));
        // Prowlarr's proxy links are not magnets: nothing to read there.
        let mut proxied = found("The.Matrix.1999.720p", None, 3);
        proxied["magnetUrl"] = json!("https://prowlarr.example.com/7/download?link=x");
        host.route(
            &search_url("%7BImdbId%3Att0133093%7D", "movie", 2000),
            200,
            &json!([magnet, proxied]),
        );
        let releases = prowlarr::sources(&host, &movie()).unwrap();
        assert_eq!(names(&releases), ["The.Matrix.1999.1080p"]);
        assert_eq!(releases[0].info_hash, "A".repeat(40));
    }

    #[test]
    fn a_missing_or_rejected_key_asks_for_one() {
        let mut host = ScriptedHost::default();
        host.settings.insert("url".into(), URL.into());
        let err = prowlarr::sources(&host, &movie()).unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Auth);
        assert!(err.user_visible && err.message.contains("api_key"));

        let mut host = self::host();
        host.route(
            &search_url("%7BImdbId%3Att0133093%7D", "movie", 2000),
            401,
            &json!({}),
        );
        let err = prowlarr::sources(&host, &movie()).unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Auth);
    }

    #[test]
    fn the_address_must_be_https() {
        let mut host = host();
        host.settings
            .insert("url".into(), "http://prowlarr.example.com".into());
        let err = prowlarr::sources(&host, &movie()).unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Malformed);
        assert!(host.urls().is_empty());
    }

    #[test]
    fn dispatch_answers_sources() {
        let mut host = host();
        host.route(
            &search_url("%7BImdbId%3Att0133093%7D", "movie", 2000),
            200,
            &json!([found("The.Matrix.1999.1080p", Some(&"c".repeat(40)), 5)]),
        );
        let op = json!({ "op": "sources", "request": { "kind": "movie", "imdb_id": "tt0133093", "title": "The Matrix", "year": 1999 } });
        let out = serde_json::to_value(dispatch(&serde_json::to_vec(&op).unwrap(), &host).unwrap())
            .unwrap();
        assert_eq!(out["kind"], "releases");
        assert_eq!(out["value"][0]["info_hash"], "c".repeat(40));
    }
}
