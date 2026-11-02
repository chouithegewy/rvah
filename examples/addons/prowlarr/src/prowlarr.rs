//! Prowlarr's search API against the guest [`Host`], so it unit-tests
//! natively with a scripted host.

use std::collections::BTreeMap;

use rvah_addon_sdk::guest::{self, form_encode, Host};
use rvah_addon_sdk::{AddonError, AddonErrorCode, Release, SourceKind, SourceRequest};
use serde::Deserialize;

/// Results asked of Prowlarr per search; most indexers ignore it.
const LIMIT: &str = "75";
/// How much of each search's answer is read. A call's fuel reads about
/// 190 KB of answer (some 260 fuel a byte, mostly the trip from host to
/// guest), but a title search for a popular film can answer a megabyte:
/// about 3.5 KB a result, mostly the magnet link's trackers. These leave a
/// third of the fuel to spare. Prowlarr lists results indexer by indexer,
/// so a cut drops the last indexers it lists.
pub const ID_ANSWER_BYTES: u64 = 32 * 1024;
pub const TITLE_ANSWER_BYTES: u64 = 96 * 1024;
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
    max_bytes: u64,
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
    let resp = guest::http_get_capped(host, &url, headers, max_bytes)?;
    match resp.status {
        200..=299 => {
            let whole = if resp.truncated {
                whole_results(&resp.body)
            } else {
                resp.body
            };
            serde_json::from_slice(&whole).map_err(|e| {
                AddonError::new(AddonErrorCode::Malformed, format!("Prowlarr results: {e}"))
            })
        }
        401 => Err(auth_error(
            "Prowlarr rejected the API key (401): check api_key",
        )),
        status => Err(AddonError::new(
            AddonErrorCode::Network,
            format!("Prowlarr answered {status}"),
        )),
    }
}

/// The whole results at the start of a JSON array cut short, as an array:
/// `[{…},{…},{…` becomes `[{…},{…}]`.
fn whole_results(body: &[u8]) -> Vec<u8> {
    let (mut depth, mut in_string, mut escaped) = (0u32, false, false);
    let mut end = None;
    for (i, &b) in body.iter().enumerate() {
        if in_string {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                // A result (depth 2, inside the array) just closed.
                if depth == 2 && b == b'}' {
                    end = Some(i);
                }
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    let mut whole = match end {
        Some(end) => body[..=end].to_vec(),
        None => b"[".to_vec(),
    };
    whole.push(b']');
    whole
}

/// Releases of `request`, by IMDb id and by title. Only some indexers search
/// by id (of the usual public ones, YTS), so the id alone misses the rest;
/// the title reaches them all, but finds other films too, so its results
/// must name this one.
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
        found = search(host, &query, kind, categories, ID_ANSWER_BYTES)?;
    }
    let query = match (request.kind, request.year, season) {
        (SourceKind::Episode, _, Some(season)) => format!("{} S{season:02}", request.title),
        (_, Some(year), _) => format!("{} {year}", request.title),
        _ => request.title.clone(),
    };
    let by_title = search(host, &query, "search", categories, TITLE_ANSWER_BYTES)?;
    found.extend(
        by_title
            .into_iter()
            .filter(|f| names_title(&f.title, request)),
    );
    let mut seen = std::collections::HashSet::new();
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
        // Both searches, and several indexers, find the same torrents.
        .filter(|r| seen.insert(r.info_hash.to_ascii_lowercase()))
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

/// Whether the release `name` is of `request`'s title: its words in a row
/// (or run together, as in `Spiderman`), and for a film, its year (or the
/// next or last, as dates differ), or no year besides the title's own.
fn names_title(name: &str, request: &SourceRequest) -> bool {
    let title = words(&request.title);
    let name = words(name);
    // Words are ASCII; a title that is not (`Amélie`) cannot be matched
    // word for word, so only its year is checked.
    let ascii = request.title.is_ascii() && !title.is_empty();
    if ascii {
        let in_a_row = name.windows(title.len()).any(|w| w == title.as_slice());
        let joined = title.concat();
        let run_together = joined.len() >= 6 && name.concat().contains(&joined);
        if !in_a_row && !run_together {
            return false;
        }
    }
    let Some(year) = request.year.filter(|_| request.kind == SourceKind::Movie) else {
        return true;
    };
    let years: Vec<u16> = name
        .iter()
        .filter(|w| w.len() == 4 && !title.contains(w))
        .filter_map(|w| w.parse().ok())
        .filter(|y| (1900..2100).contains(y))
        .collect();
    years.is_empty() || years.iter().any(|y| y.abs_diff(year) <= 1)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(body: &[u8]) -> Vec<String> {
        let found: Vec<Found> = serde_json::from_slice(&whole_results(body)).unwrap();
        found.into_iter().map(|f| f.title).collect()
    }

    #[test]
    fn a_cut_answer_keeps_its_whole_results() {
        // Braces and quotes inside strings and nested objects don't count.
        let body = br#"[{"title":"a","categories":[{"id":1,"name":"x}"}]},{"title":"b \"}\""},{"title":"c","categories":[{"id":"#;
        assert_eq!(titles(body), ["a", "b \"}\""]);
        assert!(titles(br#"[{"title":"a"#).is_empty());
        assert!(titles(b"").is_empty());
    }

    fn film(title: &str, year: u16) -> SourceRequest {
        SourceRequest {
            kind: SourceKind::Movie,
            imdb_id: None,
            title: title.into(),
            year: Some(year),
            season: None,
            episode: None,
        }
    }

    #[test]
    fn a_title_search_result_must_name_the_film() {
        let dark_knight = film("The Dark Knight", 2008);
        assert!(names_title(
            "The.Dark.Knight.2008.1080p.BluRay.x264",
            &dark_knight
        ));
        assert!(names_title("The Dark Knight (2009) 720p", &dark_knight));
        assert!(names_title("The Dark Knight IMAX 1080p", &dark_knight));
        assert!(!names_title(
            "The.Dark.Knight.Rises.2012.1080p",
            &dark_knight
        ));
        assert!(!names_title(
            "The Dark Knight Returns Part 1 2012",
            &dark_knight
        ));
        assert!(!names_title("Batman.Begins.2005.1080p", &dark_knight));

        let your_name = film("Your Name.", 2016);
        assert!(names_title(
            "[Group] Kimi no Na wa. (Your Name.) (2016) [BD 1080p]",
            &your_name
        ));
        assert!(names_title(
            "Spiderman.2002.1080p",
            &film("Spider-Man", 2002)
        ));
        // A title's own number is not a year.
        assert!(names_title("1917.2019.2160p.UHD", &film("1917", 2019)));
        assert!(names_title("Amelie.2001.1080p", &film("Amélie", 2001)));
        assert!(!names_title("Amelie.2011.1080p", &film("Amélie", 2001)));
    }

    #[test]
    fn an_episode_is_matched_by_title_but_not_year() {
        let episode = SourceRequest {
            kind: SourceKind::Episode,
            imdb_id: None,
            title: "Acme Corp".into(),
            year: Some(2011),
            season: Some(1),
            episode: Some(2),
        };
        assert!(names_title("Acme.Corp.2019.S01E02.1080p", &episode));
        assert!(!names_title("Other.Show.S01E02.1080p", &episode));
    }
}
