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
        return Ok(Page {
            items: ROWS
                .iter()
                .map(|(title, path, _)| {
                    PageItem::Directory(DirectoryItem {
                        id: path.to_string(),
                        title: title.to_string(),
                        subtitle: None,
                        artwork: Artwork::default(),
                        path: path.to_string(),
                        badges: vec![],
                    })
                })
                .collect(),
            cursor_next: None,
            total_hint: None,
        });
    };
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
type Show = fn(f64) -> String;

/// MDBList's sources we show, in order: (its name, ours, how to print).
const MDBLIST_SOURCES: &[(&str, &str, Show)] = &[
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
