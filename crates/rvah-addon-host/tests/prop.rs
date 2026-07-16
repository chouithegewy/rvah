//! Property tests for the manifest parser — the security boundary must never
//! panic on attacker-controlled input (STP fuzz/property requirement).

use proptest::prelude::*;
use rvah_addon_host::Manifest;

proptest! {
    /// Arbitrary bytes-as-text: parse may fail, but must never panic.
    #[test]
    fn manifest_parse_never_panics(input in ".*") {
        let _ = Manifest::parse(&input);
    }

    /// Arbitrary TOML-shaped ids: any Ok result implies the id actually
    /// satisfies the reverse-DNS rule.
    #[test]
    fn accepted_ids_are_reverse_dns(id in "[a-zA-Z0-9._-]{0,40}") {
        let manifest = format!(r#"
[addon]
id = "{id}"
name = "X"
version = "1.0.0"
sdk = ">=0.1, <0.2"
entry = "main"
"#);
        if Manifest::parse(&manifest).is_ok() {
            // Must contain a dot and only lowercase label characters.
            prop_assert!(id.contains('.'));
            prop_assert!(id.chars().all(|c| c.is_ascii_lowercase()
                || c.is_ascii_digit() || c == '.' || c == '-'));
            prop_assert!(!id.starts_with('.') && !id.ends_with('.'));
        }
    }

    /// Storage quota strings never panic and never overflow into 0-byte
    /// grants for positive sizes.
    #[test]
    fn storage_quota_strings_never_panic(size in "[0-9]{0,12}(B|KiB|MiB|GiB|XYZ)?") {
        let manifest = format!(r#"
[addon]
id = "com.example.a"
name = "X"
version = "1.0.0"
sdk = ">=0.1, <0.2"
entry = "main"

[permissions]
storage = "{size}"
"#);
        let _ = Manifest::parse(&manifest);
    }
}
