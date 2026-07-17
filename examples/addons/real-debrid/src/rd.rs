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

fn auth_error(message: &str) -> AddonError {
    let mut err = AddonError::new(AddonErrorCode::Auth, message);
    err.user_visible = true;
    err
}

fn token(host: &dyn Host) -> Result<String, AddonError> {
    host::secret_get(host, "api_token")?.ok_or_else(|| {
        auth_error("no Real-Debrid token: set the api_token secret (real-debrid.com/apitoken)")
    })
}

fn api_get(host: &dyn Host, path_query: &str) -> Result<HttpResponse, AddonError> {
    let mut headers = BTreeMap::new();
    headers.insert(
        "authorization".to_string(),
        format!("Bearer {}", token(host)?),
    );
    let resp = host::http_get(host, &format!("{API}{path_query}"), headers)?;
    match resp.status {
        200..=299 => Ok(resp),
        401 => Err(auth_error("Real-Debrid rejected the api_token (401)")),
        403 => Err(auth_error(
            "Real-Debrid account locked or not premium (403)",
        )),
        status => Err(AddonError::new(
            AddonErrorCode::Network,
            format!("Real-Debrid API returned {status}"),
        )),
    }
}

fn downloads_page(host: &dyn Host, page: u32) -> Result<Vec<DownloadEntry>, AddonError> {
    let resp = api_get(host, &format!("/downloads?page={page}&limit={PAGE_LIMIT}"))?;
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
    }
}

/// Catalog: one flat listing of the account's downloads history, paged by
/// RD's own page numbers (the cursor).
pub fn list(host: &dyn Host, cursor: Option<&str>) -> Result<Page, AddonError> {
    let page_no: u32 = match cursor {
        Some(c) => c
            .parse()
            .map_err(|_| AddonError::new(AddonErrorCode::Malformed, "bad cursor"))?,
        None => 1,
    };
    let entries = downloads_page(host, page_no)?;
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

fn source_for(url: String) -> PlayableSource {
    PlayableSource {
        protocol: Protocol::from_url(&url).unwrap_or(Protocol::Https),
        url,
        headers: BTreeMap::new(),
        subtitles: Vec::new(),
        quality: None,
        audio_lang: None,
        expires_at: None,
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
        let entries = downloads_page(host, page)?;
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
        Err(e) if e.code == AddonErrorCode::Auth => return Ok(AccountStatus::SignedOut),
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
