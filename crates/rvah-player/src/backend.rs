use std::collections::VecDeque;

use rvah_addon_sdk::PlayableSource;
use serde::{Deserialize, Serialize};

/// Structured player error (FR-066). `fatal` errors end the session; others
/// surface to the UI but leave playback recoverable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct PlayerError {
    pub code: PlayerErrorCode,
    pub message: String,
    pub fatal: bool,
}

impl PlayerError {
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: PlayerErrorCode::Internal,
            message: message.into(),
            fatal: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayerErrorCode {
    BackendMissing,
    UnsupportedMedia,
    Network,
    Io,
    Internal,
}

/// Events a backend reports back to the controller.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    Loaded { duration_secs: f64 },
    Buffering,
    Progress { position_secs: f64 },
    Ended,
    Error(PlayerError),
}

/// Minimal imperative surface a media backend must provide (SDD §8).
pub trait PlayerBackend {
    fn load(&mut self, source: &PlayableSource, start_at: f64) -> Result<(), PlayerError>;
    fn play(&mut self) -> Result<(), PlayerError>;
    fn pause(&mut self) -> Result<(), PlayerError>;
    fn stop(&mut self) -> Result<(), PlayerError>;
    fn seek(&mut self, secs: f64) -> Result<(), PlayerError>;
    fn set_volume(&mut self, volume: f32) -> Result<(), PlayerError>;
    /// Drain pending backend events (non-blocking).
    fn poll_events(&mut self) -> Vec<PlayerEvent>;
}

/// Scriptable backend for tests and the developer simulator (STP mock player).
///
/// Push events with [`MockPlayer::script_events`]; they are returned by the
/// next `poll_events`. All commands are recorded in `commands`.
#[derive(Default)]
pub struct MockPlayer {
    queue: VecDeque<PlayerEvent>,
    pub commands: Vec<String>,
    pub loaded_source: Option<PlayableSource>,
    pub last_start_at: Option<f64>,
    pub volume: f32,
    /// When set, the next command returns this error once.
    pub fail_next: Option<PlayerError>,
}

impl MockPlayer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn script_events(&mut self, events: impl IntoIterator<Item = PlayerEvent>) {
        self.queue.extend(events);
    }

    fn command(&mut self, name: &str) -> Result<(), PlayerError> {
        self.commands.push(name.to_string());
        match self.fail_next.take() {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

impl PlayerBackend for MockPlayer {
    fn load(&mut self, source: &PlayableSource, start_at: f64) -> Result<(), PlayerError> {
        self.loaded_source = Some(source.clone());
        self.last_start_at = Some(start_at);
        self.command("load")
    }

    fn play(&mut self) -> Result<(), PlayerError> {
        self.command("play")
    }

    fn pause(&mut self) -> Result<(), PlayerError> {
        self.command("pause")
    }

    fn stop(&mut self) -> Result<(), PlayerError> {
        self.command("stop")
    }

    fn seek(&mut self, secs: f64) -> Result<(), PlayerError> {
        self.command(&format!("seek:{secs}"))
    }

    fn set_volume(&mut self, volume: f32) -> Result<(), PlayerError> {
        self.volume = volume;
        self.command("set_volume")
    }

    fn poll_events(&mut self) -> Vec<PlayerEvent> {
        self.queue.drain(..).collect()
    }
}
