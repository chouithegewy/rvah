//! RVAH Discover (ABI v0.1 wasm guest): one board from two add-ons nested
//! as components, the TMDB add-on and the Trakt add-on, each compiled in
//! as a library (without its own wasm entry points).
//!
//! The root's columns, left to right: what is new to streaming and what is
//! popular to stream or rent (TMDB), what you might like (Trakt, once
//! linked), then your genres (TMDB, the `genres` setting). Search, details,
//! seasons and episodes are TMDB's; sign-in, account and scrobbling are
//! Trakt's. A Trakt path is mounted under `trakt/`.
//!
//! This product uses the TMDB API but is not endorsed or certified by TMDB.

use rvah_addon_sdk::guest::{read_op, Host, Op, Response};
use rvah_addon_sdk::{AddonError, AddonErrorCode, Page, PageItem};
use rvah_addon_tmdb::tmdb;
use rvah_addon_trakt::trakt;

/// Where the Trakt component's paths are mounted.
const TRAKT: &str = "trakt/";

fn root(host: &dyn Host) -> Result<Page, AddonError> {
    let mut items = tmdb::streaming_rows();
    if let Some(PageItem::Directory(mut row)) = trakt::you_might_like(host)? {
        row.path = format!("{TRAKT}{}", row.path);
        row.id = row.path.clone();
        items.push(PageItem::Directory(row));
    }
    items.extend(tmdb::genre_rows(host)?);
    Ok(Page {
        items,
        cursor_next: None,
        total_hint: None,
    })
}

fn list(host: &dyn Host, path: Option<&str>, cursor: Option<&str>) -> Result<Page, AddonError> {
    match path.filter(|p| *p != "/") {
        None => root(host),
        Some(path) => match path.strip_prefix(TRAKT) {
            Some(rest) => trakt::list(host, Some(rest), cursor),
            None => tmdb::list(host, Some(path), cursor),
        },
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn dispatch(input: &[u8], host: &dyn Host) -> Result<Response, AddonError> {
    match read_op(input)? {
        Op::List { path, cursor } => {
            list(host, path.as_deref(), cursor.as_deref()).map(Response::Page)
        }
        Op::Search { query, cursor, .. } => {
            tmdb::search(host, &query, cursor.as_deref()).map(Response::Page)
        }
        Op::Details { item_id } => {
            tmdb::details(host, &item_id).map(|details| Response::Details(Box::new(details)))
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
    use serde_json::{json, Value};

    const V3_KEY: &str = "0123456789abcdef0123456789abcdef";

    fn host(linked: bool) -> ScriptedHost {
        let mut host = ScriptedHost::default();
        host.secrets.insert("tmdb_key".into(), V3_KEY.into());
        host.secrets
            .insert("client_id".into(), "trakt-client".into());
        if linked {
            host.secrets.insert("access_token".into(), "at1".into());
            host.secrets.insert("refresh_token".into(), "rt1".into());
        }
        host.settings.insert("genres".into(), "Anime,Sci-Fi".into());
        host
    }

    fn columns(page: &Page) -> Vec<(String, String)> {
        page.items
            .iter()
            .map(|item| match item {
                PageItem::Directory(d) => (d.title.clone(), d.path.clone()),
                other => panic!("expected columns, got {other:?}"),
            })
            .collect()
    }

    #[test]
    fn the_columns_run_streaming_then_trakt_then_your_genres() {
        assert_eq!(
            columns(&list(&host(true), None, None).unwrap()),
            [
                ("New to streaming".into(), "streaming/new".into()),
                (
                    "Popular to stream or rent".into(),
                    "streaming/popular".into()
                ),
                ("You might like".into(), "trakt/recommendations".into()),
                ("Anime".into(), "genre/anime".into()),
                ("Sci-Fi".into(), "genre/scifi".into()),
            ]
        );
        // Not linked to Trakt: no column of yours.
        let unlinked = columns(&list(&host(false), None, None).unwrap());
        assert!(unlinked.iter().all(|(title, _)| title != "You might like"));
        assert_eq!(unlinked.len(), 4);
    }

    #[test]
    fn paths_reach_the_component_that_lists_them() {
        let mut host = host(true);
        host.route(
            "https://api.trakt.tv/recommendations/movies?extended=full&page=1&limit=10&ignore_collected=true",
            200,
            &json!([]),
        );
        host.route(
            "https://api.trakt.tv/recommendations/shows?extended=full&page=1&limit=10&ignore_collected=true",
            200,
            &json!([]),
        );
        list(&host, Some("trakt/recommendations"), None).unwrap();
        host.route(
            &format!("https://api.themoviedb.org/3/tv/1399?api_key={V3_KEY}&language=en-US"),
            200,
            &json!({ "name": "Acme Corp", "seasons": [] }),
        );
        list(&host, Some("tv:1399"), None).unwrap();
        let hosts: Vec<String> = host
            .urls()
            .iter()
            .map(|u| u.split('/').nth(2).unwrap().to_string())
            .collect();
        assert_eq!(
            hosts,
            ["api.trakt.tv", "api.trakt.tv", "api.themoviedb.org"]
        );
    }

    #[test]
    fn operations_go_to_their_component() {
        let mut host = host(true);
        host.route("https://api.trakt.tv/scrobble/start", 201, &json!({}));
        let op = json!({ "op": "scrobble", "action": "start", "item": { "kind": "movie", "tmdb_id": "603" }, "progress": 2.0 });
        let out: Value =
            serde_json::to_value(dispatch(&serde_json::to_vec(&op).unwrap(), &host).unwrap())
                .unwrap();
        assert_eq!(out, json!({ "kind": "done" }));
        assert_eq!(host.urls(), ["https://api.trakt.tv/scrobble/start"]);

        let err = dispatch(br#"{"op":"details","item_id":"person:1"}"#, &host).unwrap_err();
        assert_eq!(err.code, AddonErrorCode::NotFound, "details are TMDB's");
        let err = dispatch(
            br#"{"op":"resolve","item_id":"movie:1","source_id":null}"#,
            &host,
        )
        .unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Internal);
    }
}
