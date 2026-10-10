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

#[cfg_attr(
    any(not(target_arch = "wasm32"), not(feature = "standalone")),
    allow(dead_code)
)]
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

#[cfg(feature = "standalone")]
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

    const TODAY: u64 = 1_791_590_400; // 2026-10-10
    const AVAILABLE: &str =
        "watch_region=US&with_watch_monetization_types=flatrate%7Cfree%7Cads%7Crent%7Cbuy";

    fn root_rows(host: &ScriptedHost) -> Vec<(String, String)> {
        tmdb::list(host, None, None)
            .unwrap()
            .items
            .iter()
            .map(|item| match item {
                PageItem::Directory(d) => (d.title.clone(), d.path.clone()),
                other => panic!("expected rows, got {other:?}"),
            })
            .collect()
    }

    #[test]
    fn the_root_is_whats_streaming_then_your_genres() {
        let mut host = host_with(V3_KEY);
        host.settings
            .insert("genres".into(), "Sci-Fi,Anime,Thriller".into());
        assert_eq!(
            root_rows(&host),
            [
                ("New to streaming".into(), "streaming/new".into()),
                (
                    "Popular to stream or rent".into(),
                    "streaming/popular".into()
                ),
                ("Sci-Fi".into(), "genre/scifi".into()),
                ("Anime".into(), "genre/anime".into()),
                ("Thriller".into(), "genre/thriller".into()),
            ]
        );
        // Unknown names are skipped; no setting means the default genres.
        host.settings
            .insert("genres".into(), "Anime, Nonsense ,Horror".into());
        assert_eq!(
            root_rows(&host)[2..],
            [
                ("Anime".into(), "genre/anime".into()),
                ("Horror".into(), "genre/horror".into())
            ]
        );
        host.settings.remove("genres");
        assert!(root_rows(&host).len() > 8);
        assert!(host.urls().is_empty(), "the root costs no request");
    }

    #[test]
    fn new_to_streaming_is_the_last_two_months_of_digital_releases() {
        let mut host = host_with(V3_KEY);
        host.now = Some(TODAY + 3_600);
        host.route(
            &format!("https://api.themoviedb.org/3/discover/movie?api_key={V3_KEY}&language=en-US&with_release_type=4&release_date.gte=2026-08-11&release_date.lte=2026-10-10&region=US&sort_by=popularity.desc&vote_count.gte=10&include_adult=false&{AVAILABLE}&page=1"),
            200,
            &page(vec![movie(603, "Coyote vs. Acme", "2026-03-14")], 1, 4),
        );
        let new = tmdb::list(&host, Some("streaming/new"), None).unwrap();
        assert_eq!(media(&new.items[0]).id, "movie:603");
        assert_eq!(new.cursor_next.as_deref(), Some("2"));
    }

    #[test]
    fn popular_to_stream_or_rent_is_out_on_digital_or_disc() {
        let mut host = host_with(V3_KEY);
        host.now = Some(TODAY);
        host.route(
            &format!("https://api.themoviedb.org/3/discover/movie?api_key={V3_KEY}&language=en-US&with_release_type=4%7C5&release_date.lte=2026-10-10&region=US&sort_by=popularity.desc&vote_count.gte=100&include_adult=false&{AVAILABLE}&page=1"),
            200,
            &page(vec![movie(1, "Hit", "2025-01-01")], 1, 1),
        );
        let popular = tmdb::list(&host, Some("streaming/popular"), None).unwrap();
        assert_eq!(media(&popular.items[0]).id, "movie:1");
        assert_eq!(popular.cursor_next, None);
    }

    #[test]
    fn a_genre_column_interleaves_films_and_series_you_can_watch() {
        let mut host = host_with(V3_KEY);
        host.route(
            &format!("https://api.themoviedb.org/3/discover/movie?api_key={V3_KEY}&language=en-US&with_genres=16&with_original_language=ja&sort_by=popularity.desc&vote_count.gte=100&include_adult=false&{AVAILABLE}&page=1"),
            200,
            &page(vec![movie(1, "Film A", "2025-01-01"), movie(2, "Film B", "2024-01-01")], 1, 1),
        );
        host.route(
            &format!("https://api.themoviedb.org/3/discover/tv?api_key={V3_KEY}&language=en-US&with_genres=16&with_original_language=ja&sort_by=popularity.desc&vote_count.gte=50&include_adult=false&{AVAILABLE}&page=1"),
            200,
            &page(vec![series(10, "Show A", "2023-01-01")], 1, 3),
        );
        let anime = tmdb::list(&host, Some("genre/anime"), None).unwrap();
        let ids: Vec<&str> = anime.items.iter().map(|i| media(i).id.as_str()).collect();
        assert_eq!(ids, ["movie:1", "tv:10", "movie:2"]);
        // Only the series have more pages: the next asks only for series.
        assert_eq!(anime.cursor_next.as_deref(), Some("2t"));

        // A genre TMDB has only for films asks only for films.
        let mut host = host_with(V3_KEY);
        host.route(
            &format!("https://api.themoviedb.org/3/discover/movie?api_key={V3_KEY}&language=en-US&with_genres=53&sort_by=popularity.desc&vote_count.gte=100&include_adult=false&{AVAILABLE}&page=1"),
            200,
            &page(vec![movie(5, "Tense", "2022-01-01")], 1, 1),
        );
        assert_eq!(
            tmdb::list(&host, Some("genre/thriller"), None)
                .unwrap()
                .items
                .len(),
            1
        );
        assert_eq!(host.urls().len(), 1);
        assert_eq!(
            tmdb::list(&host, Some("genre/nonsense"), None)
                .unwrap_err()
                .code,
            AddonErrorCode::NotFound
        );
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

    fn show() -> Value {
        json!({
            "id": 1399, "name": "Acme Corp", "first_air_date": "2011-04-17",
            "poster_path": "/show.jpg", "backdrop_path": "/showb.jpg",
            "genres": [{ "id": 18, "name": "Drama" }],
            "external_ids": { "imdb_id": "tt0944947" },
            "seasons": [
                { "season_number": 0, "name": "Specials", "episode_count": 3, "poster_path": "/s0.jpg", "air_date": "2010-12-05" },
                { "season_number": 1, "name": "Season 1", "episode_count": 10, "poster_path": "/s1.jpg", "air_date": "2011-04-17" },
                { "season_number": 2, "name": "Season 2", "episode_count": 10, "poster_path": null, "air_date": "2012-04-01" }
            ]
        })
    }

    #[test]
    fn a_series_lists_its_seasons_specials_last() {
        let mut host = host_with(V3_KEY);
        host.route(
            &format!("https://api.themoviedb.org/3/tv/1399?api_key={V3_KEY}&language=en-US"),
            200,
            &show(),
        );
        let seasons = tmdb::list(&host, Some("tv:1399"), None).unwrap();
        let rows: Vec<(String, String, Option<String>)> = seasons
            .items
            .iter()
            .map(|item| match item {
                PageItem::Directory(d) => (d.title.clone(), d.path.clone(), d.subtitle.clone()),
                other => panic!("expected seasons, got {other:?}"),
            })
            .collect();
        assert_eq!(
            rows,
            [
                (
                    "Season 1".into(),
                    "tv:1399/season/1".into(),
                    Some("10 episodes · 2011".into())
                ),
                (
                    "Season 2".into(),
                    "tv:1399/season/2".into(),
                    Some("10 episodes · 2012".into())
                ),
                (
                    "Specials".into(),
                    "tv:1399/season/0".into(),
                    Some("3 episodes · 2010".into())
                ),
            ]
        );
    }

    #[test]
    fn a_season_lists_its_episodes() {
        let mut host = host_with(V3_KEY);
        host.route(
            &format!("https://api.themoviedb.org/3/tv/1399/season/1?api_key={V3_KEY}&language=en-US"),
            200,
            &json!({ "season_number": 1, "episodes": [
                { "episode_number": 1, "season_number": 1, "name": "Pilot", "overview": "It begins.",
                  "still_path": "/e1.jpg", "air_date": "2011-04-17", "runtime": 62, "vote_average": 8.1, "vote_count": 300 },
                { "episode_number": 2, "season_number": 1, "name": "The Next One", "overview": "",
                  "still_path": null, "air_date": "2011-04-24", "runtime": null, "vote_average": 0.0, "vote_count": 0 }
            ] }),
        );
        let episodes = tmdb::list(&host, Some("tv:1399/season/1"), None).unwrap();
        let first = media(&episodes.items[0]);
        assert_eq!(first.id, "tv:1399:s1e1");
        assert_eq!(first.title, "1. Pilot");
        assert_eq!((first.season, first.episode), (Some(1), Some(1)));
        assert_eq!(first.content_type, ContentType::Episode);
        assert_eq!(first.runtime_secs, Some(62 * 60));
        assert_eq!(
            first.artwork.poster.as_deref(),
            Some("https://image.tmdb.org/t/p/w300/e1.jpg")
        );
        assert_eq!(media(&episodes.items[1]).synopsis, None);
        assert_eq!(episodes.cursor_next, None);
    }

    #[test]
    fn an_episode_page_names_its_series_for_finding_sources() {
        let mut host = host_with(V3_KEY);
        host.route(
            &format!("https://api.themoviedb.org/3/tv/1399/season/1/episode/2?api_key={V3_KEY}&language=en-US"),
            200,
            &json!({ "episode_number": 2, "season_number": 1, "name": "The Next One",
                     "overview": "It continues.", "still_path": "/e2.jpg", "air_date": "2011-04-24",
                     "runtime": 56, "vote_average": 8.0, "vote_count": 120 }),
        );
        host.route(
            &format!("https://api.themoviedb.org/3/tv/1399?api_key={V3_KEY}&language=en-US&append_to_response=external_ids"),
            200,
            &show(),
        );
        let details = tmdb::details(&host, "tv:1399:s1e2").unwrap();
        assert_eq!(details.series_title.as_deref(), Some("Acme Corp"));
        assert_eq!(details.item.title, "The Next One");
        assert_eq!(details.item.content_type, ContentType::Episode);
        assert_eq!(
            (details.item.season, details.item.episode),
            (Some(1), Some(2))
        );
        assert_eq!(details.item.provider_ids["imdb"], "tt0944947");
        assert_eq!(details.item.runtime_secs, Some(56 * 60));
        assert_eq!(details.released.as_deref(), Some("2011-04-24"));
        assert_eq!(details.genres, ["Drama"]);
        assert_eq!(
            details.item.artwork.fanart.as_deref(),
            Some("https://image.tmdb.org/t/p/w780/e2.jpg")
        );
        assert_eq!(
            details.item.artwork.poster.as_deref(),
            Some("https://image.tmdb.org/t/p/w342/show.jpg")
        );
        assert_eq!(details.ratings[0].value, "8.0/10");
    }

    #[test]
    fn season_and_episode_ids_are_checked() {
        let host = host_with(V3_KEY);
        for path in [
            "tv:",
            "tv:13a",
            "tv:1/season/",
            "tv:1/season/x",
            "tv:1/episodes",
            "movie:1/season/1",
        ] {
            assert_eq!(
                tmdb::list(&host, Some(path), None).unwrap_err().code,
                AddonErrorCode::NotFound,
                "{path}"
            );
        }
        for id in [
            "tv:1:s1",
            "tv:1:e2",
            "tv:1:s1e",
            "tv:1:sxe2",
            "movie:1:s1e2",
        ] {
            assert_eq!(
                tmdb::details(&host, id).unwrap_err().code,
                AddonErrorCode::NotFound,
                "{id}"
            );
        }
        assert!(host.urls().is_empty());
    }

    #[test]
    fn a_genre_board_lists_tmdb_genres() {
        let mut host = host_with(V3_KEY);
        host.route(
            &format!(
                "https://api.themoviedb.org/3/genre/movie/list?api_key={V3_KEY}&language=en-US"
            ),
            200,
            &json!({ "genres": [{ "id": 28, "name": "Action" }, { "id": 35, "name": "Comedy" }] }),
        );
        let board = tmdb::list(&host, Some("genres/movie"), None).unwrap();
        let genres: Vec<(String, String)> = board
            .items
            .iter()
            .map(|item| match item {
                PageItem::Directory(d) => (d.title.clone(), d.path.clone()),
                other => panic!("expected genres, got {other:?}"),
            })
            .collect();
        assert_eq!(
            genres,
            [
                ("Action".into(), "genre/movie/28".into()),
                ("Comedy".into(), "genre/movie/35".into())
            ]
        );
    }

    #[test]
    fn a_genre_lists_its_popular_titles() {
        let mut host = host_with(V3_KEY);
        host.route(
            &format!("https://api.themoviedb.org/3/discover/tv?api_key={V3_KEY}&language=en-US&with_genres=18&sort_by=popularity.desc&vote_count.gte=50&include_adult=false&page=2"),
            200,
            &page(vec![series(1399, "Acme Corp", "2011-04-17")], 2, 9),
        );
        let titles = tmdb::list(&host, Some("genre/tv/18"), Some("2")).unwrap();
        let m = media(&titles.items[0]);
        assert_eq!(
            (m.id.as_str(), m.content_type),
            ("tv:1399", ContentType::Series)
        );
        assert_eq!(titles.cursor_next.as_deref(), Some("3"));
        for path in [
            "genre/tv/",
            "genre/tv/1a",
            "genre/person/1",
            "genres/person",
        ] {
            assert_eq!(
                tmdb::list(&host, Some(path), None).unwrap_err().code,
                AddonErrorCode::NotFound,
                "{path}"
            );
        }
    }
}
