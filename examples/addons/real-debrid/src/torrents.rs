//! Playing a release another add-on found (capability `debrid`), instant
//! only: add its magnet, pick the video file (the episode, for a season
//! pack), select it, and play it only if Real-Debrid has it ready at once.
//! Anything Real-Debrid would still have to fetch is deleted again, so the
//! viewer can pick another release instead of waiting.
//!
//! Real-Debrid dropped its instant-availability check in 2024; adding the
//! magnet is the only way to find out.

use rvah_addon_sdk::{AddonError, AddonErrorCode, Release, ResolveResult};
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::host::{self, form_encode, Host};
use crate::rd::{api_url, bearer, check_status, source_for};

const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "mov", "ts", "m2ts", "webm", "wmv", "mpg", "mpeg",
];

#[derive(Deserialize)]
struct Added {
    id: String,
}

#[derive(Deserialize)]
struct TorrentFile {
    id: u64,
    path: String,
    bytes: u64,
}

#[derive(Deserialize)]
struct TorrentInfo {
    status: String,
    #[serde(default)]
    files: Vec<TorrentFile>,
    #[serde(default)]
    links: Vec<String>,
}

#[derive(Deserialize)]
struct Unrestricted {
    download: String,
}

fn malformed(what: &str, e: impl std::fmt::Display) -> AddonError {
    AddonError::new(
        AddonErrorCode::Malformed,
        format!("Real-Debrid {what}: {e}"),
    )
}

fn send<T: DeserializeOwned>(
    host: &dyn Host,
    method: &str,
    path: &str,
    form: Option<&[(&str, &str)]>,
) -> Result<T, AddonError> {
    let resp = send_raw(host, method, path, form)?;
    serde_json::from_slice(&resp).map_err(|e| malformed(path, e))
}

fn send_raw(
    host: &dyn Host,
    method: &str,
    path: &str,
    form: Option<&[(&str, &str)]>,
) -> Result<Vec<u8>, AddonError> {
    let mut headers = bearer(host)?;
    let body = form.map(|fields| {
        headers.insert(
            "content-type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        );
        form_encode(fields).into_bytes()
    });
    let resp = host::http_send(host, method, &api_url(path), headers, body)?;
    check_status(resp.status)?;
    Ok(resp.body)
}

fn info(host: &dyn Host, id: &str) -> Result<TorrentInfo, AddonError> {
    send(host, "GET", &format!("/torrents/info/{id}"), None)
}

fn not_ready(host: &dyn Host, id: &str, reason: String) -> Result<ResolveResult, AddonError> {
    // Best effort: a leftover torrent in the account is harmless.
    if let Err(e) = send_raw(host, "DELETE", &format!("/torrents/delete/{id}"), None) {
        host::log(
            host,
            &format!("could not delete torrent {id}: {}", e.message),
        );
    }
    Ok(ResolveResult::Unavailable { reason })
}

fn is_hash(hash: &str) -> bool {
    hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The file to play: the largest video that is not a sample, and for a
/// season and episode, one whose name says so (`S01E02`, `1x02`).
fn pick_file(
    files: &[TorrentFile],
    season: Option<u32>,
    episode: Option<u32>,
) -> Option<&TorrentFile> {
    files
        .iter()
        .filter(|f| {
            let name = f.path.to_ascii_lowercase();
            let video = name
                .rsplit_once('.')
                .is_some_and(|(_, ext)| VIDEO_EXTENSIONS.contains(&ext));
            video && !words(&name).any(|w| w == "sample")
        })
        .filter(|f| match (season, episode) {
            (Some(s), Some(e)) => episode_matches(&f.path, s, e),
            _ => true,
        })
        .max_by_key(|f| f.bytes)
}

fn words(name: &str) -> impl Iterator<Item = &str> {
    name.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
}

/// Whether a file name is of `season` × `episode`: `S01E02`, `s1e2`,
/// `S01E01E02` (a double episode), or `1x02`.
pub(crate) fn episode_matches(path: &str, season: u32, episode: u32) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let found = words(&name).any(|word| {
        if let Some(rest) = word.strip_prefix('s') {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 || digits > 2 || rest[..digits].parse() != Ok(season) {
                return false;
            }
            let mut rest = &rest[digits..];
            while let Some(after) = rest.strip_prefix('e') {
                let n = after.bytes().take_while(u8::is_ascii_digit).count();
                if n == 0 || n > 3 {
                    return false;
                }
                if after[..n].parse() == Ok(episode) {
                    return true;
                }
                rest = &after[n..];
            }
            false
        } else if let Some((s, e)) = word.split_once('x') {
            s.len() <= 2
                && (2..=3).contains(&e.len())
                && s.parse() == Ok(season)
                && e.parse() == Ok(episode)
        } else {
            false
        }
    });
    found
}

/// Play `release`, or say why it cannot play right now.
pub fn resolve_release(
    host: &dyn Host,
    release: &Release,
    season: Option<u32>,
    episode: Option<u32>,
) -> Result<ResolveResult, AddonError> {
    if !is_hash(&release.info_hash) {
        return Err(malformed("release", "not an info hash"));
    }
    let magnet = format!("magnet:?xt=urn:btih:{}", release.info_hash);
    let added: Added = send(
        host,
        "POST",
        "/torrents/addMagnet",
        Some(&[("magnet", &magnet)]),
    )?;
    let torrent = info(host, &added.id)?;
    match torrent.status.as_str() {
        // Real-Debrid already holds this one in full.
        "downloaded" => return play(host, &torrent),
        // Known at once: Real-Debrid has its file list.
        "waiting_files_selection" => {}
        _ => {
            return not_ready(
                host,
                &added.id,
                "not cached on Real-Debrid; pick another release".into(),
            )
        }
    }
    let Some(file) = pick_file(&torrent.files, season, episode) else {
        let what = match (season, episode) {
            (Some(s), Some(e)) => format!("no file for S{s:02}E{e:02} in this release"),
            _ => "no video file in this release".to_string(),
        };
        return not_ready(host, &added.id, what);
    };
    send_raw(
        host,
        "POST",
        &format!("/torrents/selectFiles/{}", added.id),
        Some(&[("files", &file.id.to_string())]),
    )?;
    // A cached file is ready as soon as it is selected; Real-Debrid can take
    // a moment to say so.
    for _ in 0..3 {
        let torrent = info(host, &added.id)?;
        match torrent.status.as_str() {
            "downloaded" => return play(host, &torrent),
            "waiting_files_selection" => continue,
            _ => break,
        }
    }
    not_ready(
        host,
        &added.id,
        "not cached on Real-Debrid; pick another release".into(),
    )
}

fn play(host: &dyn Host, torrent: &TorrentInfo) -> Result<ResolveResult, AddonError> {
    let link = torrent
        .links
        .first()
        .ok_or_else(|| malformed("torrent", "downloaded but no link"))?;
    let unrestricted: Unrestricted =
        send(host, "POST", "/unrestrict/link", Some(&[("link", link)]))?;
    Ok(ResolveResult::Playables(vec![source_for(
        unrestricted.download,
    )]))
}
