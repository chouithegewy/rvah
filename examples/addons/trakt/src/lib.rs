//! Trakt add-on for RVAH (ABI v0.1 wasm guest).
//!
//! Rows from [Trakt](https://trakt.tv): your watchlist and recommendations
//! once linked, trending and most anticipated films and series always.
//! Items carry TMDB ids (`movie:<id>`, `tv:<id>`), so the TV opens them on
//! the TMDB add-on's details page. Sign-in is Trakt's device code flow with
//! only a client id (Trakt apps have no secret since 2026-10-01), and what
//! you play from a details page is scrobbled to your history.

pub mod trakt;

use rvah_addon_sdk::guest::{read_op, Host, Op, Response};
use rvah_addon_sdk::{AddonError, AddonErrorCode};

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn dispatch(input: &[u8], host: &dyn Host) -> Result<Response, AddonError> {
    match read_op(input)? {
        Op::List { path, cursor } => {
            trakt::list(host, path.as_deref(), cursor.as_deref()).map(Response::Page)
        }
        Op::AccountStatus => trakt::account(host).map(Response::Account),
        Op::AuthBegin => trakt::auth_begin(host).map(Response::DeviceAuth),
        Op::AuthPoll { device_code } => {
            trakt::auth_poll(host, &device_code).map(Response::AuthPoll)
        }
        Op::AuthRefresh => trakt::auth_refresh(host).map(Response::Credentials),
        Op::Scrobble {
            action,
            item,
            progress,
        } => trakt::scrobble(host, action, &item, progress).map(|()| Response::Done),
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
    use rvah_addon_sdk::{
        AccountStatus, AuthPoll, ContentType, PageItem, ScrobbleAction, ScrobbleItem, SourceKind,
    };
    use serde_json::{json, Value};

    const API: &str = "https://api.trakt.tv";
    const CLIENT: &str = "aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789_-abcde";
    const REDIRECT: &str = "https://rvah.example.com";

    fn host(signed_in: bool) -> ScriptedHost {
        let mut host = ScriptedHost::default();
        host.secrets.insert("client_id".into(), CLIENT.into());
        host.settings.insert("redirect_uri".into(), REDIRECT.into());
        if signed_in {
            host.secrets.insert("access_token".into(), "at1".into());
            host.secrets.insert("refresh_token".into(), "rt1".into());
        }
        host
    }

    fn media(title: &str, trakt: u64, tmdb: Option<u64>) -> Value {
        json!({
            "title": title, "year": 2026,
            "ids": { "trakt": trakt, "slug": title.to_lowercase(), "imdb": format!("tt{trakt:07}"), "tmdb": tmdb },
            "overview": format!("About {title}."), "runtime": 101, "rating": 7.8, "votes": 1234,
            "genres": ["comedy"],
            "images": {
                "poster": [format!("walter-r2.trakt.tv/images/movies/000/000/{trakt:03}/posters/thumb/p.jpg.webp")],
                "fanart": [format!("walter-r2.trakt.tv/images/movies/000/000/{trakt:03}/fanarts/medium/f.jpg.webp")],
                "logo": [], "clearart": [], "banner": [], "thumb": []
            }
        })
    }

    fn items(page: &rvah_addon_sdk::Page) -> Vec<(String, ContentType)> {
        page.items
            .iter()
            .map(|item| match item {
                PageItem::Media(m) => (m.id.clone(), m.content_type),
                other => panic!("expected media, got {other:?}"),
            })
            .collect()
    }

    fn rows(page: &rvah_addon_sdk::Page) -> Vec<String> {
        page.items
            .iter()
            .map(|item| match item {
                PageItem::Directory(d) => d.title.clone(),
                other => panic!("expected rows, got {other:?}"),
            })
            .collect()
    }

    #[test]
    fn your_rows_appear_once_signed_in() {
        let public = [
            "Trending films",
            "Trending series",
            "Most anticipated films",
            "Most anticipated series",
        ];
        assert_eq!(
            rows(&trakt::list(&host(false), None, None).unwrap()),
            public
        );
        let signed_in = rows(&trakt::list(&host(true), None, None).unwrap());
        assert_eq!(
            signed_in[..3],
            ["Your watchlist", "Recommended films", "Recommended series"]
        );
        assert_eq!(signed_in[3..], public);
    }

    #[test]
    fn trending_films_open_as_tmdb_items_with_trakt_posters() {
        let mut host = host(false);
        host.route_with_headers(
            &format!("{API}/movies/trending?extended=full&page=1&limit=20"),
            200,
            &[("X-Pagination-Page-Count", "5")],
            &json!([
                { "watchers": 40, "movie": media("Coyote vs. Acme", 12, Some(603)) },
                // No TMDB id: nothing could describe it.
                { "watchers": 30, "movie": media("Obscure", 13, None) },
            ]),
        );
        let page = trakt::list(&host, Some("movies/trending"), None).unwrap();
        assert_eq!(items(&page), [("movie:603".into(), ContentType::Movie)]);
        let PageItem::Media(m) = &page.items[0] else {
            unreachable!()
        };
        assert_eq!(m.title, "Coyote vs. Acme");
        assert_eq!(m.year, Some(2026));
        assert_eq!(m.rating, Some(7.8));
        assert_eq!(m.provider_ids["tmdb"], "603");
        assert_eq!(m.provider_ids["imdb"], "tt0000012");
        assert_eq!(
            m.artwork.poster.as_deref(),
            Some("https://walter-r2.trakt.tv/images/movies/000/000/012/posters/thumb/p.jpg.webp")
        );
        assert_eq!(page.cursor_next.as_deref(), Some("2"));
        let sent = &host.requests()[0];
        assert_eq!(sent.headers["trakt-api-key"], CLIENT);
        assert_eq!(sent.headers["trakt-api-version"], "2");
        assert!(
            !sent.headers.contains_key("authorization"),
            "public lists send no token"
        );
    }

    #[test]
    fn recommendations_come_unwrapped_and_page_by_count() {
        let mut host = host(true);
        let shows: Vec<Value> = (0..20)
            .map(|n| media(&format!("Show {n}"), 100 + n, Some(2000 + n)))
            .collect();
        host.route(
            &format!(
                "{API}/recommendations/shows?extended=full&page=1&limit=20&ignore_collected=true"
            ),
            200,
            &shows,
        );
        let page = trakt::list(&host, Some("recommendations/shows"), None).unwrap();
        assert_eq!(page.items.len(), 20);
        assert_eq!(items(&page)[0], ("tv:2000".into(), ContentType::Series));
        // A full page and no count header: there may be more.
        assert_eq!(page.cursor_next.as_deref(), Some("2"));
        assert_eq!(host.requests()[0].headers["authorization"], "Bearer at1");
    }

    #[test]
    fn the_watchlist_mixes_films_and_series() {
        let mut host = host(true);
        host.route(
            &format!("{API}/sync/watchlist?extended=full&page=1&limit=20"),
            200,
            &json!([
                { "rank": 1, "type": "movie", "movie": media("Film", 1, Some(10)) },
                { "rank": 2, "type": "show", "show": media("Series", 2, Some(20)) },
                { "rank": 3, "type": "season", "season": { "number": 2 }, "show": media("Other", 3, Some(30)) },
            ]),
        );
        let page = trakt::list(&host, Some("sync/watchlist"), None).unwrap();
        assert_eq!(
            items(&page),
            [
                ("movie:10".into(), ContentType::Movie),
                ("tv:20".into(), ContentType::Series),
                ("tv:30".into(), ContentType::Series),
            ]
        );
        assert_eq!(page.cursor_next, None, "a short page is the last");
    }

    #[test]
    fn your_rows_ask_for_a_sign_in() {
        let err = trakt::list(&host(false), Some("sync/watchlist"), None).unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Auth);
        assert!(err.user_visible);
        let err = trakt::list(&host(true), Some("users/me/history"), None).unwrap_err();
        assert_eq!(err.code, AddonErrorCode::NotFound);
    }

    #[test]
    fn device_sign_in_needs_only_the_client_id() {
        let mut host = host(false);
        host.route(
            &format!("{API}/oauth/device/code"),
            200,
            &json!({ "device_code": "dc1", "user_code": "ABCD1234", "verification_url": "https://trakt.tv/activate", "expires_in": 600, "interval": 5 }),
        );
        let begun = trakt::auth_begin(&host).unwrap();
        assert_eq!(begun.user_code, "ABCD1234");
        assert_eq!(begun.verification_url, "https://trakt.tv/activate");
        assert_eq!((begun.interval_secs, begun.expires_in_secs), (5, 600));
        let sent = &host.requests()[0];
        assert_eq!(sent.method, "POST");
        let body: Value = serde_json::from_slice(sent.body.as_ref().unwrap()).unwrap();
        assert_eq!(body, json!({ "client_id": CLIENT }));

        for (status, expected) in [
            (400, AuthPoll::Pending),
            (429, AuthPoll::Pending),
            (418, AuthPoll::Denied),
        ] {
            let mut host = self::host(false);
            host.route(&format!("{API}/oauth/device/token"), status, &json!({}));
            assert_eq!(
                trakt::auth_poll(&host, "dc1").unwrap(),
                expected,
                "{status}"
            );
        }
        let mut host = self::host(false);
        host.route(
            &format!("{API}/oauth/device/token"),
            200,
            &json!({ "access_token": "at9", "token_type": "bearer", "expires_in": 604800, "refresh_token": "rt9", "scope": "public", "created_at": 1 }),
        );
        let AuthPoll::Authorized(credentials) = trakt::auth_poll(&host, "dc1").unwrap() else {
            panic!("expected authorized");
        };
        assert_eq!(credentials.0["access_token"], "at9");
        assert_eq!(credentials.0["refresh_token"], "rt9");
        let body: Value =
            serde_json::from_slice(host.requests()[0].body.as_ref().unwrap()).unwrap();
        assert_eq!(body, json!({ "code": "dc1", "client_id": CLIENT }));

        let mut host = self::host(false);
        host.route(&format!("{API}/oauth/device/token"), 410, &json!({}));
        assert_eq!(
            trakt::auth_poll(&host, "dc1").unwrap_err().code,
            AddonErrorCode::Auth
        );
    }

    #[test]
    fn a_refresh_sends_the_refresh_token_and_redirect_uri() {
        let mut host = host(true);
        host.route(
            &format!("{API}/oauth/token"),
            200,
            &json!({ "access_token": "at2", "token_type": "bearer", "expires_in": 604800, "refresh_token": "rt2", "scope": "public", "created_at": 2 }),
        );
        let credentials = trakt::auth_refresh(&host).unwrap();
        assert_eq!(credentials.0["access_token"], "at2");
        assert_eq!(credentials.0["refresh_token"], "rt2");
        let body: Value =
            serde_json::from_slice(host.requests()[0].body.as_ref().unwrap()).unwrap();
        assert_eq!(
            body,
            json!({ "refresh_token": "rt1", "client_id": CLIENT, "redirect_uri": REDIRECT, "grant_type": "refresh_token" })
        );
    }

    #[test]
    fn account_status_names_the_user_or_asks_to_refresh() {
        assert_eq!(
            trakt::account(&host(false)).unwrap(),
            AccountStatus::SignedOut
        );
        let mut host = host(true);
        host.route(
            &format!("{API}/users/settings"),
            200,
            &json!({ "user": { "username": "chilos", "name": "David" } }),
        );
        assert_eq!(
            trakt::account(&host).unwrap(),
            AccountStatus::SignedIn {
                display_name: Some("David".into())
            }
        );
        let mut host = self::host(true);
        host.route(&format!("{API}/users/settings"), 401, &json!({}));
        assert_eq!(
            trakt::account(&host).unwrap_err().code,
            AddonErrorCode::Auth
        );
    }

    fn film() -> ScrobbleItem {
        ScrobbleItem {
            kind: SourceKind::Movie,
            tmdb_id: "603".into(),
            imdb_id: None,
            season: None,
            episode: None,
        }
    }

    #[test]
    fn scrobbles_name_a_film_or_an_episode_of_a_series() {
        let mut host = host(true);
        host.route(&format!("{API}/scrobble/start"), 201, &json!({}));
        host.route(&format!("{API}/scrobble/stop"), 201, &json!({}));
        trakt::scrobble(&host, ScrobbleAction::Start, &film(), 1.5).unwrap();
        let episode = ScrobbleItem {
            kind: SourceKind::Episode,
            tmdb_id: "1399".into(),
            imdb_id: Some("tt0944947".into()),
            season: Some(1),
            episode: Some(2),
        };
        trakt::scrobble(&host, ScrobbleAction::Stop, &episode, 92.0).unwrap();
        let bodies: Vec<Value> = host
            .requests()
            .iter()
            .map(|r| serde_json::from_slice(r.body.as_ref().unwrap()).unwrap())
            .collect();
        assert_eq!(
            bodies[0],
            json!({ "movie": { "ids": { "tmdb": 603 } }, "progress": 1.5 })
        );
        assert_eq!(
            bodies[1],
            json!({ "show": { "ids": { "tmdb": 1399 } }, "episode": { "season": 1, "number": 2 }, "progress": 92.0 })
        );
        assert_eq!(host.requests()[1].headers["authorization"], "Bearer at1");
    }

    #[test]
    fn scrobbling_stays_quiet_unless_the_sign_in_needs_refreshing() {
        // Not linked: nothing is sent, and nothing fails.
        let host = host(false);
        trakt::scrobble(&host, ScrobbleAction::Start, &film(), 0.0).unwrap();
        assert!(host.urls().is_empty());
        // Already scrobbled, or unknown to Trakt: fine.
        for status in [409, 404, 422, 500] {
            let mut host = self::host(true);
            host.route(&format!("{API}/scrobble/pause"), status, &json!({}));
            trakt::scrobble(&host, ScrobbleAction::Pause, &film(), 50.0).unwrap();
        }
        // An expired token: the host refreshes and tries again.
        let mut host = self::host(true);
        host.route(&format!("{API}/scrobble/pause"), 401, &json!({}));
        let err = trakt::scrobble(&host, ScrobbleAction::Pause, &film(), 50.0).unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Auth);
    }

    #[test]
    fn dispatch_speaks_the_wire_format() {
        let mut host = host(true);
        host.route(&format!("{API}/scrobble/start"), 201, &json!({}));
        let op = json!({ "op": "scrobble", "action": "start", "item": { "kind": "movie", "tmdb_id": "603" }, "progress": 3.0 });
        let out = serde_json::to_value(dispatch(&serde_json::to_vec(&op).unwrap(), &host).unwrap())
            .unwrap();
        assert_eq!(out, json!({ "kind": "done" }));
    }
}
