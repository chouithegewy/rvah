//! Property tests for URL protocol classification (FR-060 gatekeeping).

use proptest::prelude::*;
use rvah_addon_sdk::Protocol;

proptest! {
    /// Never panics, and never classifies a non-http(s)/file scheme.
    #[test]
    fn from_url_never_panics_and_is_conservative(url in ".*") {
        let result = Protocol::from_url(&url);
        if let Some(protocol) = result {
            let lower = url.to_ascii_lowercase();
            match protocol {
                Protocol::File => prop_assert!(lower.starts_with("file://")),
                Protocol::Veilid => prop_assert!(Protocol::veilid_media_id(&url).is_some()),
                Protocol::Http => prop_assert!(lower.starts_with("http://")),
                Protocol::Https | Protocol::Hls | Protocol::Dash => {
                    prop_assert!(lower.starts_with("http://") || lower.starts_with("https://"));
                }
                Protocol::Web => prop_assert!(false, "from_url inferred web for {url}"),
            }
        }
    }
}
