//! SubDL's search API against the guest [`Host`], so it unit-tests natively
//! with a scripted host.
//!
//! One search (`GET api.subdl.com/api/v1/subtitles`, by IMDb id, else by
//! title) with `unpack=1`, which lists the files inside each subtitle's
//! archive with a link to each on its own; the player loads those files
//! as they are. A season pack's files are matched to the episode by
//! number. The download links SubDL sends carry your API key; it is cut
//! off, since the files download without it.

use std::collections::{BTreeMap, HashSet};

use rvah_addon_sdk::guest::{self, Host};
use rvah_addon_sdk::{
    AddonError, AddonErrorCode, SourceKind, SubtitleFormat, SubtitleRequest, SubtitleTrack,
};
use serde::Deserialize;

const API: &str = "https://api.subdl.com/api/v1/subtitles";
const DOWNLOADS: &str = "https://dl.subdl.com";
/// Subtitles asked for in one search; a popular episode's answer, season
/// packs and all, stays under 100 KB.
const PER_PAGE: u32 = 30;
/// Files offered, at most.
const MOST: usize = 20;

#[derive(Deserialize)]
struct Search {
    #[serde(default)]
    status: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    subtitles: Vec<Subtitle>,
}

/// One upload: an archive of one or more files.
#[derive(Deserialize)]
struct Subtitle {
    #[serde(default)]
    release_name: String,
    #[serde(default)]
    language: String,
    #[serde(default)]
    hi: bool,
    #[serde(default)]
    season: Option<u32>,
    #[serde(default)]
    episode: Option<u32>,
    #[serde(default)]
    unpack_files: Vec<File>,
}

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    name: String,
    /// 0 when the archive's own season applies.
    #[serde(default)]
    season: Option<u32>,
    #[serde(default)]
    episode: Option<u32>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    hi: Option<bool>,
    #[serde(default)]
    format: String,
    url: String,
}

fn user_error(code: AddonErrorCode, message: &str) -> AddonError {
    let mut err = AddonError::new(code, message);
    err.user_visible = true;
    err
}

/// Subtitle files for `request`, those made for the release playing first.
pub fn subtitles(
    host: &dyn Host,
    request: &SubtitleRequest,
) -> Result<Vec<SubtitleTrack>, AddonError> {
    let key = guest::secret_get(host, "api_key")?
        .filter(|k| !k.trim().is_empty())
        .ok_or_else(|| {
            user_error(
                AddonErrorCode::Auth,
                "no SubDL API key: set api_key (subdl.com, your account's API page)",
            )
        })?;
    let languages: Vec<String> = request
        .languages
        .iter()
        .filter_map(|l| subdl_language(l))
        .collect();
    let languages = if languages.is_empty() {
        "EN".to_string()
    } else {
        languages.join(",")
    };
    let episode = request.kind == SourceKind::Episode;
    let (season, number) = (
        request.season.map(|n| n.to_string()),
        request.episode.map(|n| n.to_string()),
    );
    let year = request.year.map(|y| y.to_string());
    let per_page = PER_PAGE.to_string();
    let mut fields: Vec<(&str, &str)> = vec![("api_key", &key)];
    match request.imdb_id.as_deref().filter(|id| id.starts_with("tt")) {
        Some(imdb) => fields.push(("imdb_id", imdb)),
        None => {
            fields.push(("film_name", &request.title));
            if let Some(year) = year.as_deref().filter(|_| !episode) {
                fields.push(("year", year));
            }
        }
    }
    fields.push(("type", if episode { "tv" } else { "movie" }));
    if episode {
        if let (Some(season), Some(number)) = (season.as_deref(), number.as_deref()) {
            fields.push(("season_number", season));
            fields.push(("episode_number", number));
        }
    }
    fields.extend([
        ("languages", languages.as_str()),
        ("subs_per_page", per_page.as_str()),
        ("unpack", "1"),
    ]);
    let url = format!("{API}?{}", guest::form_encode(&fields));
    let resp = guest::http_get(host, &url, BTreeMap::new())?;
    match resp.status {
        200..=299 => {}
        401 | 403 => {
            return Err(user_error(
                AddonErrorCode::Auth,
                "SubDL refused the API key: check api_key",
            ))
        }
        429 => {
            let mut err = user_error(
                AddonErrorCode::Network,
                "SubDL's limit for today is used up",
            );
            err.retryable = true;
            return Err(err);
        }
        status => {
            return Err(AddonError::new(
                AddonErrorCode::Network,
                format!("SubDL answered {status}"),
            ))
        }
    }
    let search: Search = serde_json::from_slice(&resp.body)
        .map_err(|e| AddonError::new(AddonErrorCode::Malformed, format!("SubDL search: {e}")))?;
    if !search.status {
        // "can't find movie or tv": nothing to offer, not a failure.
        guest::log(
            host,
            &format!(
                "SubDL found nothing: {}",
                search.error.as_deref().unwrap_or("no reason given")
            ),
        );
        return Ok(Vec::new());
    }

    let wanted = request.season.zip(request.episode).filter(|_| episode);
    let mut found: Vec<(u32, bool, SubtitleTrack)> = Vec::new();
    let mut seen = HashSet::new();
    for sub in search.subtitles {
        if let (Some((season, _)), Some(s)) = (wanted, sub.season) {
            if s != 0 && s != season {
                continue;
            }
        }
        let readable: Vec<(File, SubtitleFormat)> = sub
            .unpack_files
            .into_iter()
            .filter_map(|f| format(&f.format).map(|fmt| (f, fmt)))
            .collect();
        let files: Vec<(File, SubtitleFormat)> = match wanted {
            Some((season, number)) => {
                let alone = readable.len() == 1 && sub.episode == Some(number);
                readable
                    .into_iter()
                    .filter(|(f, _)| match f.episode {
                        Some(e) => e == number && f.season.is_none_or(|s| s == 0 || s == season),
                        None => alone,
                    })
                    .collect()
            }
            // A film split over several files (CD1, CD2) can't play as one.
            None if readable.len() == 1 => readable,
            None => continue,
        };
        for (file, format) in files {
            let path = file.url.split(['?', '#']).next().unwrap_or_default();
            if !path.starts_with("/subtitle/") || !seen.insert(path.to_string()) {
                continue;
            }
            let hi = file.hi.unwrap_or(sub.hi);
            let language = file
                .language
                .as_deref()
                .and_then(iso_language)
                .or_else(|| iso_language(&sub.language))
                .unwrap_or_else(|| "en".to_string());
            let made_for = if sub.release_name.trim().is_empty() {
                file.name.clone()
            } else {
                sub.release_name.clone()
            };
            let score = request
                .release_name
                .as_deref()
                .map(|release| matching(&format!("{made_for} {}", file.name), release))
                .unwrap_or(0);
            let label = if hi {
                format!("{made_for} · SDH")
            } else {
                made_for
            };
            found.push((
                score,
                hi,
                SubtitleTrack {
                    url: format!("{DOWNLOADS}{path}"),
                    lang: language,
                    label: Some(label),
                    format: Some(format),
                },
            ));
        }
    }
    // The best match first; among equals, plain before SDH, then SubDL's order.
    found.sort_by_key(|(score, hi, _)| (std::cmp::Reverse(*score), *hi));
    Ok(found
        .into_iter()
        .take(MOST)
        .map(|(_, _, track)| track)
        .collect())
}

