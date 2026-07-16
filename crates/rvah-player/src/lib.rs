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
pub use validate::{validate_source, SourceRejection};
