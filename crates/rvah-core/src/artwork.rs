//! Which add-on artwork the UI may load. The UI fetches artwork itself,
//! outside the add-on's sandbox, so an add-on must not be able to point it
//! at an arbitrary server: only https URLs on hosts the add-on declared in
//! its network permissions get through.

use rvah_addon_sdk::{Artwork, Page, PageItem};
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

pub(crate) fn screen(artwork: &mut Artwork, allow: &Allowlist) {
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
