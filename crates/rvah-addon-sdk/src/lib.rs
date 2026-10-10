//! RVAH add-on SDK: normalized data model, error type, and provider/host traits.
//!
//! Add-ons return catalogs, media items, and playable sources; they never touch
//! UI, sockets, or credentials directly. All host services are reached through
//! [`HostApi`], which the host capability-checks at runtime.

mod error;
#[cfg(feature = "guest")]
pub mod guest;
mod provider;
mod types;

pub use error::{AddonError, AddonErrorCode};
pub use provider::{AddonProvider, HostApi, HttpRequest, HttpResponse, LogLevel};
pub use types::{
    AccountStatus, Artwork, AuthPoll, ContentType, Credentials, DeviceAuth, DirectoryItem,
    DrmScheme, Filter, ItemDetails, MediaItem, Page, PageItem, PlayableSource, Protocol, Rating,
    Release, ResolveResult, ScrobbleAction, ScrobbleItem, SourceKind, SourceRequest,
    SubtitleFormat, SubtitleTrack, Trailer, VideoSite,
};

/// SDK version implemented by this crate. Hosts check manifests' `sdk` range
/// against this value before installing an add-on.
pub const SDK_VERSION: &str = "0.1.0";
