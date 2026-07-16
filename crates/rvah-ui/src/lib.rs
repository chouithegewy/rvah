//! Presentation adapter for RVAH (SDD §4 `rvah-ui`).
//!
//! Three layers keep the toolkit swappable and the logic testable:
//! - [`service`]: `CoreService` runs `AppCore` on its own thread; the UI
//!   talks to it via `CoreRequest`/`CoreEvent` channels.
//! - [`state`]: pure `UiState` + `reduce`/`intent` functions — all screen
//!   logic, no egui types.
//! - [`views`]: thin egui rendering of `UiState`, emitting `UiIntent`s.

pub mod service;
pub mod state;
pub mod views;

pub use service::{CoreEvent, CoreHandle, CoreRequest, CoreService, PlayerCommand};
pub use state::{intent, reduce, ErrorView, Load, Screen, UiIntent, UiState};
pub use views::RvahApp;
