//! Process-wide debug logging, enabled by `rvah-app --debug` (or any caller
//! of [`set_enabled`]). Lines go to stderr, or to the [`set_sink`] function
//! where stderr goes nowhere (logcat on Android), stamped with time since
//! enable so slow add-on invocations are visible at a glance.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

static ENABLED: AtomicBool = AtomicBool::new(false);
static STARTED: Mutex<Option<Instant>> = Mutex::new(None);
static SINK: Mutex<Option<fn(&str)>> = Mutex::new(None);

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Turn logging on or off; enabling (re)starts the elapsed-time clock.
pub fn set_enabled(on: bool) {
    if on {
        *STARTED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    }
    ENABLED.store(on, Ordering::Relaxed);
}

/// Send lines to `sink` instead of stderr; `None` restores stderr.
pub fn set_sink(sink: Option<fn(&str)>) {
    *SINK.lock().unwrap_or_else(|e| e.into_inner()) = sink;
}

pub fn format_line(elapsed: Duration, args: std::fmt::Arguments<'_>) -> String {
    format!("[rvah +{:.3}s] {args}", elapsed.as_secs_f64())
}

#[doc(hidden)]
pub fn write_line(args: std::fmt::Arguments<'_>) {
    let started = *STARTED.lock().unwrap_or_else(|e| e.into_inner());
    let elapsed = started.map(|s| s.elapsed()).unwrap_or_default();
    let line = format_line(elapsed, args);
    match *SINK.lock().unwrap_or_else(|e| e.into_inner()) {
        Some(sink) => sink(&line),
        None => eprintln!("{line}"),
    }
}

/// `format!`-style debug line on stderr; free when logging is disabled.
#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        if $crate::debug::enabled() {
            $crate::debug::write_line(format_args!($($arg)*));
        }
    };
}

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

    static CAPTURED: Mutex<Vec<String>> = Mutex::new(Vec::new());

    fn capture(line: &str) {
        CAPTURED.lock().unwrap().push(line.to_string());
    }

    #[test]
    fn lines_go_to_the_installed_sink() {
        set_sink(Some(capture));
        write_line(format_args!("to the sink {}", 7));
        set_sink(None);
        write_line(format_args!("back on stderr"));
        let captured = CAPTURED.lock().unwrap();
        assert!(captured
            .iter()
            .any(|l| l.starts_with("[rvah +") && l.ends_with("to the sink 7")));
        assert!(!captured.iter().any(|l| l.contains("back on stderr")));
    }

    #[test]
    fn format_line_stamps_elapsed_and_prefix() {
        let line = format_line(Duration::from_millis(1234), format_args!("hello {}", 42));
        assert_eq!(line, "[rvah +1.234s] hello 42");
    }
}