fn format(name: &str) -> Option<SubtitleFormat> {
    match name.to_ascii_lowercase().as_str() {
        "srt" => Some(SubtitleFormat::SubRip),
        "vtt" => Some(SubtitleFormat::WebVtt),
        "ass" => Some(SubtitleFormat::Ass),
        "ssa" => Some(SubtitleFormat::Ssa),
        _ => None,
    }
}

/// SubDL's code for an ISO 639-1 language (`en` → `EN`, `pt-BR` → `BR_PT`).
fn subdl_language(iso: &str) -> Option<String> {
    let iso = iso.trim().to_ascii_lowercase();
    if iso == "pt-br" {
        return Some("BR_PT".into());
    }
    let primary = iso.split(['-', '_']).next()?;
    (primary.len() == 2 && primary.bytes().all(|b| b.is_ascii_lowercase()))
        .then(|| primary.to_ascii_uppercase())
}

/// The ISO 639-1 code for SubDL's (`EN` → `en`, `BR_PT` → `pt`).
fn iso_language(subdl: &str) -> Option<String> {
    let code = subdl.trim().to_ascii_lowercase();
    let code = code.rsplit('_').next()?;
    (code.len() == 2 && code.bytes().all(|b| b.is_ascii_lowercase())).then(|| code.to_string())
}

fn tokens(name: &str) -> HashSet<String> {
    name.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// The release group: what follows the last `-` ("…WEB-DL-FLUX" → "flux").
fn group(name: &str) -> Option<String> {
    let name = name.trim();
    let name = name
        .rsplit_once('.')
        .filter(|(_, ext)| ext.len() <= 4 && ext.chars().all(|c| c.is_ascii_alphanumeric()))
        .map_or(name, |(stem, _)| stem);
    let (_, group) = name.rsplit_once('-')?;
    let group = group.trim().to_ascii_lowercase();
    ((2..=20).contains(&group.len()) && group.chars().all(|c| c.is_ascii_alphanumeric()))
        .then_some(group)
}

/// How well a subtitle made for `made_for` should fit `release`: the words
/// they share (resolution, source, codec…), and the release group most.
fn matching(made_for: &str, release: &str) -> u32 {
    let shared = tokens(made_for).intersection(&tokens(release)).count() as u32;
    let same_group = group(release).is_some_and(|g| tokens(made_for).contains(&g));
    shared + if same_group { 5 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_translate_both_ways() {
        assert_eq!(subdl_language("en").as_deref(), Some("EN"));
        assert_eq!(subdl_language("pt-BR").as_deref(), Some("BR_PT"));
        assert_eq!(subdl_language("es-419").as_deref(), Some("ES"));
        assert_eq!(subdl_language("english"), None);
        assert_eq!(iso_language("EN").as_deref(), Some("en"));
        assert_eq!(iso_language("BR_PT").as_deref(), Some("pt"));
        assert_eq!(iso_language("English"), None);
    }

    #[test]
    fn the_release_group_counts_most() {
        assert_eq!(
            group("Acme.Corp.S01E02.1080p.WEB-DL.DDP5.1.H.264-FLUX.mkv").as_deref(),
            Some("flux")
        );
        assert_eq!(group("Acme Corp, Season 1 1080p"), None);
        let release = "Acme.Corp.S01E02.1080p.WEB-DL.DDP5.1.H.264-FLUX";
        assert!(
            matching("Acme.Corp.S01E02.720p.HDTV.x264-FLUX", release)
                > matching("Acme.Corp.S01E02.1080p.BluRay.x264-OTHER", release)
        );
    }
}
