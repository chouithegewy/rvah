/// Scrubs secret material from text bound for logs or diagnostics (FR-084).
///
/// Two layers: exact matches of registered secret values, and pattern-based
/// masking of common credential shapes (`Authorization:` headers, `Bearer`
/// tokens, `api_key=`/`access_token=` query pairs) so unregistered secrets
/// still never leak verbatim (TC-SEC-003).
#[derive(Default)]
pub struct Redactor {
    secrets: Vec<String>,
}

const MASK: &str = "[REDACTED]";

impl Redactor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_secret(&mut self, value: impl Into<String>) {
        let value = value.into();
        // Never register trivially short values; replacing them would mangle text.
        if value.len() >= 4 && !self.secrets.contains(&value) {
            self.secrets.push(value);
        }
    }

    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for secret in &self.secrets {
            out = out.replace(secret.as_str(), MASK);
        }
        out = mask_after_prefix(&out, "authorization:", MaskSpan::RestOfLine);
        out = mask_after_prefix(&out, "bearer ", MaskSpan::Token);
        out = mask_after_prefix(&out, "api_key=", MaskSpan::Token);
        out = mask_after_prefix(&out, "access_token=", MaskSpan::Token);
        out
    }
}

enum MaskSpan {
    /// Mask through the end of the line (header values may contain spaces).
    RestOfLine,
    /// Mask a single token (ends at whitespace, `&`, `"`, or `'`).
    Token,
}

fn mask_after_prefix(text: &str, prefix: &str, span: MaskSpan) -> String {
    let lower = text.to_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    while let Some(found) = lower[pos..].find(prefix) {
        let start = pos + found + prefix.len();
        out.push_str(&text[pos..start]);
        let rest = &text[start..];
        let skip = rest.len() - rest.trim_start().len();
        out.push_str(&rest[..skip]);
        let value = &rest[skip..];
        let end = match span {
            MaskSpan::RestOfLine => value.find(['\n', '\r']).unwrap_or(value.len()),
            MaskSpan::Token => value
                .find(|c: char| c.is_whitespace() || matches!(c, '&' | '"' | '\''))
                .unwrap_or(value.len()),
        };
        if end > 0 {
            out.push_str(MASK);
        }
        pos = start + skip + end;
    }
    out.push_str(&text[pos..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redactor_masks_registered_and_patterns() {
        let mut r = Redactor::new();
        r.register_secret("super-secret-value");

        // Registered value, anywhere in the text.
        let out = r.redact("request failed: token=super-secret-value try again");
        assert!(!out.contains("super-secret-value"));
        assert!(out.contains("[REDACTED]"));

        // Authorization header masks the whole value, case-insensitively.
        let out = r.redact("Authorization: Basic dXNlcjpwYXNz\nnext line ok");
        assert!(!out.contains("dXNlcjpwYXNz"));
        assert!(out.contains("next line ok"));

        // Bearer token.
        let out = r.redact("sent bearer eyJhbGciOi.header to api");
        assert!(!out.contains("eyJhbGciOi"));
        assert!(out.contains("to api"));

        // Query-style pairs stop at `&`.
        let out = r.redact("GET /v1?api_key=abc123&page=2&access_token=zzz9");
        assert!(!out.contains("abc123"));
        assert!(!out.contains("zzz9"));
        assert!(out.contains("&page=2&"));
    }

    #[test]
    fn redactor_leaves_clean_text_alone() {
        let r = Redactor::new();
        let text = "browsing catalog page 2 of com.example.addon";
        assert_eq!(r.redact(text), text);
    }

    #[test]
    fn short_secrets_are_not_registered() {
        let mut r = Redactor::new();
        r.register_secret("ab");
        assert_eq!(r.redact("lab table"), "lab table");
    }
}
