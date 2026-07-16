//! Permissioned networking for add-ons (FR-080, FR-081).
//!
//! Add-ons never get raw sockets. All outbound HTTP flows through
//! [`PermissionedClient`], which enforces the manifest's domain allowlist,
//! an http(s)-only scheme policy, and a per-invocation request quota —
//! all *before* any transport activity.

mod allowlist;
mod client;
mod mock;
mod resolve;

pub use allowlist::Allowlist;
pub use client::{NetError, PermissionedClient, Transport, UreqTransport};
pub use mock::{MockResolver, MockTransport};
pub use resolve::{Resolve, SystemResolver};
