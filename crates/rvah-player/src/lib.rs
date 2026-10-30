//! Player subsystem: backend abstraction, controller state machine, source
//! validation, and watch-progress tracking (FR-060..FR-066, SDD §8).
//!
//! Only resolved, host-validated sources reach a backend ([`validate_source`]);
//! the [`PlayerController`] owns the state machine so backends stay dumb.

mod backend;
mod controller;
mod progress;
mod validate;

pub use backend::{MockPlayer, PlayerBackend, PlayerError, PlayerErrorCode, PlayerEvent};
pub use controller::{PlayerController, PlayerState};
pub use progress::{ProgressTracker, ProgressUpdate};
pub use validate::{
    validate_source, validate_source_opts, validate_source_with, SourcePolicy, SourceRejection,
};

/// Allow runtime-chosen backends (`AppCore<Box<dyn PlayerBackend + Send>>`).
impl PlayerBackend for Box<dyn PlayerBackend + Send> {
    fn load(
        &mut self,
        source: &rvah_addon_sdk::PlayableSource,
        start_at: f64,
    ) -> Result<(), PlayerError> {
        (**self).load(source, start_at)
    }
    fn play(&mut self) -> Result<(), PlayerError> {
        (**self).play()
    }
    fn pause(&mut self) -> Result<(), PlayerError> {
        (**self).pause()
    }
    fn stop(&mut self) -> Result<(), PlayerError> {
        (**self).stop()
    }
    fn seek(&mut self, secs: f64) -> Result<(), PlayerError> {
        (**self).seek(secs)
    }
    fn set_volume(&mut self, volume: f32) -> Result<(), PlayerError> {
        (**self).set_volume(volume)
    }
    fn poll_events(&mut self) -> Vec<PlayerEvent> {
        (**self).poll_events()
    }
    fn supports_dash(&self) -> bool {
        (**self).supports_dash()
    }
    fn select_subtitle(&mut self, index: Option<usize>) -> Result<(), PlayerError> {
        (**self).select_subtitle(index)
    }
}
