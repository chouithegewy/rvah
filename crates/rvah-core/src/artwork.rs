//! Which add-on artwork and subtitle files the UI may load, and which
//! trailers it may open. The UI fetches these itself, outside the add-on's
//! sandbox, so an add-on must not be able to point it at an arbitrary
//! server: only https URLs on hosts the add-on declared in its network
//! permissions get through. Trailers are a site's video key; the UI builds
//! the link.

use rvah_addon_sdk::{Artwork, ItemDetails, Page, PageItem, SubtitleTrack};
use rvah_net::Allowlist;

/// Drop every artwork URL in `page` that `network` does not allow.
pub(crate) fn screen_page(page: &mut Page, network: &[String]) {
    let allow = Allowlist::new(network.iter().cloned());
    for item in &mut page.items {
        let artwork = match item {
            PageItem::Directory(d) => &mut d.artwork,
            PageItem::Media(m) => &mut m.artwork,
        };
        screen(artwork, &allow);
    }
}

/// `details` with the same artwork rule, and only trailers whose key is a
/// plain video id (no path, query or other characters to smuggle a link).
pub(crate) fn screen_details(details: &mut ItemDetails, network: &[String]) {
    screen(
        &mut details.item.artwork,
        &Allowlist::new(network.iter().cloned()),
    );
    details
        .trailers
        .retain(|trailer| plain_video_key(&trailer.key));
}

/// Only subtitle files on the same rule, in a format the player can read
/// (given, or told by the URL's extension), with a language.
pub(crate) fn screen_subtitles(tracks: &mut Vec<SubtitleTrack>, network: &[String]) {
    let allow = Allowlist::new(network.iter().cloned());
    tracks.retain(|track| {
        allowed(&track.url, &allow)
            && track.effective_format().is_some()
            && !track.lang.trim().is_empty()
    });
}

fn plain_video_key(key: &str) -> bool {
    (1..=64).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn screen(artwork: &mut Artwork, allow: &Allowlist) {
    for url in [&mut artwork.poster, &mut artwork.fanart, &mut artwork.thumb] {
        if url.as_deref().is_some_and(|u| !allowed(u, allow)) {
            *url = None;
        }
    }
}

fn allowed(url: &str, allow: &Allowlist) -> bool {
    let Ok(url) = url::Url::parse(url) else {
        return false;
    };
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.host_str().is_some_and(|host| allow.permits(host))
}
