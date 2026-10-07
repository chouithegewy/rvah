//! mpv-backed [`PlayerBackend`] over mpv's JSON IPC protocol (SDD §4
//! `rvah-player-mpv`).
//!
//! No libmpv linkage: [`MpvBackend::spawn`] launches the system `mpv` binary
//! with `--input-ipc-server` and drives it over a Unix socket (unix) or a
//! named pipe (Windows), so the crate compiles and tests everywhere (a fake
//! IPC server stands in for mpv in tests). [`MpvBackend::spawn_embedded`]
//! directs video into an application-owned native child window; RVAH supplies
//! the controls.

mod backend;
mod ipc;

pub use backend::MpvBackend;
pub use ipc::MpvIpc;
