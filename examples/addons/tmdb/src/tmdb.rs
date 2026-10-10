//! TMDB logic against the guest [`Host`], so it unit-tests natively with a
//! scripted host. Item ids are `movie:<tmdb id>` and `tv:<tmdb id>`.

use std::collections::BTreeMap;

use rvah_addon_sdk::guest::{self, form_encode, Host};
use rvah_addon_sdk::{
    AddonError, AddonErrorCode, Artwork, ContentType, DirectoryItem, ItemDetails, MediaItem, Page,
    PageItem, Rating, Trailer, VideoSite,
};
use serde::de::DeserializeOwned;
use serde::Deserialize;

const API: &str = "https://api.themoviedb.org/3";
const IMAGES: &str = "https://image.tmdb.org/t/p";
/// TMDB serves up to 500 pages of a list; nobody browses that far.
const MAX_PAGES: u32 = 50;
const MAX_TRAILERS: usize = 3;

/// The root rows: (title, list path, whether the list is per region).
const ROWS: &[(&str, &str, bool)] = &[
    ("Trending this week", "trending/all/week", false),
    ("In theaters", "movie/now_playing", true),
    ("Coming soon", "movie/upcoming", true),
    ("Popular movies", "movie/popular", false),
    ("Top rated movies", "movie/top_rated", false),
    ("New episodes this week", "tv/on_the_air", false),
    ("Popular series", "tv/popular", false),
    ("Top rated series", "tv/top_rated", false),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Movie,
    Tv,
}

impl Kind {
    fn from_media_type(media_type: &str) -> Option<Kind> {
        match media_type {
            "movie" => Some(Kind::Movie),
            "tv" => Some(Kind::Tv),
            _ => None,
        }
    }

    /// TMDB's path segment and our id prefix.
    fn segment(self) -> &'static str {
        match self {
            Kind::Movie => "movie",
            Kind::Tv => "tv",
        }
    }

    fn content_type(self) -> ContentType {
        match self {
            Kind::Movie => ContentType::Movie,
            Kind::Tv => ContentType::Series,
        }
    }
}

fn auth_error(message: &str) -> AddonError {
    let mut err = AddonError::new(AddonErrorCode::Auth, message);
    err.user_visible = true;
    err
}

fn not_found(message: String) -> AddonError {
    AddonError::new(AddonErrorCode::NotFound, message)
}

/// GET `path` with `params`, authenticated with the `tmdb_key` secret: a
/// read access token (a JWT, `eyJ…`) goes in the Authorization header, an
/// API key in the query.
fn get<T: DeserializeOwned>(
    host: &dyn Host,
    path: &str,
    params: &[(&str, &str)],
) -> Result<T, AddonError> {
    let key = guest::secret_get(host, "tmdb_key")?.ok_or_else(|| {
        auth_error("no TMDB key: set the tmdb_key secret (themoviedb.org/settings/api)")
    })?;
    let language = guest::setting_get(host, "language")?.unwrap_or_else(|| "en-US".into());
    let mut headers = BTreeMap::new();
    let mut query: Vec<(&str, &str)> = Vec::new();
    if key.starts_with("eyJ") {
        headers.insert("authorization".to_string(), format!("Bearer {key}"));
    } else {
        query.push(("api_key", &key));
    }
    query.push(("language", &language));
    query.extend_from_slice(params);
    let url = format!("{API}/{path}?{}", form_encode(&query));
    let resp = guest::http_get(host, &url, headers)?;
    match resp.status {
        200..=299 => serde_json::from_slice(&resp.body)
            .map_err(|e| AddonError::new(AddonErrorCode::Malformed, format!("TMDB {path}: {e}"))),
        401 => Err(auth_error("TMDB rejected the key (401): check tmdb_key")),
        404 => Err(not_found(format!("TMDB has no {path}"))),
        429 => {
            let mut err = AddonError::new(AddonErrorCode::Quota, "TMDB rate limit; try again");
            err.retryable = true;
            Err(err)
        }
        status => Err(AddonError::new(
            AddonErrorCode::Network,
            format!("TMDB answered {status} for {path}"),
        )),
    }
}

