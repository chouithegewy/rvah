//! Network filtering for the web player: EasyList-style rules (Adblock Plus
//! syntax) through Brave's adblock-rust. Network rules only; the player
//! shows one page, so there is nothing to hide cosmetically.

use std::sync::Arc;

use adblock::lists::{FilterSet, ParseOptions, RuleTypes};
use adblock::request::Request;
use adblock::Engine;

/// Answers, per request the web player makes, whether a list blocks it.
#[derive(uniffi::Object)]
pub struct AdBlocker {
    engine: Engine,
    rules: u32,
}

#[uniffi::export]
impl AdBlocker {
    /// `lists` are whole filter lists' text, such as EasyList.
    #[uniffi::constructor]
    pub fn new(lists: Vec<String>) -> Arc<Self> {
        let mut set = FilterSet::new(false);
        let options = ParseOptions {
            rule_types: RuleTypes::NetworkOnly,
            ..ParseOptions::default()
        };
        let mut rules = 0u32;
        for list in lists {
            rules += list.lines().filter(|line| is_network_rule(line)).count() as u32;
            set.add_filter_list(list, options);
        }
        let engine = Engine::new_with_filter_set(set);
        rvah_core::debug_log!("ad blocker: {rules} network rules");
        Arc::new(Self { engine, rules })
    }

    /// Whether to block `url`, requested by the page at `source_url` as
    /// `request_type` ("script", "image", "xmlhttprequest", …). A request
    /// that cannot be parsed is let through. Rules naming a method are
    /// rare; requests are checked as GETs.
    pub fn should_block(&self, url: String, source_url: String, request_type: String) -> bool {
        match Request::new(&url, &source_url, &request_type, "GET") {
            Ok(request) => self.engine.check_network_request(&request).should_block(),
            Err(_) => false,
        }
    }

    /// How many network rules the lists held, for the log.
    pub fn rule_count(&self) -> u32 {
        self.rules
    }
}

/// A list line that is a network rule: not blank, a comment (`!`), a header
/// (`[Adblock Plus …]`) or a cosmetic rule (`##`, `#@#`, `#?#`, `#$#`).
fn is_network_rule(line: &str) -> bool {
    let line = line.trim();
    !(line.is_empty()
        || line.starts_with('!')
        || line.starts_with('[')
        || line.contains("##")
        || line.contains("#@#")
        || line.contains("#?#")
        || line.contains("#$#"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: &str = "||tracker.example^\n||ads.example^$third-party\n@@||ads.example/ok^\n";

    fn check(blocker: &AdBlocker, url: &str, kind: &str) -> bool {
        blocker.should_block(url.into(), "https://site.example/".into(), kind.into())
    }

    #[test]
    fn blocks_listed_tracker() {
        let b = AdBlocker::new(vec![RULES.into()]);
        assert!(check(&b, "https://tracker.example/t.js", "script"));
        assert!(!check(&b, "https://cdn.example.net/v.ts", "xmlhttprequest"));
    }

    #[test]
    fn exceptions_and_garbage_are_allowed() {
        let b = AdBlocker::new(vec![RULES.into()]);
        assert!(!check(&b, "https://ads.example/ok", "image"));
        assert!(!check(&b, "not a url", "other"));
    }

    #[test]
    fn empty_lists_block_nothing() {
        let b = AdBlocker::new(vec![]);
        assert!(!check(&b, "https://tracker.example/", "script"));
        assert_eq!(b.rule_count(), 0);
    }

    #[test]
    fn counts_network_rules() {
        assert_eq!(AdBlocker::new(vec![RULES.into()]).rule_count(), 3);
    }

    #[allow(dead_code)]
    fn send_sync() {
        fn check<T: Send + Sync>() {}
        check::<AdBlocker>();
    }
}
