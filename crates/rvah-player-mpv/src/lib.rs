//! mpv-backed [`PlayerBackend`] over mpv's JSON IPC protocol (SDD §4
//! `rvah-player-mpv`).
//!
//! No libmpv linkage: [`MpvBackend::spawn`] launches the system `mpv` binary
//! with `--input-ipc-server` and drives it over a Unix socket, so the crate
//! compiles and tests everywhere (a fake IPC server stands in for mpv in
//! tests). Video renders in mpv's own window; RVAH supplies the controls.
//!
//! Unix only for now — Windows named-pipe transport is deferred.

mod backend;
mod ipc;

pub use backend::MpvBackend;
pub use ipc::MpvIpc;