/// One result of a TMDB list or search (fields we use).
#[derive(Deserialize)]
struct Entry {
    id: u64,
    #[serde(default)]
    media_type: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    first_air_date: Option<String>,
    #[serde(default)]
    poster_path: Option<String>,
    #[serde(default)]
    backdrop_path: Option<String>,
    #[serde(default)]
    vote_average: Option<f32>,
    #[serde(default)]
    vote_count: Option<u32>,
}

#[derive(Deserialize)]
struct Listing {
    page: u32,
    total_pages: u32,
    results: Vec<Entry>,
}

fn non_empty(text: Option<String>) -> Option<String> {
    text.filter(|t| !t.trim().is_empty())
}

fn year(date: Option<&str>) -> Option<u16> {
    date?.get(..4)?.parse().ok()
}

fn image(size: &str, path: Option<String>) -> Option<String> {
    path.filter(|p| p.starts_with('/'))
        .map(|p| format!("{IMAGES}/{size}{p}"))
}

/// A score only when people voted.
fn score(average: Option<f32>, votes: Option<u32>) -> Option<f32> {
    average.filter(|_| votes.unwrap_or(0) > 0)
}

fn media_item(kind: Kind, entry: Entry) -> MediaItem {
    let date = match kind {
        Kind::Movie => entry.release_date,
        Kind::Tv => entry.first_air_date,
    };
    MediaItem {
        id: format!("{}:{}", kind.segment(), entry.id),
        content_type: kind.content_type(),
        title: entry.title.or(entry.name).unwrap_or_default(),
        year: year(date.as_deref()),
        season: None,
        episode: None,
        runtime_secs: None,
        synopsis: non_empty(entry.overview),
        rating: score(entry.vote_average, entry.vote_count),
        artwork: Artwork {
            poster: image("w342", entry.poster_path),
            fanart: image("w780", entry.backdrop_path),
            thumb: None,
        },
        provider_ids: BTreeMap::from([("tmdb".to_string(), entry.id.to_string())]),
    }
}

/// A page of results; `kind` is the list's kind, or `None` for mixed lists
/// whose entries say (`media_type`), where people are left out.
fn to_page(listing: Listing, kind: Option<Kind>) -> Page {
    let next = (listing.page < listing.total_pages.min(MAX_PAGES)).then(|| listing.page + 1);
    Page {
        items: listing
            .results
            .into_iter()
            .filter_map(|entry| {
                let kind = kind.or_else(|| Kind::from_media_type(entry.media_type.as_deref()?))?;
                Some(PageItem::Media(media_item(kind, entry)))
            })
            .collect(),
        cursor_next: next.map(|page| page.to_string()),
        total_hint: None,
    }
}

fn page_number(cursor: Option<&str>) -> Result<String, AddonError> {
    match cursor {
        None => Ok("1".into()),
        Some(c) if c.parse::<u32>().is_ok_and(|n| n >= 1) => Ok(c.to_string()),
        Some(_) => Err(AddonError::new(AddonErrorCode::Malformed, "bad cursor")),
    }
}

/// The root is the rows; a row's path lists it. Only the rows can be
/// listed, so a path never reaches TMDB's URL unchecked.
pub fn list(host: &dyn Host, path: Option<&str>, cursor: Option<&str>) -> Result<Page, AddonError> {
    let Some(path) = path.filter(|p| *p != "/") else {
        let mut items = streaming_rows();
        items.extend(genre_rows(host)?);
        return Ok(Page {
            items,
            cursor_next: None,
            total_hint: None,
        });
    };
    match path {
        "streaming/new" => return new_to_streaming(host, cursor),
        "streaming/popular" => return popular_to_stream(host, cursor),
        _ => {}
    }
    // Genres: TMDB's for films or series (genre boards), or a column of
    // the root mixing both.
    if let Some(kind) = path.strip_prefix("genres/") {
        return genres(host, media_kind(kind)?);
    }
    if let Some(rest) = path.strip_prefix("genre/") {
        return match rest.split_once('/') {
            Some((kind, id)) => genre(host, media_kind(kind)?, digits(id)?, cursor),
            None => genre_column(host, rest, cursor),
        };
    }
    // A series lists its seasons; a season, its episodes.
    if let Some(series) = path.strip_prefix("tv:") {
        return match series.split_once("/season/") {
            None => seasons(host, digits(series)?),
            Some((id, number)) => episodes(host, digits(id)?, digits(number)?),
        };
    }
    let &(_, path, regional) = ROWS
        .iter()
        .find(|(_, row, _)| *row == path)
        .ok_or_else(|| not_found(format!("no such row: {path}")))?;
    let page = page_number(cursor)?;
    let region = guest::setting_get(host, "region")?.unwrap_or_else(|| "US".into());
    let mut params = vec![("page", page.as_str())];
    if regional {
        params.push(("region", &region));
    }
    let listing: Listing = get(host, path, &params)?;
    let kind = match path.split('/').next() {
        Some("movie") => Some(Kind::Movie),
        Some("tv") => Some(Kind::Tv),
        _ => None,
    };
    Ok(to_page(listing, kind))
}

