//! Platform-neutral playback sessions. [`PlaybackTracker`] owns the active
//! session and its watch-state persistence behind its own lock (see
//! [`crate::AppCore::playback_tracker`]), so a native player's callbacks
//! never wait behind a slow add-on invocation holding the core.

use std::sync::{Arc, Mutex, MutexGuard};

use rvah_addon_sdk::{PlayableSource, Protocol, SubtitleTrack};
use rvah_auth::Redactor;
use rvah_data::{Database, DiagnosticEvent, WatchState};
use rvah_player::{PlayerBackend, ProgressTracker, ProgressUpdate};

use crate::errors::CoreError;

const WATCHED_THRESHOLD: f32 = 0.9;
const PROGRESS_DEBOUNCE_SECS: f64 = 5.0;

/// Opaque correlation token shared with a platform-native player. A callback
/// for any older token is ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlaybackSessionId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaybackCapabilities {
    pub progressive_http: bool,
    pub hls: bool,
    pub dash: bool,
    /// Opens web sources in a locked-down web player.
    pub web: bool,
}

impl PlaybackCapabilities {
    pub const fn media3() -> Self {
        Self {
            progressive_http: true,
            hls: true,
            dash: true,
            web: false,
        }
    }

    /// Media3 plus the Android TV app's web player.
    pub const fn android_tv() -> Self {
        Self {
            web: true,
            ..Self::media3()
        }
    }

    pub fn for_backend(backend: &impl PlayerBackend) -> Self {
        Self {
            progressive_http: true,
            hls: true,
            dash: backend.supports_dash(),
            web: false,
        }
    }

    pub(crate) fn supports(self, protocol: Protocol) -> bool {
        match protocol {
            Protocol::Http | Protocol::Https => self.progressive_http,
            Protocol::Hls => self.hls,
            Protocol::Dash => self.dash,
            Protocol::Web => self.web,
            Protocol::File | Protocol::Veilid => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackDisplayMetadata {
    pub title: String,
    pub subtitle: Option<String>,
}

/// Platform-neutral output of source resolution. It deliberately contains no
/// add-on secrets, diagnostics, or unsanitized provider metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedPlayback {
    pub session_id: PlaybackSessionId,
    pub source: PlayableSource,
    pub resume_position_secs: f64,
    pub subtitles: Vec<SubtitleTrack>,
    pub display: PlaybackDisplayMetadata,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlaybackLifecycleEvent {
    Preparing,
    Buffering,
    Playing {
        position_secs: f64,
        duration_secs: Option<f64>,
    },
    Paused {
        position_secs: f64,
        duration_secs: Option<f64>,
    },
    Ended {
        position_secs: f64,
        duration_secs: Option<f64>,
    },
    Error {
        message: String,
    },
}

/// An in-flight playback created by [`crate::AppCore::prepare_playback`].
#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackSession {
    pub id: PlaybackSessionId,
    pub addon_id: String,
    pub item_id: String,
    pub source: PlayableSource,
    pub resume_position_secs: f64,
    pub lifecycle: PlaybackLifecycleEvent,
}

/// The active playback session plus debounced watch-state persistence
/// (FR-065). Shared between [`crate::AppCore`] and platform adapters.
pub struct PlaybackTracker {
    db: Arc<Database>,
    session: Option<PlaybackSession>,
    next_session_id: u64,
    progress: ProgressTracker,
}

impl PlaybackTracker {
    pub(crate) fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            session: None,
            next_session_id: 1,
            progress: ProgressTracker::new(WATCHED_THRESHOLD, PROGRESS_DEBOUNCE_SECS),
        }
    }

    pub fn session(&self) -> Option<&PlaybackSession> {
        self.session.as_ref()
    }

    /// Start a new session, superseding any previous one.
    pub(crate) fn begin(
        &mut self,
        addon_id: &str,
        item_id: &str,
        source: PlayableSource,
        resume_position_secs: f64,
    ) -> PlaybackSessionId {
        self.progress = ProgressTracker::new(WATCHED_THRESHOLD, PROGRESS_DEBOUNCE_SECS);
        let id = PlaybackSessionId(self.next_session_id);
        self.next_session_id = self.next_session_id.wrapping_add(1).max(1);
        self.session = Some(PlaybackSession {
            id,
            addon_id: addon_id.to_string(),
            item_id: item_id.to_string(),
            source,
            resume_position_secs,
            lifecycle: PlaybackLifecycleEvent::Preparing,
        });
        id
    }

    /// Record one native-player callback. Returns `false` for a stale session
    /// and intentionally performs no state or persistence mutation.
    pub fn report(
        &mut self,
        session_id: PlaybackSessionId,
        event: PlaybackLifecycleEvent,
    ) -> Result<bool, CoreError> {
        let Some(session) = self.session.as_mut() else {
            return Ok(false);
        };
        if session.id != session_id {
            return Ok(false);
        }

        let (position, duration, ended) = match &event {
            PlaybackLifecycleEvent::Playing {
                position_secs,
                duration_secs,
            }
            | PlaybackLifecycleEvent::Paused {
                position_secs,
                duration_secs,
            } => (Some(*position_secs), *duration_secs, false),
            PlaybackLifecycleEvent::Ended {
                position_secs,
                duration_secs,
            } => (Some(*position_secs), *duration_secs, true),
            PlaybackLifecycleEvent::Error { message } => {
                let message = Redactor::new().redact(message);
                self.db.log_event(&DiagnosticEvent {
                    level: "error".into(),
                    addon_id: Some(session.addon_id.clone()),
                    code: "player".into(),
                    message,
                })?;
                (None, None, false)
            }
            PlaybackLifecycleEvent::Preparing | PlaybackLifecycleEvent::Buffering => {
                (None, None, false)
            }
        };

        session.lifecycle = event;
        if let Some(position_secs) = position {
            let duration_secs = duration.unwrap_or(0.0);
            let update = if ended {
                Some(ProgressUpdate {
                    position_secs: duration.unwrap_or(position_secs),
                    duration_secs,
                    watched: true,
                })
            } else {
                self.progress
                    .on_progress(position_secs, duration_secs, position_secs)
            };
            if let Some(update) = update {
                self.db.set_watch_state(&WatchState {
                    addon_id: session.addon_id.clone(),
                    item_id: session.item_id.clone(),
                    position_secs: update.position_secs,
                    duration_secs: update.duration_secs,
                    watched: update.watched,
                    last_played_at: crate::app::unix_now(),
                })?;
            }
        }
        if ended {
            self.session = None;
        }
        Ok(true)
    }

    /// End `session_id` if it is still the active session.
    pub fn stop(&mut self, session_id: PlaybackSessionId) -> bool {
        if self.session.as_ref().map(|session| session.id) != Some(session_id) {
            return false;
        }
        self.session = None;
        true
    }
}

/// Lock the shared tracker. A panic elsewhere must not wedge playback
/// reporting for the rest of the process, so poisoning is ignored: the
/// tracker's state is updated field by field and stays consistent.
pub fn lock_tracker(tracker: &Mutex<PlaybackTracker>) -> MutexGuard<'_, PlaybackTracker> {
    tracker
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
