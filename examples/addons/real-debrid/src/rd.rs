//! Real-Debrid logic: the account's downloads history is the catalog
//! (`list`), and `resolve` returns the entry's already-unrestricted direct
//! URL. Everything runs against the [`Host`] trait so it unit-tests
//! natively with a scripted host.

use std::collections::BTreeMap;

use rvah_addon_sdk::{
    AccountStatus, AddonError, AddonErrorCode, ContentType, HttpResponse, MediaItem, Page,
    PageItem, PlayableSource, Protocol, ResolveResult,
};
use serde::Deserialize;

use crate::host::{self, Host};

const API: &str = "https://api.real-debrid.com/rest/1.0";
const PAGE_LIMIT: usize = 50;
/// Search reads the history in pages of RD's default size.
const SEARCH_PAGE_LIMIT: usize = 100;
/// History pages one search call reads. A call gets 50M fuel and 2 s; a
/// page of 100 typical entries takes about 12M fuel under Wasmi, plus a
/// request, and a Shield TV runs Wasmi roughly ten times slower than a
/// desktop. Two pages leave room for long file names and a slow network.
pub(crate) const SEARCH_SCAN_PAGES: u32 = 2;
/// How many history pages `resolve` will scan when the storage cache misses.
const RESOLVE_SCAN_PAGES: u32 = 10;

/// One entry of `GET /downloads` (fields we use).
#[derive(Debug, Clone, Deserialize)]
struct DownloadEntry {
    id: String,
    filename: String,
    /// The unrestricted direct link.
    download: String,
    #[serde(default)]
    filesize: Option<u64>,
}

pub(crate) fn auth_error(message: &str) -> AddonError {
    let mut err = AddonError::new(AddonErrorCode::Auth, message);
    err.user_visible = true;
    err
}

fn token(host: &dyn Host) -> Result<String, AddonError> {
    host::secret_get(host, "api_token")?.ok_or_else(|| {
        auth_error(
            "not signed in to Real-Debrid: use Link account, or set the api_token secret \
             (real-debrid.com/apitoken)",
        )
    })
}

pub(crate) fn api_url(path_query: &str) -> String {
    format!("{API}{path_query}")
}

/// The Authorization header for the REST API.
pub(crate) fn bearer(host: &dyn Host) -> Result<BTreeMap<String, String>, AddonError> {
    Ok(BTreeMap::from([(
        "authorization".to_string(),
        format!("Bearer {}", token(host)?),
    )]))
}

pub(crate) fn check_status(status: u16) -> Result<(), AddonError> {
    match status {
        200..=299 => Ok(()),
        // Also how an expired device-link access token shows up; the host
        // then asks `auth::refresh` for a new one and retries.
        401 => Err(auth_error("Real-Debrid rejected the access token (401)")),
        403 => Err(auth_error(
            "Real-Debrid account locked or not premium (403)",
        )),
        status => Err(AddonError::new(
            AddonErrorCode::Network,
            format!("Real-Debrid API returned {status}"),
        )),
    }
}

fn api_get(host: &dyn Host, path_query: &str) -> Result<HttpResponse, AddonError> {
    let resp = host::http_get(host, &api_url(path_query), bearer(host)?)?;
    check_status(resp.status)?;
    Ok(resp)
}

fn downloads_page(
    host: &dyn Host,
    page: u32,
    limit: usize,
) -> Result<Vec<DownloadEntry>, AddonError> {
    let resp = api_get(host, &format!("/downloads?page={page}&limit={limit}"))?;
    if resp.body.is_empty() {
        // RD answers 204 with an empty body past the last page.
        return Ok(Vec::new());
    }
    serde_json::from_slice(&resp.body).map_err(|e| {
        AddonError::new(
            AddonErrorCode::Malformed,
            format!("Real-Debrid downloads JSON: {e}"),
        )
    })
}

fn cache_key(item_id: &str) -> String {
    format!("dl:{item_id}")
}

fn remember(host: &dyn Host, entry: &DownloadEntry) {
    // Cache id → direct URL so resolve avoids re-walking the history.
    let _ = host::storage_set(host, &cache_key(&entry.id), entry.download.as_bytes());
}

fn to_item(entry: &DownloadEntry) -> MediaItem {
    MediaItem {
        id: entry.id.clone(),
        content_type: ContentType::Other,
        title: entry.filename.clone(),
        year: None,
        season: None,
        episode: None,
        runtime_secs: None,
        synopsis: entry
            .filesize
            .map(|bytes| format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))),
        rating: None,
        artwork: Default::default(),
        provider_ids: Default::default(),
        note: None,
    }
}

/// The history page a cursor names: RD's own page numbers.
fn page_number(cursor: Option<&str>) -> Result<u32, AddonError> {
    match cursor {
        Some(c) => c
            .parse()
            .map_err(|_| AddonError::new(AddonErrorCode::Malformed, "bad cursor")),
        None => Ok(1),
    }
}