/// Films and series whose title matches `query`.
pub fn search(host: &dyn Host, query: &str, cursor: Option<&str>) -> Result<Page, AddonError> {
    let page = page_number(cursor)?;
    let listing: Listing = get(
        host,
        "search/multi",
        &[
            ("query", query),
            ("page", &page),
            ("include_adult", "false"),
        ],
    )?;
    Ok(to_page(listing, None))
}

#[derive(Deserialize)]
struct Genre {
    name: String,
}

#[derive(Deserialize)]
struct Video {
    name: String,
    site: String,
    key: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    official: bool,
}

#[derive(Deserialize, Default)]
struct Videos {
    results: Vec<Video>,
}

#[derive(Deserialize, Default)]
struct ExternalIds {
    #[serde(default)]
    imdb_id: Option<String>,
}

/// `/movie/{id}` or `/tv/{id}` with videos appended (fields we use).
#[derive(Deserialize)]
struct Detailed {
    #[serde(flatten)]
    entry: Entry,
    #[serde(default)]
    tagline: Option<String>,
    #[serde(default)]
    genres: Vec<Genre>,
    #[serde(default)]
    runtime: Option<u32>,
    #[serde(default)]
    episode_run_time: Vec<u32>,
    #[serde(default)]
    imdb_id: Option<String>,
    #[serde(default)]
    external_ids: ExternalIds,
    #[serde(default)]
    videos: Videos,
}

