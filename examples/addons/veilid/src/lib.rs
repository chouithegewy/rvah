//! Metadata-only provider. The host owns routes, encryption keys and media bytes.
use std::collections::{BTreeMap, BTreeSet};

use rvah_addon_sdk::guest::{read_op, setting_get, Host, Op, Response};
use rvah_addon_sdk::{
    AddonError, AddonErrorCode, Artwork, ContentType, MediaItem, Page, PageItem, PlayableSource,
    Protocol, ResolveResult,
};
use serde::Deserialize;

const MAX_CATALOG_BYTES: usize = 256 * 1024;
const MAX_ITEMS: usize = 1000;
const PAGE_SIZE: usize = 50;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    id: String,
    title: String,
}

fn invalid(message: &str) -> AddonError {
    AddonError::new(AddonErrorCode::Malformed, message)
}

fn catalog(host: &dyn Host) -> Result<Vec<Entry>, AddonError> {
    let text = setting_get(host, "catalog")?.unwrap_or_else(|| "[]".into());
    if text.len() > MAX_CATALOG_BYTES {
        return Err(invalid("Veilid catalog exceeds 256 KiB"));
    }
    let entries: Vec<Entry> = serde_json::from_str(&text)
        .map_err(|_| invalid("Veilid catalog must be an array of id/title objects"))?;
    if entries.len() > MAX_ITEMS {
        return Err(invalid("Veilid catalog exceeds 1000 entries"));
    }
    let mut seen = BTreeSet::new();
    for entry in &entries {
        if Protocol::veilid_media_id(&format!("rvah+veilid://media/{}", entry.id)).is_none()
            || !seen.insert(&entry.id)
            || entry.title.trim().is_empty()
            || entry.title.len() > 512
            || entry.title.chars().any(char::is_control)
        {
            return Err(invalid(
                "Veilid catalog has an invalid/duplicate ID or title",
            ));
        }
    }
    Ok(entries)
}

fn page(entries: Vec<Entry>, cursor: Option<String>) -> Result<Response, AddonError> {
    let offset = match cursor {
        Some(cursor) => cursor
            .parse::<usize>()
            .map_err(|_| invalid("Invalid cursor"))?,
        None => 0,
    };
    let total = entries.len();
    if offset > total {
        return Err(invalid("Cursor exceeds catalog"));
    }
    let items = entries
        .into_iter()
        .skip(offset)
        .take(PAGE_SIZE)
        .map(|entry| {
            PageItem::Media(MediaItem {
                id: entry.id,
                title: entry.title,
                content_type: ContentType::Clip,
                year: None,
                season: None,
                episode: None,
                runtime_secs: None,
                synopsis: None,
                rating: None,
                artwork: Artwork::default(),
                provider_ids: BTreeMap::new(),
                note: Some("Host-owned Veilid session".into()),
            })
        })
        .collect();
    Ok(Response::Page(Page {
        items,
        cursor_next: (offset + PAGE_SIZE < total).then(|| (offset + PAGE_SIZE).to_string()),
        total_hint: Some(total as u64),
    }))
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn dispatch(input: &[u8], host: &dyn Host) -> Result<Response, AddonError> {
    let op = read_op(input)?;
    let entries = catalog(host)?;
    match op {
        Op::List { path, cursor } => {
            if path
                .as_deref()
                .is_some_and(|path| !matches!(path, "" | "/"))
            {
                return Err(invalid("Unknown Veilid catalog path"));
            }
            page(entries, cursor)
        }
        Op::Search {
            query,
            filters,
            cursor,
        } => {
            if !filters.is_empty() || query.len() > 512 {
                return Err(invalid("Unsupported filters or excessive query length"));
            }
            let query = query.to_lowercase();
            page(
                entries
                    .into_iter()
                    .filter(|entry| entry.title.to_lowercase().contains(&query))
                    .collect(),
                cursor,
            )
        }
        Op::Resolve { item_id, source_id } => {
            if source_id.is_some() || !entries.iter().any(|entry| entry.id == item_id) {
                return Ok(Response::Resolve(ResolveResult::Unavailable {
                    reason: "Unknown Veilid media ID or source".into(),
                }));
            }
            Ok(Response::Resolve(ResolveResult::Playables(vec![
                PlayableSource {
                    url: format!("rvah+veilid://media/{item_id}"),
                    protocol: Protocol::Veilid,
                    headers: BTreeMap::new(),
                    subtitles: vec![],
                    quality: None,
                    audio_lang: None,
                    expires_at: None,
                    drm: None,
                },
            ])))
        }
        _ => Err(invalid("Operation not declared by Veilid provider")),
    }
}

rvah_addon_sdk::export_guest!(dispatch);

#[cfg(test)]
mod tests {
    use super::*;
    use rvah_addon_sdk::guest::ScriptedHost;

    fn host(catalog: &str) -> ScriptedHost {
        let mut host = ScriptedHost::default();
        host.settings.insert("catalog".into(), catalog.into());
        host
    }

    #[test]
    fn browse_search_resolve_without_network_or_secrets() {
        let host = host(r#"[{"id":"demo","title":"Demo film"}]"#);
        let Response::Page(page) = dispatch(br#"{"op":"list"}"#, &host).unwrap() else {
            panic!()
        };
        assert_eq!(page.items.len(), 1);
        let Response::Page(page) = dispatch(br#"{"op":"search","query":"DEMO"}"#, &host).unwrap()
        else {
            panic!()
        };
        assert_eq!(page.items.len(), 1);
        let Response::Resolve(ResolveResult::Playables(sources)) =
            dispatch(br#"{"op":"resolve","item_id":"demo"}"#, &host).unwrap()
        else {
            panic!()
        };
        assert_eq!(sources[0].url, "rvah+veilid://media/demo");
        assert_eq!(sources[0].protocol, Protocol::Veilid);
        assert!(host.requests().is_empty());
        assert!(matches!(
            dispatch(br#"{"op":"resolve","item_id":"absent"}"#, &host).unwrap(),
            Response::Resolve(ResolveResult::Unavailable { .. })
        ));
    }

    #[test]
    fn malformed_and_credential_bearing_catalogs_fail_closed() {
        for text in [
            r#"[{"id":"../secret","title":"Bad"}]"#,
            r#"[{"id":"demo","title":"Good","key":"secret"}]"#,
            r#"[{"id":"demo","title":"A"},{"id":"demo","title":"B"}]"#,
            r#"[{"id":"demo","title":""}]"#,
        ] {
            assert!(dispatch(br#"{"op":"list"}"#, &host(text)).is_err());
        }
        assert!(dispatch(
            br#"{"op":"list","cursor":"18446744073709551615"}"#,
            &host("[]")
        )
        .is_err());
    }
}