/// Catalog: one flat listing of the account's downloads history, paged by
/// RD's own page numbers (the cursor).
pub fn list(host: &dyn Host, cursor: Option<&str>) -> Result<Page, AddonError> {
    let page_no = page_number(cursor)?;
    let entries = downloads_page(host, page_no, PAGE_LIMIT)?;
    for entry in &entries {
        remember(host, entry);
    }
    Ok(Page {
        cursor_next: (entries.len() == PAGE_LIMIT).then(|| (page_no + 1).to_string()),
        total_hint: None,
        items: entries
            .iter()
            .map(|e| PageItem::Media(to_item(e)))
            .collect(),
    })
}

/// A search query as lowercase words. Anything but a letter or digit
/// separates words, so "coyote vs. acme" and "Coyote.vs.Acme_2026" compare.
pub(crate) struct Query(Vec<String>);

impl Query {
    pub(crate) fn new(text: &str) -> Self {
        Query(
            text.split(|c: char| !c.is_alphanumeric())
                .filter(|word| !word.is_empty())
                .map(str::to_lowercase)
                .collect(),
        )
    }

    /// Every query word appears in the file name, in any order; a word may
    /// be the start or part of a longer one ("2160" finds "2160p").
    pub(crate) fn matches(&self, filename: &str) -> bool {
        let name = Query::new(filename).0.join(" ");
        self.0.iter().all(|word| name.contains(word.as_str()))
    }
}

/// Search: RD cannot search the history itself, so read it newest first,
/// [`SEARCH_SCAN_PAGES`] pages per call, keeping the downloads whose file
/// name has every query word. The cursor is the next unread page.
pub fn search(host: &dyn Host, query: &str, cursor: Option<&str>) -> Result<Page, AddonError> {
    let query = Query::new(query);
    let mut items = Vec::new();
    let mut next = None;
    if !query.0.is_empty() {
        let first = page_number(cursor)?;
        for page_no in first..first + SEARCH_SCAN_PAGES {
            let entries = downloads_page(host, page_no, SEARCH_PAGE_LIMIT)?;
            for entry in entries.iter().filter(|e| query.matches(&e.filename)) {
                // Only matches: resolve needs them, and each write is a host call.
                remember(host, entry);
                items.push(PageItem::Media(to_item(entry)));
            }
            if entries.len() < SEARCH_PAGE_LIMIT {
                next = None;
                break;
            }
            next = Some(page_no + 1);
            if items.len() >= PAGE_LIMIT {
                break;
            }
        }
    }
    Ok(Page {
        items,
        cursor_next: next.map(|page| page.to_string()),
        total_hint: None,
    })
}

pub(crate) fn source_for(url: String) -> PlayableSource {
    PlayableSource {
        protocol: Protocol::from_url(&url).unwrap_or(Protocol::Https),
        url,
        headers: BTreeMap::new(),
        subtitles: Vec::new(),
        quality: None,
        audio_lang: None,
        expires_at: None,
        drm: None,
        never_block: Vec::new(),
    }
}

/// Resolve: direct URL from the list-time cache, else re-scan the history.
pub fn resolve(host: &dyn Host, item_id: &str) -> Result<ResolveResult, AddonError> {
    if let Some(bytes) = host::storage_get(host, &cache_key(item_id))? {
        if let Ok(url) = String::from_utf8(bytes) {
            return Ok(ResolveResult::Playables(vec![source_for(url)]));
        }
    }
    for page in 1..=RESOLVE_SCAN_PAGES {
        let entries = downloads_page(host, page, PAGE_LIMIT)?;
        let last_page = entries.len() < PAGE_LIMIT;
        for entry in &entries {
            remember(host, entry);
            if entry.id == item_id {
                return Ok(ResolveResult::Playables(vec![source_for(
                    entry.download.clone(),
                )]));
            }
        }
        if last_page {
            break;
        }
    }
    Ok(ResolveResult::Unavailable {
        reason: format!("{item_id} not found in the Real-Debrid downloads history"),
    })
}

fn linked(host: &dyn Host) -> Result<bool, AddonError> {
    Ok(host::secret_get(host, "oauth_client")?.is_some())
}

/// `GET /user`: premium account → signed in.
pub fn account(host: &dyn Host) -> Result<AccountStatus, AddonError> {
    #[derive(Deserialize)]
    struct User {
        username: String,
        #[serde(rename = "type", default)]
        kind: Option<String>,
    }
    let resp = match api_get(host, "/user") {
        Ok(resp) => resp,
        // A device link has refresh material: let the host refresh it.
        Err(e) if e.code == AddonErrorCode::Auth && !linked(host)? => {
            return Ok(AccountStatus::SignedOut)
        }
        Err(e) => return Err(e),
    };
    let user: User = serde_json::from_slice(&resp.body).map_err(|e| {
        AddonError::new(AddonErrorCode::Malformed, format!("Real-Debrid user: {e}"))
    })?;
    host::log(
        host,
        &format!(
            "Real-Debrid account {} ({})",
            user.username,
            user.kind.as_deref().unwrap_or("unknown")
        ),
    );
    Ok(AccountStatus::SignedIn {
        display_name: Some(user.username),
    })
}