fn parse_id(item_id: &str) -> Result<(Kind, &str), AddonError> {
    let parsed = item_id.split_once(':').and_then(|(kind, id)| {
        let kind = Kind::from_media_type(kind)?;
        (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then_some((kind, id))
    });
    parsed.ok_or_else(|| not_found(format!("not a TMDB item: {item_id}")))
}

/// YouTube trailers and teasers: official trailers first, then trailers,
/// then teasers.
fn trailers(videos: Vec<Video>) -> Vec<Trailer> {
    let mut picked: Vec<Video> = videos
        .into_iter()
        .filter(|v| v.site == "YouTube" && (v.kind == "Trailer" || v.kind == "Teaser"))
        .collect();
    picked.sort_by_key(|v| (v.kind != "Trailer", !v.official));
    picked
        .into_iter()
        .take(MAX_TRAILERS)
        .map(|v| Trailer {
            name: v.name,
            site: VideoSite::YouTube,
            key: v.key,
        })
        .collect()
}

/// A details page: TMDB's details and trailers, then ratings from MDBList
/// when an `mdblist_key` is set. MDBList failing costs only its ratings.
pub fn details(host: &dyn Host, item_id: &str) -> Result<ItemDetails, AddonError> {
    // An episode: tv:<id>:s<season>e<episode>.
    if let Some((id, numbers)) = item_id
        .strip_prefix("tv:")
        .and_then(|rest| rest.split_once(':'))
    {
        let (season, episode) = numbers
            .strip_prefix('s')
            .and_then(|rest| rest.split_once('e'))
            .ok_or_else(|| not_found(format!("not a TMDB episode: {item_id}")))?;
        return episode_details(
            host,
            item_id,
            digits(id)?,
            digits(season)?,
            digits(episode)?,
        );
    }
    let (kind, id) = parse_id(item_id)?;
    let append = match kind {
        // Films carry their IMDb id at the top level.
        Kind::Movie => "videos",
        Kind::Tv => "videos,external_ids",
    };
    let detailed: Detailed = get(
        host,
        &format!("{}/{id}", kind.segment()),
        &[
            ("append_to_response", append),
            ("include_video_language", "en,null"),
        ],
    )?;
    let runtime_mins = detailed
        .runtime
        .or_else(|| detailed.episode_run_time.first().copied());
    let imdb = detailed
        .imdb_id
        .or(detailed.external_ids.imdb_id)
        .filter(|i| !i.is_empty());
    let tmdb_score = score(detailed.entry.vote_average, detailed.entry.vote_count);
    let released = match kind {
        Kind::Movie => detailed.entry.release_date.clone(),
        Kind::Tv => detailed.entry.first_air_date.clone(),
    };
    let mut item = media_item(kind, detailed.entry);
    item.runtime_secs = runtime_mins.filter(|m| *m > 0).map(|m| m * 60);
    if let Some(imdb) = imdb {
        item.provider_ids.insert("imdb".into(), imdb);
    }
    let mut ratings = mdblist_ratings(host, kind, id);
    if let Some(score) = tmdb_score {
        ratings.push(Rating {
            source: "tmdb".into(),
            value: format!("{score:.1}/10"),
        });
    }
    Ok(ItemDetails {
        item,
        series_title: None,
        tagline: non_empty(detailed.tagline),
        genres: detailed.genres.into_iter().map(|g| g.name).collect(),
        released: non_empty(released),
        trailers: trailers(detailed.videos.results),
        ratings,
    })
}

#[derive(Deserialize)]
struct MdbRating {
    source: String,
    #[serde(default)]
    value: Option<f64>,
}

#[derive(Deserialize)]
struct MdbMedia {
    #[serde(default)]
    ratings: Vec<MdbRating>,
}

/// How a rating prints: `93%`, `7.8/10`.
type Format = fn(f64) -> String;

/// MDBList's sources we show, in order: (its name, ours, how to print).
const MDBLIST_SOURCES: &[(&str, &str, Format)] = &[
    ("tomatoes", "rotten_tomatoes", |v| format!("{v:.0}%")),
    ("popcorn", "rt_audience", |v| format!("{v:.0}%")),
    ("imdb", "imdb", |v| format!("{v:.1}/10")),
    ("metacritic", "metacritic", |v| format!("{v:.0}/100")),
    ("letterboxd", "letterboxd", |v| format!("{v:.1}/5")),
];

fn mdblist_ratings(host: &dyn Host, kind: Kind, id: &str) -> Vec<Rating> {
    let Ok(Some(key)) = guest::secret_get(host, "mdblist_key") else {
        return Vec::new();
    };
    let kind = match kind {
        Kind::Movie => "movie",
        Kind::Tv => "show",
    };
    let url = format!(
        "https://api.mdblist.com/tmdb/{kind}/{id}?{}",
        form_encode(&[("apikey", &key)])
    );
    let media: MdbMedia = match guest::http_get(host, &url, BTreeMap::new()) {
        Ok(resp) if resp.status == 200 => match serde_json::from_slice(&resp.body) {
            Ok(media) => media,
            Err(e) => {
                guest::log(host, &format!("MDBList {kind}/{id}: {e}"));
                return Vec::new();
            }
        },
        Ok(resp) => {
            guest::log(host, &format!("MDBList {kind}/{id}: {}", resp.status));
            return Vec::new();
        }
        Err(e) => {
            guest::log(host, &format!("MDBList {kind}/{id}: {}", e.message));
            return Vec::new();
        }
    };
    MDBLIST_SOURCES
        .iter()
        .filter_map(|(theirs, ours, show)| {
            let value = media
                .ratings
                .iter()
                .find(|r| r.source == *theirs)?
                .value
                .filter(|v| *v > 0.0)?;
            Some(Rating {
                source: ours.to_string(),
                value: show(value),
            })
        })
        .collect()
}

/// `text` when it is a plain number (an id, a season), so it can go into a
/// TMDB path unchecked.
fn digits(text: &str) -> Result<&str, AddonError> {
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        Ok(text)
    } else {
        Err(not_found(format!("not a TMDB number: {text}")))
    }
}

