//! TMDB Discover add-on for RVAH (ABI v0.1 wasm guest).
//!
//! Browse what is trending, in theaters, coming soon and popular on
//! [TMDB](https://www.themoviedb.org), search it, and open a details page
//! with genres, trailers and ratings (Rotten Tomatoes and IMDb through
//! MDBList when an `mdblist_key` is set). Nothing here is playable: the TV
//! finds a title in your other add-ons, such as Real-Debrid.
//!
//! This product uses the TMDB API but is not endorsed or certified by TMDB.

pub mod tmdb;

use rvah_addon_sdk::guest::{read_op, Host, Op, Response};
use rvah_addon_sdk::{AddonError, AddonErrorCode};

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn dispatch(input: &[u8], host: &dyn Host) -> Result<Response, AddonError> {
    match read_op(input)? {
        Op::List { path, cursor } => {
            tmdb::list(host, path.as_deref(), cursor.as_deref()).map(Response::Page)
        }
        Op::Search { query, cursor, .. } => {
            tmdb::search(host, &query, cursor.as_deref()).map(Response::Page)
        }
        Op::Details { item_id } => {
            tmdb::details(host, &item_id).map(|details| Response::Details(Box::new(details)))
        }
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
    use rvah_addon_sdk::{ContentType, PageItem};
    use serde_json::{json, Value};

    const V3_KEY: &str = "0123456789abcdef0123456789abcdef";
    const V4_TOKEN: &str = "eyJhbGciOiJIUzI1NiJ9.eyJhdWQiOiJ4In0.sig";

    fn host_with(key: &str) -> ScriptedHost {
        let mut host = ScriptedHost::default();
        host.secrets.insert("tmdb_key".into(), key.into());
        host.settings.insert("region".into(), "US".into());
        host.settings.insert("language".into(), "en-US".into());
        host
    }

    fn movie(id: u64, title: &str, date: &str) -> Value {
        json!({
            "adult": false, "backdrop_path": format!("/b{id}.jpg"), "genre_ids": [35],
            "id": id, "original_language": "en", "original_title": title,
            "overview": format!("About {title}."), "popularity": 99.5,
            "poster_path": format!("/p{id}.jpg"), "release_date": date, "title": title,
            "video": false, "vote_average": 7.4, "vote_count": 1200
        })
    }

    fn series(id: u64, name: &str, date: &str) -> Value {
        json!({
            "backdrop_path": null, "id": id, "name": name, "original_name": name,
            "overview": "", "poster_path": format!("/s{id}.jpg"), "first_air_date": date,
            "vote_average": 0.0, "vote_count": 0, "origin_country": ["US"]
        })
    }

    fn page(results: Vec<Value>, page: u32, total_pages: u32) -> Value {
        json!({ "page": page, "results": results, "total_pages": total_pages, "total_results": 999 })
    }

    fn media(item: &PageItem) -> &rvah_addon_sdk::MediaItem {
        match item {
            PageItem::Media(m) => m,
            other => panic!("expected media, got {other:?}"),
        }
    }

    #[test]
    fn the_root_lists_the_rows() {
        let host = host_with(V3_KEY);
        let root = tmdb::list(&host, None, None).unwrap();
        let rows: Vec<(String, String)> = root
            .items
            .iter()
            .map(|item| match item {
                PageItem::Directory(d) => (d.title.clone(), d.path.clone()),
                other => panic!("expected rows, got {other:?}"),
            })
            .collect();
        assert_eq!(
            rows[0],
            ("Trending this week".into(), "trending/all/week".into())
        );
        assert!(rows.contains(&("In theaters".into(), "movie/now_playing".into())));
        assert!(rows.contains(&("Coming soon".into(), "movie/upcoming".into())));
        assert!(rows.contains(&("New episodes this week".into(), "tv/on_the_air".into())));
        assert!(host.urls().is_empty(), "the root costs no request");
    }

    #[test]
    fn a_row_page_becomes_media_with_artwork_and_a_next_page() {
        let mut host = host_with(V3_KEY);
        host.route(
            &format!("https://api.themoviedb.org/3/movie/now_playing?api_key={V3_KEY}&language=en-US&page=1&region=US"),
            200,
            &page(vec![movie(603, "Coyote vs. Acme", "2026-08-28")], 1, 3),
        );
        let listing = tmdb::list(&host, Some("movie/now_playing"), None).unwrap();
        let m = media(&listing.items[0]);
        assert_eq!(m.id, "movie:603");
        assert_eq!(m.title, "Coyote vs. Acme");
        assert_eq!(m.year, Some(2026));
        assert_eq!(m.content_type, ContentType::Movie);
        assert_eq!(m.synopsis.as_deref(), Some("About Coyote vs. Acme."));
        assert_eq!(m.rating, Some(7.4));
        assert_eq!(
            m.artwork.poster.as_deref(),
            Some("https://image.tmdb.org/t/p/w342/p603.jpg")
        );
        assert_eq!(
            m.artwork.fanart.as_deref(),
            Some("https://image.tmdb.org/t/p/w780/b603.jpg")
        );
        assert_eq!(m.provider_ids["tmdb"], "603");
        assert_eq!(listing.cursor_next.as_deref(), Some("2"));
    }

    #[test]
    fn a_read_access_token_is_sent_as_a_bearer_header() {
        let mut host = host_with(V4_TOKEN);
        host.route(
            "https://api.themoviedb.org/3/tv/popular?language=en-US&page=2",
            200,
            &page(vec![series(1399, "Acme Corp", "2011-04-17")], 2, 2),
        );
        let listing = tmdb::list(&host, Some("tv/popular"), Some("2")).unwrap();
        let m = media(&listing.items[0]);
        assert_eq!(
            (m.id.as_str(), m.content_type),
            ("tv:1399", ContentType::Series)
        );
        // No votes: no rating; no backdrop: no fanart.
        assert_eq!((m.rating, m.artwork.fanart.as_deref()), (None, None));
        assert_eq!(listing.cursor_next, None, "last page");
        let sent = &host.requests()[0];
        assert_eq!(sent.headers["authorization"], format!("Bearer {V4_TOKEN}"));
        assert!(!sent.url.contains("api_key"));
    }

    #[test]
    fn trending_mixes_films_and_series_and_skips_people() {
        let mut host = host_with(V3_KEY);
        let mut film = movie(1, "Film", "2026-01-02");
        film["media_type"] = json!("movie");
        let mut show = series(2, "Show", "2025-05-06");
        show["media_type"] = json!("tv");
        let person = json!({ "id": 3, "name": "Someone", "media_type": "person" });
        host.route(
            &format!("https://api.themoviedb.org/3/trending/all/week?api_key={V3_KEY}&language=en-US&page=1"),
            200,
            &page(vec![film, person, show], 1, 1),
        );
        let listing = tmdb::list(&host, Some("trending/all/week"), None).unwrap();
        let ids: Vec<&str> = listing.items.iter().map(|i| media(i).id.as_str()).collect();
        assert_eq!(ids, ["movie:1", "tv:2"]);
    }

    #[test]
    fn only_the_rows_can_be_listed() {
        let host = host_with(V3_KEY);
        for path in ["movie/603", "../account", "trending/all/week?x=1"] {
            let err = tmdb::list(&host, Some(path), None).unwrap_err();
            assert_eq!(err.code, AddonErrorCode::NotFound, "{path}");
        }
        assert!(host.urls().is_empty());
    }

    #[test]
    fn search_covers_films_and_series() {
        let mut host = host_with(V3_KEY);
        let mut film = movie(9, "Acme Returns", "");
        film["media_type"] = json!("movie");
        host.route(
            &format!("https://api.themoviedb.org/3/search/multi?api_key={V3_KEY}&language=en-US&query=acme%20%26%20co&page=1&include_adult=false"),
            200,
            &page(vec![film], 1, 1),
        );
        let found = tmdb::search(&host, "acme & co", None).unwrap();
        let m = media(&found.items[0]);
        assert_eq!((m.id.as_str(), m.year), ("movie:9", None));
    }

    fn movie_details() -> Value {
        json!({
            "id": 603, "title": "Coyote vs. Acme", "overview": "A coyote sues.",
            "tagline": "Beep beep.", "release_date": "2026-08-28", "runtime": 91,
            "genres": [{ "id": 35, "name": "Comedy" }, { "id": 16, "name": "Animation" }],
            "poster_path": "/p603.jpg", "backdrop_path": "/b603.jpg",
            "vote_average": 7.4, "vote_count": 1200, "imdb_id": "tt0000603",
            "videos": { "results": [
                { "name": "Behind the scenes", "site": "YouTube", "key": "bts12345678", "type": "Featurette", "official": true },
                { "name": "Teaser", "site": "YouTube", "key": "teaser12345", "type": "Teaser", "official": true },
                { "name": "Fan cut", "site": "YouTube", "key": "fancut12345", "type": "Trailer", "official": false },
                { "name": "Vimeo trailer", "site": "Vimeo", "key": "123456", "type": "Trailer", "official": true },
                { "name": "Official Trailer", "site": "YouTube", "key": "official123", "type": "Trailer", "official": true }
            ] }
        })
    }

    fn details_url(key: &str) -> String {
        format!("https://api.themoviedb.org/3/movie/603?api_key={key}&language=en-US&append_to_response=videos&include_video_language=en%2Cnull")
    }

    #[test]
    fn details_bring_genres_trailers_and_the_tmdb_score() {
        let mut host = host_with(V3_KEY);
        host.route(&details_url(V3_KEY), 200, &movie_details());
        let details = tmdb::details(&host, "movie:603").unwrap();
        assert_eq!(details.item.title, "Coyote vs. Acme");
        assert_eq!(details.item.runtime_secs, Some(91 * 60));
        assert_eq!(details.item.year, Some(2026));
        assert_eq!(details.item.provider_ids["imdb"], "tt0000603");
        assert_eq!(details.tagline.as_deref(), Some("Beep beep."));
        assert_eq!(details.genres, ["Comedy", "Animation"]);
        assert_eq!(details.released.as_deref(), Some("2026-08-28"));
        let trailers: Vec<(&str, &str)> = details
            .trailers
            .iter()
            .map(|t| (t.name.as_str(), t.key.as_str()))
            .collect();
        // YouTube trailers and teasers only; official trailers first.
        assert_eq!(
            trailers,
            [
                ("Official Trailer", "official123"),
                ("Fan cut", "fancut12345"),
                ("Teaser", "teaser12345")
            ]
        );
        let ratings: Vec<(&str, &str)> = details
            .ratings
            .iter()
            .map(|r| (r.source.as_str(), r.value.as_str()))
            .collect();
        assert_eq!(ratings, [("tmdb", "7.4/10")]);
        // Without an MDBList key, nothing is asked of MDBList.
        assert_eq!(host.urls().len(), 1);
    }

    #[test]
    fn mdblist_adds_rotten_tomatoes_and_imdb() {
        let mut host = host_with(V3_KEY);
        host.secrets.insert("mdblist_key".into(), "mdb-key".into());
        host.route(&details_url(V3_KEY), 200, &movie_details());
        host.route(
            "https://api.mdblist.com/tmdb/movie/603?apikey=mdb-key",
            200,
            &json!({
                "title": "Coyote vs. Acme",
                "ratings": [
                    { "source": "imdb", "value": 7.8, "score": 78, "votes": 51000 },
                    { "source": "metacritic", "value": 81, "score": 81, "votes": 40 },
                    { "source": "tomatoes", "value": 93, "score": 93, "votes": 210 },
                    { "source": "popcorn", "value": 88, "score": 88, "votes": 5000 },
                    { "source": "letterboxd", "value": 3.9, "score": 78, "votes": 9000 },
                    { "source": "tmdb", "value": 74, "score": 74, "votes": 1200 },
                    { "source": "trakt", "value": null, "score": null, "votes": 0 }
                ]
            }),
        );
        let details = tmdb::details(&host, "movie:603").unwrap();
        let ratings: Vec<(&str, &str)> = details
            .ratings
            .iter()
            .map(|r| (r.source.as_str(), r.value.as_str()))
            .collect();
        assert_eq!(
            ratings,
            [
                ("rotten_tomatoes", "93%"),
                ("rt_audience", "88%"),
                ("imdb", "7.8/10"),
                ("metacritic", "81/100"),
                ("letterboxd", "3.9/5"),
                ("tmdb", "7.4/10"),
            ]
        );
    }

    #[test]
    fn an_mdblist_failure_only_costs_its_ratings() {
        let mut host = host_with(V3_KEY);
        host.secrets.insert("mdblist_key".into(), "mdb-key".into());
        host.route(&details_url(V3_KEY), 200, &movie_details());
        host.route(
            "https://api.mdblist.com/tmdb/movie/603?apikey=mdb-key",
            429,
            &json!({ "error": "limit" }),
        );
        let details = tmdb::details(&host, "movie:603").unwrap();
        assert_eq!(details.ratings.len(), 1);
    }

    #[test]
    fn series_details_use_the_tv_endpoints() {
        let mut host = host_with(V3_KEY);
        host.secrets.insert("mdblist_key".into(), "mdb-key".into());
        host.route(
            &format!("https://api.themoviedb.org/3/tv/1399?api_key={V3_KEY}&language=en-US&append_to_response=videos%2Cexternal_ids&include_video_language=en%2Cnull"),
            200,
            &json!({
                "id": 1399, "name": "Acme Corp", "overview": "", "first_air_date": "2011-04-17",
                "episode_run_time": [55], "genres": [], "tagline": "",
                "vote_average": 8.4, "vote_count": 20000,
                "external_ids": { "imdb_id": "tt0944947" },
                "videos": { "results": [] }
            }),
        );
        host.route(
            "https://api.mdblist.com/tmdb/show/1399?apikey=mdb-key",
            200,
            &json!({ "ratings": [{ "source": "tomatoes", "value": 89 }] }),
        );
        let details = tmdb::details(&host, "tv:1399").unwrap();
        assert_eq!(details.item.content_type, ContentType::Series);
        assert_eq!(details.item.runtime_secs, Some(55 * 60));
        assert_eq!(details.item.provider_ids["imdb"], "tt0944947");
        assert_eq!(details.tagline, None, "empty strings are no tagline");
        assert_eq!(details.item.synopsis, None);
        assert_eq!(details.ratings[0].value, "89%");
    }

    #[test]
    fn malformed_ids_are_refused_without_a_request() {
        let host = host_with(V3_KEY);
        for id in ["603", "movie:", "movie:6a", "person:1", "tv:1/videos"] {
            let err = tmdb::details(&host, id).unwrap_err();
            assert_eq!(err.code, AddonErrorCode::NotFound, "{id}");
        }
        assert!(host.urls().is_empty());
    }

    #[test]
    fn a_missing_or_rejected_key_asks_for_one() {
        let host = ScriptedHost::default();
        let err = tmdb::list(&host, Some("movie/popular"), None).unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Auth);
        assert!(err.user_visible && err.message.contains("tmdb_key"));

        let mut host = host_with(V3_KEY);
        host.route(
            &format!(
                "https://api.themoviedb.org/3/movie/popular?api_key={V3_KEY}&language=en-US&page=1"
            ),
            401,
            &json!({ "status_code": 7, "status_message": "Invalid API key" }),
        );
        let err = tmdb::list(&host, Some("movie/popular"), None).unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Auth);
    }

    #[test]
    fn dispatch_speaks_the_wire_format() {
        let mut host = host_with(V3_KEY);
        host.route(&details_url(V3_KEY), 200, &movie_details());
        let out = serde_json::to_value(
            dispatch(br#"{"op":"details","item_id":"movie:603"}"#, &host).unwrap(),
        )
        .unwrap();
        assert_eq!(out["kind"], "details");
        assert_eq!(out["value"]["trailers"][0]["site"], "youtube");
        let err = dispatch(
            br#"{"op":"resolve","item_id":"movie:603","source_id":null}"#,
            &host,
        )
        .unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Internal);
    }
}
