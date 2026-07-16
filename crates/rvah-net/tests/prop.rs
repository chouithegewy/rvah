//! Property tests for allowlist wildcard safety (FR-081, TC-SEC-002 family):
//! a wildcard rule must never leak beyond its label boundary.

use proptest::prelude::*;
use rvah_net::Allowlist;

proptest! {
    /// `*.example.com` permits exactly: hosts ending in ".example.com".
    #[test]
    fn wildcard_only_matches_true_subdomains(host in "[a-z0-9.-]{1,40}") {
        let allow = Allowlist::new(["*.example.com"]);
        if allow.permits(&host) {
            prop_assert!(host.ends_with(".example.com"));
            prop_assert!(host.len() > ".example.com".len());
        }
    }

    /// Wildcards never permit IP literals, whatever their shape.
    #[test]
    fn wildcard_never_permits_ipv4(a in 0u8..=255, b in 0u8..=255, c in 0u8..=255, d in 0u8..=255) {
        let allow = Allowlist::new(["*.example.com", "*.10.0.0.1"]);
        let host = format!("{a}.{b}.{c}.{d}");
        let permitted = allow.permits(&host);
        prop_assert!(!permitted);
    }

    /// Exact rules match only themselves (case-insensitively), never
    /// suffix-confusable hosts like "evilexample.com".
    #[test]
    fn exact_rule_is_exact(host in "[a-z0-9.-]{1,40}") {
        let allow = Allowlist::new(["example.com"]);
        if allow.permits(&host) {
            prop_assert_eq!(host.to_ascii_lowercase(), "example.com");
        }
    }

    /// permits() never panics on arbitrary junk.
    #[test]
    fn permits_never_panics(host in ".*", rule in "[a-z0-9.*-]{0,30}") {
        let allow = Allowlist::new([rule.as_str()]);
        let _ = allow.permits(&host);
    }
}
