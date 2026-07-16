/// A persistable watch-state snapshot (FR-065).
#[derive(Debug, Clone, PartialEq)]
pub struct ProgressUpdate {
    pub position_secs: f64,
    pub duration_secs: f64,
    pub watched: bool,
}

/// Debounces progress events into occasional persistence-worthy updates and
/// flags the item watched once past the threshold (default 0.9, TC-PLY-007).
pub struct ProgressTracker {
    watched_threshold: f32,
    debounce_secs: f64,
    last_emit_at: Option<f64>,
    watched_emitted: bool,
}

impl ProgressTracker {
    pub fn new(watched_threshold: f32, debounce_secs: f64) -> Self {
        Self {
            watched_threshold,
            debounce_secs,
            last_emit_at: None,
            watched_emitted: false,
        }
    }

    /// Feed a progress sample; returns an update at most once per debounce
    /// window. Crossing the watched threshold always emits immediately so the
    /// watched flag is never lost to debouncing.
    pub fn on_progress(
        &mut self,
        position_secs: f64,
        duration_secs: f64,
        now_secs: f64,
    ) -> Option<ProgressUpdate> {
        if duration_secs <= 0.0 {
            return None;
        }
        let watched = (position_secs / duration_secs) as f32 >= self.watched_threshold;
        let window_open = match self.last_emit_at {
            Some(t) => now_secs - t >= self.debounce_secs,
            None => true,
        };
        if !window_open && !(watched && !self.watched_emitted) {
            return None;
        }
        self.last_emit_at = Some(now_secs);
        self.watched_emitted |= watched;
        Some(ProgressUpdate {
            position_secs,
            duration_secs,
            watched,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_debounced_and_watched_at_threshold() {
        let mut t = ProgressTracker::new(0.9, 5.0);

        // First sample always emits; not yet watched.
        let u = t.on_progress(10.0, 100.0, 0.0).unwrap();
        assert!(!u.watched);

        // Inside the debounce window: suppressed.
        assert!(t.on_progress(12.0, 100.0, 2.0).is_none());
        assert!(t.on_progress(14.0, 100.0, 4.9).is_none());

        // Window elapsed: emits again.
        let u = t.on_progress(15.0, 100.0, 5.0).unwrap();
        assert_eq!(u.position_secs, 15.0);
        assert!(!u.watched);

        // Crossing the 90% threshold emits immediately even mid-window.
        let u = t.on_progress(90.0, 100.0, 6.0).unwrap();
        assert!(u.watched);

        // Back inside the window with watched already recorded: suppressed.
        assert!(t.on_progress(91.0, 100.0, 7.0).is_none());
    }

    #[test]
    fn zero_duration_never_emits() {
        let mut t = ProgressTracker::new(0.9, 5.0);
        assert!(t.on_progress(10.0, 0.0, 0.0).is_none());
    }
}
