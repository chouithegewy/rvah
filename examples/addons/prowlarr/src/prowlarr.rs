//! Prowlarr's search API against the guest [`Host`], so it unit-tests
//! natively with a scripted host.

use std::collections::BTreeMap;

use rvah_addon_sdk::guest::{self, form_encode, Host};
use rvah_addon_sdk::{AddonError, AddonErrorCode, Release, SourceKind, SourceRequest};
use serde::Deserialize;

/// Results asked of Prowlarr per search. A result is about 1.5 KB of JSON,
/// and a call's fuel bounds how much it can read.
const LIMIT: &str = "75";
/// Releases handed back, after filtering and sorting.
const KEEP: usize = 40;
const MOVIES: &str = "2000";
const TV: &str = "5000";

/// One search result (fields we use).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Found {
    title: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    seeders: Option<u32>,
    #[serde(default)]
    indexer: Option<String>,
    #[serde(default)]
    info_hash: Option<String>,
    #[serde(default)]
    magnet_url: Option<String>,
    #[serde(default)]
    protocol: Option<String>,
}

fn auth_error(message: &str) -> AddonError {
    let mut err = AddonError::new(AddonErrorCode::Auth, message);
    err.user_visible = true;
    err
}

/// The configured address, `https://host[:port]` with no trailing slash.
fn base_url(host: &dyn Host) -> Result<String, AddonError> {
    let url = guest::setting_get(host, "url")?.unwrap_or_default();
    let url = url.trim().trim_end_matches('/');
    if !url.starts_with("https://") {
        let mut err = AddonError::new(
            AddonErrorCode::Malformed,
            "the Prowlarr address must start with https://",
        );
        err.user_visible = true;
        return Err(err);
    }
    Ok(url.to_string())
}

fn search(
    host: &dyn Host,
    query: &str,
    kind: &str,
    categories: &str,
) -> Result<Vec<Found>, AddonError> {
    let key = guest::secret_get(host, "api_key")?.ok_or_else(|| {
        auth_error("no Prowlarr API key: set the api_key secret (Prowlarr: Settings > General)")
    })?;
    let url = format!(
        "{}/api/v1/search?{}",
        base_url(host)?,
        form_encode(&[
            ("query", query),
            ("type", kind),
            ("categories", categories),
            ("limit", LIMIT),
        ])
    );
    let headers = BTreeMap::from([("x-api-key".to_string(), key)]);
    let resp = guest::http_get(host, &url, headers)?;
    match resp.status {
        200..=299 => serde_json::from_slice(&resp.body).map_err(|e| {
            AddonError::new(AddonErrorCode::Malformed, format!("Prowlarr results: {e}"))
        }),
        401 => Err(auth_error(
            "Prowlarr rejected the API key (401): check api_key",
        )),
        status => Err(AddonError::new(
            AddonErrorCode::Network,
            format!("Prowlarr answered {status}"),
        )),
    }
}

/// Releases of `request`: by IMDb id first, then by title if that finds
/// nothing (not every indexer searches by id).
pub fn sources(host: &dyn Host, request: &SourceRequest) -> Result<Vec<Release>, AddonError> {
    let (kind, categories, season) = match request.kind {
        SourceKind::Movie => ("movie", MOVIES, None),
        SourceKind::Episode => ("tvsearch", TV, request.season),
    };
    let mut found = Vec::new();
    if let Some(imdb) = request.imdb_id.as_deref().filter(|id| plain_id(id)) {
        let mut query = format!("{{ImdbId:{imdb}}}");
        if let Some(season) = season {
            query.push_str(&format!("{{Season:{season}}}"));
        }
        found = search(host, &query, kind, categories)?;
    }
    if found.is_empty() {
        let query = match (request.kind, request.year, season) {
            (SourceKind::Episode, _, Some(season)) => format!("{} S{season:02}", request.title),
            (_, Some(year), _) => format!("{} {year}", request.title),
            _ => request.title.clone(),
        };
        found = search(host, &query, "search", categories)?;
    }
    let mut releases: Vec<Release> = found
        .into_iter()
        .filter(|f| {
            f.protocol
                .as_deref()
                .is_none_or(|p| p.eq_ignore_ascii_case("torrent"))
        })
        .filter(|f| match (request.kind, request.season, request.episode) {
            (SourceKind::Episode, Some(s), Some(e)) => of_episode(&f.title, s, e),
            _ => true,
        })
        .filter_map(|f| {
            let info_hash = f
                .info_hash
                .filter(|h| !h.is_empty())
                .or_else(|| f.magnet_url.as_deref().and_then(magnet_hash))?;
            Some(Release {
                name: f.title,
                info_hash,
                size_bytes: f.size,
                seeders: f.seeders,
                origin: f.indexer,
            })
        })
        .collect();
    releases.sort_by_key(|r| {
        (
            std::cmp::Reverse(resolution(&r.name)),
            std::cmp::Reverse(r.seeders.unwrap_or(0)),
        )
    });
    releases.truncate(KEEP);
    Ok(releases)
}

fn plain_id(id: &str) -> bool {
    id.strip_prefix("tt")
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// The info hash of a `magnet:` link; Prowlarr's own download links are not.
fn magnet_hash(url: &str) -> Option<String> {
    let query = url.strip_prefix("magnet:?")?;
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("xt=urn:btih:"))
        .map(str::to_string)
}

fn words(title: &str) -> Vec<String> {
    title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// 4 for 2160p, 3 for 1080p, 2 for 720p, 1 for SD, 0 when unsaid.
fn resolution(title: &str) -> u8 {
    let words = words(title);
    let has = |options: &[&str]| words.iter().any(|w| options.contains(&w.as_str()));
    if has(&["2160p", "4k", "uhd"]) {
        4
    } else if has(&["1080p", "1080i"]) {
        3
    } else if has(&["720p"]) {
        2
    } else if has(&["480p", "576p", "sd", "dvdrip"]) {
        1
    } else {
        0
    }
}

/// `S01E02` / `1x02` / `S01E01E02` for this episode, or a pack of its
/// season (`S01`, `Season 1`) that names no episode.
fn of_episode(title: &str, season: u32, episode: u32) -> bool {
    let words = words(title);
    let mut pack = false;
    for (i, word) in words.iter().enumerate() {
        // Before the `S01` form, which "season" would otherwise match.
        if word == "season" {
            if words.get(i + 1).and_then(|n| n.parse().ok()) == Some(season) {
                pack = true;
            }
            continue;
        }
        if let Some(rest) = word.strip_prefix('s') {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 || digits > 2 || rest[..digits].parse() != Ok(season) {
                continue;
            }
            let mut rest = &rest[digits..];
            if rest.is_empty() {
                pack = true;
                continue;
            }
            while let Some(after) = rest.strip_prefix('e') {
                let n = after.bytes().take_while(u8::is_ascii_digit).count();
                if n == 0 {
                    break;
                }
                if after[..n].parse() == Ok(episode) {
                    return true;
                }
                rest = &after[n..];
            }
            // Some other episode of the season.
            return false;
        } else if let Some((s, e)) = word.split_once('x') {
            if s.parse() == Ok(season) && !e.is_empty() && e.bytes().all(|b| b.is_ascii_digit()) {
                return e.parse() == Ok(episode);
            }
        }
    }
    pack
}
