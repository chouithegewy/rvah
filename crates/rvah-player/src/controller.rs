use rvah_addon_sdk::PlayableSource;

use crate::backend::{PlayerBackend, PlayerError, PlayerEvent};

/// Player lifecycle states (SDD §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerState {
    Idle,
    Loading,
    Buffering,
    Playing,
    Paused,
    Seeking,
    Ended,
    Error,
}

/// Owns the playback state machine; the backend only executes commands and
/// reports events. Illegal commands return a non-fatal `Internal` error and
/// leave the state untouched (FR-062).
pub struct PlayerController<B: PlayerBackend> {
    backend: B,
    state: PlayerState,
    position_secs: f64,
    duration_secs: Option<f64>,
    last_error: Option<PlayerError>,
}

impl<B: PlayerBackend> PlayerController<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            state: PlayerState::Idle,
            position_secs: 0.0,
            duration_secs: None,
            last_error: None,
        }
    }

    pub fn state(&self) -> PlayerState {
        self.state
    }

    pub fn position_secs(&self) -> f64 {
        self.position_secs
    }

    pub fn duration_secs(&self) -> Option<f64> {
        self.duration_secs
    }

    pub fn last_error(&self) -> Option<&PlayerError> {
        self.last_error.as_ref()
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    fn illegal(&self, cmd: &str) -> PlayerError {
        PlayerError::internal(format!("illegal transition: {cmd} from {:?}", self.state))
    }

    /// Load a validated source, optionally resuming at `start_at` (FR-065).
    pub fn load_source(
        &mut self,
        source: &PlayableSource,
        start_at: f64,
    ) -> Result<(), PlayerError> {
        self.backend.load(source, start_at)?;
        self.state = PlayerState::Loading;
        self.position_secs = start_at;
        self.duration_secs = None;
        self.last_error = None;
        Ok(())
    }

    pub fn play(&mut self) -> Result<(), PlayerError> {
        match self.state {
            PlayerState::Paused | PlayerState::Buffering | PlayerState::Seeking => {
                self.backend.play()?;
                self.state = PlayerState::Playing;
                Ok(())
            }
            // The source is still opening: unpause now and keep polling while
            // the backend buffers. mpv accepts this before the stream is fully
            // opened, and it avoids "Play" meaning "load paused".
            PlayerState::Loading => {
                self.backend.play()?;
                self.state = PlayerState::Buffering;
                Ok(())
            }
            _ => Err(self.illegal("play")),
        }
    }

    pub fn pause(&mut self) -> Result<(), PlayerError> {
        match self.state {
            PlayerState::Playing | PlayerState::Seeking | PlayerState::Buffering => {
                self.backend.pause()?;
                self.state = PlayerState::Paused;
                Ok(())
            }
            // Backends load paused; pausing here is already satisfied.
            PlayerState::Loading => Ok(()),
            _ => Err(self.illegal("pause")),
        }
    }

    pub fn seek(&mut self, secs: f64) -> Result<(), PlayerError> {
        match self.state {
            PlayerState::Playing | PlayerState::Paused | PlayerState::Buffering => {
                self.backend.seek(secs)?;
                self.state = PlayerState::Seeking;
                self.position_secs = secs;
                Ok(())
            }
            _ => Err(self.illegal("seek")),
        }
    }

    pub fn stop(&mut self) -> Result<(), PlayerError> {
        match self.state {
            PlayerState::Idle => Err(self.illegal("stop")),
            _ => {
                self.backend.stop()?;
                self.state = PlayerState::Idle;
                Ok(())
            }
        }
    }

    pub fn set_volume(&mut self, volume: f32) -> Result<(), PlayerError> {
        self.backend.set_volume(volume.clamp(0.0, 1.0))
    }

    /// Select a subtitle track for the active session (FR-064 direction).
    pub fn select_subtitle(&mut self, index: Option<usize>) -> Result<(), PlayerError> {
        match self.state {
            PlayerState::Idle => Err(self.illegal("select_subtitle")),
            _ => self.backend.select_subtitle(index),
        }
    }

    /// Drain backend events and advance the state machine; returns the events
    /// so callers (progress tracking, UI) can observe them (FR-062).
    pub fn handle_events(&mut self) -> Vec<PlayerEvent> {
        let events = self.backend.poll_events();
        for event in &events {
            match event {
                PlayerEvent::Loaded { duration_secs } => {
                    self.duration_secs = Some(*duration_secs);
                    if self.state == PlayerState::Loading {
                        self.state = PlayerState::Paused;
                    }
                }
                PlayerEvent::Buffering => {
                    if !matches!(self.state, PlayerState::Idle | PlayerState::Error) {
                        self.state = PlayerState::Buffering;
                    }
                }
                PlayerEvent::Progress { position_secs } => {
                    self.position_secs = *position_secs;
                    if matches!(
                        self.state,
                        PlayerState::Buffering | PlayerState::Seeking | PlayerState::Playing
                    ) {
                        self.state = PlayerState::Playing;
                    }
                }
                PlayerEvent::Ended => self.state = PlayerState::Ended,
                PlayerEvent::Error(err) => {
                    self.last_error = Some(err.clone());
                    if err.fatal {
                        self.state = PlayerState::Error;
                    }
                }
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{MockPlayer, PlayerErrorCode};
    use rvah_addon_sdk::{PlayableSource, Protocol};

    fn source() -> PlayableSource {
        PlayableSource {
            url: "https://cdn.example.com/movie.mp4".into(),
            protocol: Protocol::Https,
            headers: Default::default(),
            subtitles: vec![],
            quality: None,
            audio_lang: None,
            expires_at: None,
            drm: None,
        }
    }

    #[test]
    fn controller_full_lifecycle() {
        // TC-PLY-001: load → play → pause → seek → resume → stop.
        let mut c = PlayerController::new(MockPlayer::new());
        assert_eq!(c.state(), PlayerState::Idle);

        c.load_source(&source(), 0.0).unwrap();
        assert_eq!(c.state(), PlayerState::Loading);
        c.backend_mut().script_events([PlayerEvent::Loaded {
            duration_secs: 3600.0,
        }]);
        c.handle_events();
        assert_eq!(c.state(), PlayerState::Paused);
        assert_eq!(c.duration_secs(), Some(3600.0));

        c.play().unwrap();
        assert_eq!(c.state(), PlayerState::Playing);
        c.backend_mut().script_events([PlayerEvent::Progress {
            position_secs: 10.0,
        }]);
        c.handle_events();
        assert_eq!(c.position_secs(), 10.0);

        c.pause().unwrap();
        assert_eq!(c.state(), PlayerState::Paused);

        c.seek(300.0).unwrap();
        assert_eq!(c.state(), PlayerState::Seeking);
        assert_eq!(c.position_secs(), 300.0);

        c.play().unwrap();
        assert_eq!(c.state(), PlayerState::Playing);

        c.stop().unwrap();
        assert_eq!(c.state(), PlayerState::Idle);
        assert_eq!(
            c.backend_mut().commands,
            vec!["load", "play", "pause", "seek:300", "play", "stop"]
        );
    }

    #[test]
    fn subtitle_selection_passes_through_when_active() {
        let mut c = PlayerController::new(MockPlayer::new());
        // No session yet: refused, backend untouched.
        assert!(c.select_subtitle(Some(0)).is_err());
        assert!(c.backend_mut().commands.is_empty());

        c.load_source(&source(), 0.0).unwrap();
        c.select_subtitle(Some(0)).unwrap();
        c.select_subtitle(None).unwrap();
        assert!(c.backend_mut().commands.contains(&"subtitle:0".to_string()));
        assert!(c
            .backend_mut()
            .commands
            .contains(&"subtitle:off".to_string()));
    }

    #[test]
    fn play_during_loading_unpauses_backend_and_buffers() {
        // Real backends load asynchronously (mpv opening a network stream),
        // so play during Loading is legitimate intent, not an error.
        let mut c = PlayerController::new(MockPlayer::new());
        c.load_source(&source(), 0.0).unwrap();
        c.play().unwrap();
        assert_eq!(c.state(), PlayerState::Buffering);
        assert!(c.backend_mut().commands.contains(&"play".to_string()));

        c.backend_mut().script_events([PlayerEvent::Loaded {
            duration_secs: 100.0,
        }]);
        c.handle_events();
        assert_eq!(c.state(), PlayerState::Buffering);
    }

    #[test]
    fn pause_during_loading_or_buffering_keeps_backend_paused() {
        let mut c = PlayerController::new(MockPlayer::new());
        c.load_source(&source(), 0.0).unwrap();
        c.pause().unwrap();
        c.backend_mut().script_events([PlayerEvent::Loaded {
            duration_secs: 100.0,
        }]);
        c.handle_events();
        assert_eq!(c.state(), PlayerState::Paused);
        assert!(!c.backend_mut().commands.contains(&"play".to_string()));

        // Once play has unpaused an opening source, pause reaches the backend.
        c.play().unwrap();
        assert_eq!(c.state(), PlayerState::Playing);
        c.load_source(&source(), 0.0).unwrap();
        c.play().unwrap();
        c.pause().unwrap();
        assert_eq!(c.state(), PlayerState::Paused);
        assert!(c.backend_mut().commands.contains(&"pause".to_string()));
    }

    #[test]
    fn illegal_transition_is_nonfatal_error() {
        let mut c = PlayerController::new(MockPlayer::new());
        let err = c.play().unwrap_err();
        assert_eq!(err.code, PlayerErrorCode::Internal);
        assert!(!err.fatal);
        assert_eq!(c.state(), PlayerState::Idle);
        // Backend never received the command.
        assert!(c.backend_mut().commands.is_empty());
    }

    #[test]
    fn backend_fatal_error_moves_to_error_state() {
        // TC-PLY-010: fatal backend error ends the session in Error state.
        let mut c = PlayerController::new(MockPlayer::new());
        c.load_source(&source(), 0.0).unwrap();
        c.backend_mut()
            .script_events([PlayerEvent::Error(PlayerError {
                code: PlayerErrorCode::Network,
                message: "connection reset".into(),
                fatal: true,
            })]);
        c.handle_events();
        assert_eq!(c.state(), PlayerState::Error);
        assert_eq!(c.last_error().unwrap().code, PlayerErrorCode::Network);
    }

    #[test]
    fn nonfatal_backend_error_keeps_state() {
        let mut c = PlayerController::new(MockPlayer::new());
        c.load_source(&source(), 0.0).unwrap();
        c.backend_mut().script_events([
            PlayerEvent::Loaded {
                duration_secs: 100.0,
            },
            PlayerEvent::Error(PlayerError {
                code: PlayerErrorCode::Network,
                message: "transient".into(),
                fatal: false,
            }),
        ]);
        c.handle_events();
        assert_eq!(c.state(), PlayerState::Paused);
        assert!(c.last_error().is_some());
    }
}
