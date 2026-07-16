/// Manifest-declared egress domain allowlist (FR-081).
///
/// Rules are bare lowercase domains (`api.example.com`) or wildcards over a
/// single-or-more subdomain prefix (`*.example.com`). Wildcards match on
/// label boundaries only and never match the bare apex or IP literals.
#[derive(Debug, Clone, Default)]
pub struct Allowlist {
    rules: Vec<String>,
}

impl Allowlist {
    pub fn new(rules: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Allowlist {
            rules: rules.into_iter().map(|r| r.into().to_ascii_lowercase()).collect(),
        }
    }

    pub fn permits(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        if host.is_empty() {
            return false;
        }
        let is_ip = host.parse::<std::net::IpAddr>().is_ok();
        self.rules.iter().any(|rule| {
            if let Some(apex) = rule.strip_prefix("*.") {
                !is_ip
                    && host.len() > apex.len()
                    && host.ends_with(apex)
                    && host.as_bytes()[host.len() - apex.len() - 1] == b'.'
            } else {
                host == *rule
            }
        })
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_exact_domain() {
        let al = Allowlist::new(["api.example.com"]);
        assert!(al.permits("api.example.com"));
        assert!(al.permits("API.Example.COM"));
        assert!(!al.permits("cdn.example.com"));
        assert!(!al.permits("example.com"));
    }

    #[test]
    fn wildcard_matches_subdomain_not_bare_or_suffix() {
        let al = Allowlist::new(["*.example.com"]);
        assert!(al.permits("a.example.com"));
        assert!(al.permits("deep.a.example.com"));
        assert!(!al.permits("example.com"), "bare apex not covered by wildcard");
        assert!(!al.permits("evilexample.com"), "suffix trick must not match");
        assert!(!al.permits("example.com.evil.org"));
    }

    #[test]
    fn wildcard_never_permits_ip_literals() {
        let al = Allowlist::new(["*.1"], );
        assert!(!al.permits("127.0.0.1"));
        let al = Allowlist::new(["*.0.1"]);
        assert!(!al.permits("127.0.0.1"));
    }
}