#[derive(Deserialize)]
struct SeasonInfo {
    season_number: u32,
    name: String,
    #[serde(default)]
    episode_count: Option<u32>,
    #[serde(default)]
    poster_path: Option<String>,
    #[serde(default)]
    air_date: Option<String>,
}

/// `/tv/{id}` (fields we use).
#[derive(Deserialize)]
struct Show {
    name: String,
    #[serde(default)]
    poster_path: Option<String>,
    #[serde(default)]
    genres: Vec<Genre>,
    #[serde(default)]
    external_ids: ExternalIds,
    #[serde(default)]
    seasons: Vec<SeasonInfo>,
}

#[derive(Deserialize)]
struct Episode {
    episode_number: u32,
    season_number: u32,
    name: String,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    still_path: Option<String>,
    #[serde(default)]
    air_date: Option<String>,
    #[serde(default)]
    runtime: Option<u32>,
    #[serde(default)]
    vote_average: Option<f32>,
    #[serde(default)]
    vote_count: Option<u32>,
}

#[derive(Deserialize)]
struct Season {
    episodes: Vec<Episode>,
}

/// A series' seasons in order, specials (season 0) last.
fn seasons(host: &dyn Host, id: &str) -> Result<Page, AddonError> {
    let mut show: Show = get(host, &format!("tv/{id}"), &[])?;
    show.seasons
        .sort_by_key(|s| (s.season_number == 0, s.season_number));
    Ok(Page {
        items: show
            .seasons
            .into_iter()
            .map(|season| {
                let count = season
                    .episode_count
                    .map(|n| format!("{n} episode{}", if n == 1 { "" } else { "s" }));
                let subtitle = [
                    count,
                    year(season.air_date.as_deref()).map(|y| y.to_string()),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
                let path = format!("tv:{id}/season/{}", season.season_number);
                PageItem::Directory(DirectoryItem {
                    id: path.clone(),
                    title: season.name,
                    subtitle: (!subtitle.is_empty()).then_some(subtitle),
                    artwork: Artwork {
                        poster: image("w342", season.poster_path),
                        fanart: None,
                        thumb: None,
                    },
                    path,
                    of_directories: false,
                    badges: vec![],
                })
            })
            .collect(),
        cursor_next: None,
        total_hint: None,
    })
}

fn episode_item(series: &str, episode: Episode) -> MediaItem {
    MediaItem {
        id: format!(
            "tv:{series}:s{}e{}",
            episode.season_number, episode.episode_number
        ),
        content_type: ContentType::Episode,
        title: episode.name,
        year: year(episode.air_date.as_deref()),
        season: Some(episode.season_number),
        episode: Some(episode.episode_number),
        runtime_secs: episode.runtime.filter(|m| *m > 0).map(|m| m * 60),
        synopsis: non_empty(episode.overview),
        rating: score(episode.vote_average, episode.vote_count),
        artwork: Artwork {
            poster: image("w300", episode.still_path.clone()),
            fanart: image("w780", episode.still_path),
            thumb: None,
        },
        provider_ids: BTreeMap::from([("tmdb".to_string(), series.to_string())]),
    }
}

/// A season's episodes, titled with their number.
fn episodes(host: &dyn Host, id: &str, season: &str) -> Result<Page, AddonError> {
    let season: Season = get(host, &format!("tv/{id}/season/{season}"), &[])?;
    Ok(Page {
        items: season
            .episodes
            .into_iter()
            .map(|episode| {
                let number = episode.episode_number;
                let mut item = episode_item(id, episode);
                item.title = format!("{number}. {}", item.title);
                PageItem::Media(item)
            })
            .collect(),
        cursor_next: None,
        total_hint: None,
    })
}

/// An episode's page, with its series' name and IMDb id, which finding
/// sources for it needs.
fn episode_details(
    host: &dyn Host,
    item_id: &str,
    id: &str,
    season: &str,
    number: &str,
) -> Result<ItemDetails, AddonError> {
    let episode: Episode = get(
        host,
        &format!("tv/{id}/season/{season}/episode/{number}"),
        &[],
    )?;
    let show: Show = get(
        host,
        &format!("tv/{id}"),
        &[("append_to_response", "external_ids")],
    )?;
    let tmdb_score = score(episode.vote_average, episode.vote_count);
    let released = non_empty(episode.air_date.clone());
    let mut item = episode_item(id, episode);
    item.id = item_id.to_string();
    item.artwork.poster = image("w342", show.poster_path);
    if let Some(imdb) = show.external_ids.imdb_id.filter(|i| !i.is_empty()) {
        item.provider_ids.insert("imdb".into(), imdb);
    }
    Ok(ItemDetails {
        item,
        series_title: Some(show.name),
        tagline: None,
        genres: show.genres.into_iter().map(|g| g.name).collect(),
        released,
        trailers: vec![],
        ratings: tmdb_score
            .map(|score| Rating {
                source: "tmdb".into(),
                value: format!("{score:.1}/10"),
            })
            .into_iter()
            .collect(),
    })
}

fn folder(title: &str, path: &str, of_directories: bool) -> PageItem {
    PageItem::Directory(DirectoryItem {
        id: path.to_string(),
        title: title.to_string(),
        subtitle: None,
        artwork: Artwork::default(),
        path: path.to_string(),
        of_directories,
        badges: vec![],
    })
}

fn media_kind(segment: &str) -> Result<Kind, AddonError> {
    Kind::from_media_type(segment)
        .ok_or_else(|| not_found(format!("not films or series: {segment}")))
}

#[derive(Deserialize)]
struct Genres {
    genres: Vec<GenreId>,
}

#[derive(Deserialize)]
struct GenreId {
    id: u32,
    name: String,
}

/// TMDB's genres for films or series, each a folder of its titles.
fn genres(host: &dyn Host, kind: Kind) -> Result<Page, AddonError> {
    let genres: Genres = get(host, &format!("genre/{}/list", kind.segment()), &[])?;
    Ok(Page {
        items: genres
            .genres
            .into_iter()
            .map(|g| {
                folder(
                    &g.name,
                    &format!("genre/{}/{}", kind.segment(), g.id),
                    false,
                )
            })
            .collect(),
        cursor_next: None,
        total_hint: None,
    })
}

/// A genre's popular titles, leaving out those too few people rated.
fn genre(host: &dyn Host, kind: Kind, id: &str, cursor: Option<&str>) -> Result<Page, AddonError> {
    let page = page_number(cursor)?;
    let min_votes = match kind {
        Kind::Movie => "100",
        Kind::Tv => "50",
    };
    let listing: Listing = get(
        host,
        &format!("discover/{}", kind.segment()),
        &[
            ("with_genres", id),
            ("sort_by", "popularity.desc"),
            ("vote_count.gte", min_votes),
            ("include_adult", "false"),
            ("page", &page),
        ],
    )?;
    Ok(to_page(listing, Some(kind)))
}

/// The first columns: films out to stream, newest and most popular.
const STREAMING_ROWS: &[(&str, &str)] = &[
    ("New to streaming", "streaming/new"),
    ("Popular to stream or rent", "streaming/popular"),
];

/// A genre column: its name in the `genres` setting, the TMDB genre of
/// films and of series it covers, and an original language. TMDB has no
/// anime genre: anime is animation in Japanese.
struct GenreColumn {
    name: &'static str,
    films: Option<&'static str>,
    series: Option<&'static str>,
    language: Option<&'static str>,
}

const GENRES: &[GenreColumn] = &[
    GenreColumn {
        name: "Sci-Fi",
        films: Some("878"),
        series: Some("10765"),
        language: None,
    },
    GenreColumn {
        name: "Anime",
        films: Some("16"),
        series: Some("16"),
        language: Some("ja"),
    },
    GenreColumn {
        name: "Thriller",
        films: Some("53"),
        series: None,
        language: None,
    },
    GenreColumn {
        name: "Action",
        films: Some("28"),
        series: Some("10759"),
        language: None,
    },
    GenreColumn {
        name: "Comedy",
        films: Some("35"),
        series: Some("35"),
        language: None,
    },
    GenreColumn {
        name: "Horror",
        films: Some("27"),
        series: None,
        language: None,
    },
    GenreColumn {
        name: "Drama",
        films: Some("18"),
        series: Some("18"),
        language: None,
    },
    GenreColumn {
        name: "Mystery",
        films: Some("9648"),
        series: Some("9648"),
        language: None,
    },
    GenreColumn {
        name: "Crime",
        films: Some("80"),
        series: Some("80"),
        language: None,
    },
    GenreColumn {
        name: "Fantasy",
        films: Some("14"),
        series: Some("10765"),
        language: None,
    },
    GenreColumn {
        name: "Animation",
        films: Some("16"),
        series: Some("16"),
        language: None,
    },
    GenreColumn {
        name: "Documentary",
        films: Some("99"),
        series: Some("99"),
        language: None,
    },
    GenreColumn {
        name: "Romance",
        films: Some("10749"),
        series: None,
        language: None,
    },
    GenreColumn {
        name: "Family",
        films: Some("10751"),
        series: Some("10751"),
        language: None,
    },
    GenreColumn {
        name: "War",
        films: Some("10752"),
        series: Some("10768"),
        language: None,
    },
    GenreColumn {
        name: "Western",
        films: Some("37"),
        series: Some("37"),
        language: None,
    },
    GenreColumn {
        name: "History",
        films: Some("36"),
        series: None,
        language: None,
    },
    GenreColumn {
        name: "Music",
        films: Some("10402"),
        series: None,
        language: None,
    },
    GenreColumn {
        name: "Kids",
        films: None,
        series: Some("10762"),
        language: None,
    },
    GenreColumn {
        name: "Reality",
        films: None,
        series: Some("10764"),
        language: None,
    },
];

/// Used until the `genres` setting says otherwise.
const DEFAULT_GENRES: &str =
    "Sci-Fi,Anime,Thriller,Action,Comedy,Horror,Drama,Mystery,Crime,Fantasy,Documentary,Romance";

/// How many days "new to streaming" reaches back.
const NEW_DAYS: u64 = 60;

/// A genre's column path: its name, lowercase letters and digits only.
fn slug(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The streaming columns, for the root (and an add-on that nests this one).
pub fn streaming_rows() -> Vec<PageItem> {
    STREAMING_ROWS
        .iter()
        .map(|(title, path)| folder(title, path, false))
        .collect()
}

/// A column per genre in the `genres` setting, in its order.
pub fn genre_rows(host: &dyn Host) -> Result<Vec<PageItem>, AddonError> {
    let chosen = guest::setting_get(host, "genres")?.unwrap_or_else(|| DEFAULT_GENRES.into());
    let mut seen = Vec::new();
    Ok(chosen
        .split(',')
        .map(str::trim)
        .filter_map(|name| GENRES.iter().find(|g| g.name.eq_ignore_ascii_case(name)))
        .filter(|g| {
            let fresh = !seen.contains(&g.name);
            seen.push(g.name);
            fresh
        })
        .map(|g| folder(g.name, &format!("genre/{}", slug(g.name)), false))
        .collect())
}

fn region(host: &dyn Host) -> Result<String, AddonError> {
    Ok(guest::setting_get(host, "region")?.unwrap_or_else(|| "US".into()))
}

/// GET a discover list with `params`, then only what some streaming,
/// rental or purchase service in the region has, then `page`.
fn discover(
    host: &dyn Host,
    kind: Kind,
    params: &[(&str, &str)],
    page: &str,
) -> Result<Listing, AddonError> {
    let region = region(host)?;
    let mut all: Vec<(&str, &str)> = params.to_vec();
    all.push(("watch_region", &region));
    all.push((
        "with_watch_monetization_types",
        "flatrate|free|ads|rent|buy",
    ));
    all.push(("page", page));
    get(host, &format!("discover/{}", kind.segment()), &all)
}

/// Films released to streaming (digitally) in the last two months, in the
/// region, most popular first: never theatrical-only or upcoming.
fn new_to_streaming(host: &dyn Host, cursor: Option<&str>) -> Result<Page, AddonError> {
    let page = page_number(cursor)?;
    let region = region(host)?;
    let now = guest::now_unix(host);
    let from = now.map(|t| guest::civil_date(t.saturating_sub(NEW_DAYS * 86_400)));
    let to = now.map(guest::civil_date);
    let mut params = vec![("with_release_type", "4")];
    if let (Some(from), Some(to)) = (&from, &to) {
        params.push(("release_date.gte", from));
        params.push(("release_date.lte", to));
    }
    params.extend([
        ("region", region.as_str()),
        ("sort_by", "popularity.desc"),
        ("vote_count.gte", "10"),
        ("include_adult", "false"),
    ]);
    Ok(to_page(
        discover(host, Kind::Movie, &params, &page)?,
        Some(Kind::Movie),
    ))
}

/// Popular films already out digitally or on disc, so there to stream or rent.
fn popular_to_stream(host: &dyn Host, cursor: Option<&str>) -> Result<Page, AddonError> {
    let page = page_number(cursor)?;
    let region = region(host)?;
    let to = guest::now_unix(host).map(guest::civil_date);
    let mut params = vec![("with_release_type", "4|5")];
    if let Some(to) = &to {
        params.push(("release_date.lte", to));
    }
    params.extend([
        ("region", region.as_str()),
        ("sort_by", "popularity.desc"),
        ("vote_count.gte", "100"),
        ("include_adult", "false"),
    ]);
    Ok(to_page(
        discover(host, Kind::Movie, &params, &page)?,
        Some(Kind::Movie),
    ))
}

/// A genre's popular films and series you can watch, interleaved. The
/// cursor is the next page, ending in `m` or `t` when only films or only
/// series have more.
fn genre_column(host: &dyn Host, name: &str, cursor: Option<&str>) -> Result<Page, AddonError> {
    let &GenreColumn {
        films,
        series,
        language,
        ..
    } = GENRES
        .iter()
        .find(|g| slug(g.name) == name)
        .ok_or_else(|| not_found(format!("no such genre: {name}")))?;
    let (page, only) = match cursor {
        None => ("1".to_string(), None),
        Some(c) => {
            let (number, only) = if let Some(rest) = c.strip_suffix('m') {
                (rest, Some(Kind::Movie))
            } else if let Some(rest) = c.strip_suffix('t') {
                (rest, Some(Kind::Tv))
            } else {
                (c, None)
            };
            (page_number(Some(number))?, only)
        }
    };
    let fetch = |kind: Kind, genre: Option<&str>| -> Result<Option<Listing>, AddonError> {
        let Some(genre) = genre else { return Ok(None) };
        if only.is_some_and(|k| k != kind) {
            return Ok(None);
        }
        let min_votes = if kind == Kind::Movie { "100" } else { "50" };
        let mut params = vec![("with_genres", genre)];
        if let Some(language) = language {
            params.push(("with_original_language", language));
        }
        params.extend([
            ("sort_by", "popularity.desc"),
            ("vote_count.gte", min_votes),
            ("include_adult", "false"),
        ]);
        discover(host, kind, &params, &page).map(Some)
    };
    let films = fetch(Kind::Movie, films)?;
    let series = fetch(Kind::Tv, series)?;
    let more = |listing: &Option<Listing>| {
        listing
            .as_ref()
            .is_some_and(|l| l.page < l.total_pages.min(MAX_PAGES))
    };
    let next = page.parse::<u32>().unwrap_or(1) + 1;
    let cursor_next = match (more(&films), more(&series)) {
        (true, true) => Some(next.to_string()),
        (true, false) => Some(format!("{next}m")),
        (false, true) => Some(format!("{next}t")),
        (false, false) => None,
    };
    let mut films = films
        .map(|l| to_page(l, Some(Kind::Movie)).items)
        .unwrap_or_default()
        .into_iter();
    let mut series = series
        .map(|l| to_page(l, Some(Kind::Tv)).items)
        .unwrap_or_default()
        .into_iter();
    let mut items = Vec::new();
    loop {
        match (films.next(), series.next()) {
            (None, None) => break,
            (a, b) => items.extend(a.into_iter().chain(b)),
        }
    }
    Ok(Page {
        items,
        cursor_next,
        total_hint: None,
    })
}
