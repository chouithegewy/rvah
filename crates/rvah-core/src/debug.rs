//! Process-wide debug logging, enabled by `rvah-app --debug` (or any caller
//! of [`set_enabled`]). Lines go to stderr, stamped with time since enable so
//! slow add-on invocations are visible at a glance.

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Global-flag lifecycle in one test: parallel test threads must not
    /// observe each other's toggles.
    #[test]
    fn flag_toggles_and_macro_is_callable() {
        assert!(!enabled(), "debug logging must be off by default");
        set_enabled(true);
        assert!(enabled());
        crate::debug_log!("macro works while enabled: {}", 42);
        set_enabled(false);
        assert!(!enabled());
        crate::debug_log!("macro is a no-op while disabled");
    }

    #[test]
    fn format_line_stamps_elapsed_and_prefix() {
        let line = format_line(Duration::from_millis(1234), format_args!("hello {}", 42));
        assert_eq!(line, "[rvah +1.234s] hello 42");
    }
}
