//! Use-case orchestration for RVAH: browse/search/resolve flows across
//! installed add-ons, failure counting with quarantine (TC-NFR-005),
//! playback with resume persistence (FR-065), navigation history (FR-042),
//! and favorites.

mod app;
pub mod debug;
mod errors;
mod nav;

pub use app::{
    AppCore, PlaybackCapabilities, PlaybackDisplayMetadata, PlaybackLifecycleEvent,
    PlaybackSession, PlaybackSessionId, PreparedPlayback, DEFAULT_INVOKE_TIMEOUT,
    QUARANTINE_THRESHOLD,
};
pub use errors::{CoreError, ErrorClass};
pub use nav::NavHistory;
