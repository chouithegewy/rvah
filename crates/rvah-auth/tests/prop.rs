//! Property tests for redaction (FR-084, TC-SEC-003): a registered secret
//! must never survive redaction, whatever surrounds it.

use proptest::prelude::*;
use rvah_auth::Redactor;

proptest! {
    /// Any registered secret (>= 4 chars) is scrubbed from any text.
    #[test]
    fn registered_secret_never_survives(
        prefix in "[ -~]{0,30}",
        secret in "[a-zA-Z0-9]{4,24}",
        suffix in "[ -~]{0,30}",
    ) {
        let mut redactor = Redactor::new();
        redactor.register_secret(secret.clone());
        let out = redactor.redact(&format!("{prefix}{secret}{suffix}"));
        prop_assert!(!out.contains(&secret));
    }

    /// Bearer-style tokens are pattern-masked even when never registered.
    #[test]
    fn bearer_tokens_pattern_masked(token in "[A-Za-z0-9._-]{8,40}") {
        let redactor = Redactor::new();
        let out = redactor.redact(&format!("sent Bearer {token} upstream"));
        prop_assert!(!out.contains(&token));
    }

    /// Redaction never panics on arbitrary input.
    #[test]
    fn redact_never_panics(text in ".*") {
        let mut redactor = Redactor::new();
        redactor.register_secret("some-secret-value");
        let _ = redactor.redact(&text);
    }
}
