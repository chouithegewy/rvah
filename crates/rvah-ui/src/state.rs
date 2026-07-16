use std::path::PathBuf;

use rvah_addon_sdk::Page;
use rvah_core::ErrorClass;
use rvah_data::{AddonRecord, DiagnosticEvent, FavoriteRecord, WatchState};
use rvah_player::PlayerState;

use crate::service::{CoreEvent, CoreRequest, PlaybackSnapshot, PlayerCommand, SettingEntry};

/// Async data slot with explicit loading/empty/error states (FR-043).
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Load<T> {
    #[default]
    Idle,
    Loading,
    Loaded(T),
    Failed(ErrorView),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ErrorView {
    pub context: String,
    pub message: String,
    pub class: ErrorClass,
    pub retryable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Home,
    Browse,
    Search,
    Player,
    Permissions,
    Settings,
    Diagnostics,
    Auth,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct HomeData {
    pub addons: Vec<AddonRecord>,
    pub favorites: Vec<FavoriteRecord>,
    pub recents: Vec<WatchState>,
}

/// A browse location: add-on plus path within it.
#[derive(Debug, Clone, PartialEq)]
pub struct BrowseLoc {
    pub addon_id: String,
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlayerView {
    pub state: Option<PlayerState>,
    pub position_secs: f64,
    pub duration_secs: Option<f64>,
    pub volume: f32,
    pub now_playing: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AuthView {
    pub status: String,
    pub user_code: Option<String>,
    pub verification_url: Option<String>,
}

/// The whole UI, as plain data. Views render it; [`reduce`] and [`intent`]
/// change it. No egui types anywhere in this module.
pub struct UiState {
    pub screen: Screen,
    pub home: Load<HomeData>,
    pub browse: Load<Page>,
    pub browse_loc: Option<BrowseLoc>,
    pub history_back: Vec<BrowseLoc>,
    pub history_forward: Vec<BrowseLoc>,
    pub search_query: String,
    pub search_addon: Option<String>,
    pub search: Load<Vec<(String, Result<Page, String>)>>,
    pub player: PlayerView,
    pub permissions_addon: Option<String>,
    pub permissions: Load<Option<String>>,
    pub settings_addon: Option<String>,
    pub settings: Load<Vec<SettingEntry>>,
    pub diagnostics: Load<Vec<DiagnosticEvent>>,
    pub export_status: Option<String>,
    pub auth: Load<AuthView>,
    pub banner: Option<ErrorView>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            screen: Screen::Home,
            home: Load::Idle,
            browse: Load::Idle,
            browse_loc: None,
            history_back: Vec::new(),
            history_forward: Vec::new(),
            search_query: String::new(),
            search_addon: None,
            search: Load::Idle,
            player: PlayerView {
                volume: 1.0,
                ..Default::default()
            },
            permissions_addon: None,
            permissions: Load::Idle,
            settings_addon: None,
            settings: Load::Idle,
            diagnostics: Load::Idle,
            export_status: None,
            auth: Load::Idle,
            banner: None,
        }
    }
}

/// What the user did in a view. Views emit these; [`intent`] turns them into
/// state changes plus an optional [`CoreRequest`].
#[derive(Debug, Clone, PartialEq)]
pub enum UiIntent {
    OpenHome,
    OpenBrowse {
        addon_id: String,
        path: Option<String>,
    },
    NavBack,
    NavForward,
    OpenSearch {
        addon_id: Option<String>,
    },
    SetSearchQuery(String),
    SubmitSearch,
    PlayItem {
        addon_id: String,
        item_id: String,
    },
    Player(PlayerCommand),
    OpenPermissions {
        addon_id: String,
    },
    Revoke {
        addon_id: String,
    },
    Grant {
        addon_id: String,
    },
    SetEnabled {
        addon_id: String,
        enabled: bool,
    },
    OpenSettings {
        addon_id: String,
    },
    SaveSetting {
        key: String,
        value: String,
    },
    OpenDiagnostics,
    ExportDiagnostics {
        path: PathBuf,
    },
    AddFavorite {
        addon_id: String,
        item_id: String,
        title: String,
    },
    RemoveFavorite {
        addon_id: String,
        item_id: String,
    },
    OpenAuth {
        addon_id: String,
    },
    PollAuth,
    DismissBanner,
}

/// Apply a user action; returns the request to send to the core, if any.
pub fn intent(state: &mut UiState, intent: UiIntent) -> Option<CoreRequest> {
    match intent {
        UiIntent::OpenHome => {
            state.screen = Screen::Home;
            state.home = Load::Loading;
            Some(CoreRequest::LoadHome)
        }
        UiIntent::OpenBrowse { addon_id, path } => {
            if let Some(loc) = state.browse_loc.take() {
                state.history_back.push(loc);
            }
            state.history_forward.clear();
            open_browse(state, BrowseLoc { addon_id, path })
        }
        UiIntent::NavBack => {
            let prev = state.history_back.pop()?;
            if let Some(cur) = state.browse_loc.take() {
                state.history_forward.push(cur);
            }
            open_browse(state, prev)
        }
        UiIntent::NavForward => {
            let next = state.history_forward.pop()?;
            if let Some(cur) = state.browse_loc.take() {
                state.history_back.push(cur);
            }
            open_browse(state, next)
        }
        UiIntent::OpenSearch { addon_id } => {
            state.screen = Screen::Search;
            state.search_addon = addon_id;
            None
        }
        UiIntent::SetSearchQuery(q) => {
            state.search_query = q;
            None
        }
        UiIntent::SubmitSearch => {
            if state.search_query.trim().is_empty() {
                return None;
            }
            state.search = Load::Loading;
            Some(CoreRequest::Search {
                addon_id: state.search_addon.clone(),
                query: state.search_query.clone(),
            })
        }
        UiIntent::PlayItem { addon_id, item_id } => {
            state.screen = Screen::Player;
            state.player.now_playing = Some(format!("{addon_id}:{item_id}"));
            Some(CoreRequest::Play { addon_id, item_id })
        }
        UiIntent::Player(cmd) => {
            if let PlayerCommand::Volume(v) = &cmd {
                state.player.volume = *v;
            }
            Some(CoreRequest::Player(cmd))
        }
        UiIntent::OpenPermissions { addon_id } => {
            state.screen = Screen::Permissions;
            state.permissions_addon = Some(addon_id.clone());
            state.permissions = Load::Loading;
            Some(CoreRequest::LoadPermissions { addon_id })
        }
        UiIntent::Revoke { addon_id } => {
            state.permissions = Load::Loading;
            Some(CoreRequest::RevokePermissions { addon_id })
        }
        UiIntent::Grant { addon_id } => {
            state.permissions = Load::Loading;
            Some(CoreRequest::GrantPermissions { addon_id })
        }
        UiIntent::SetEnabled { addon_id, enabled } => {
            state.home = Load::Loading;
            Some(CoreRequest::SetEnabled { addon_id, enabled })
        }
        UiIntent::OpenSettings { addon_id } => {
            state.screen = Screen::Settings;
            state.settings_addon = Some(addon_id.clone());
            state.settings = Load::Loading;
            Some(CoreRequest::LoadSettings { addon_id })
        }
        UiIntent::SaveSetting { key, value } => {
            let addon_id = state.settings_addon.clone()?;
            Some(CoreRequest::SaveSetting {
                addon_id,
                key,
                value,
            })
        }
        UiIntent::OpenDiagnostics => {
            state.screen = Screen::Diagnostics;
            state.diagnostics = Load::Loading;
            state.export_status = None;
            Some(CoreRequest::LoadDiagnostics)
        }
        UiIntent::ExportDiagnostics { path } => Some(CoreRequest::ExportDiagnostics { path }),
        UiIntent::AddFavorite {
            addon_id,
            item_id,
            title,
        } => Some(CoreRequest::AddFavorite {
            addon_id,
            item_id,
            title,
        }),
        UiIntent::RemoveFavorite { addon_id, item_id } => {
            Some(CoreRequest::RemoveFavorite { addon_id, item_id })
        }
        UiIntent::OpenAuth { addon_id } => {
            state.screen = Screen::Auth;
            state.auth = Load::Loading;
            Some(CoreRequest::StartAuth { addon_id })
        }
        UiIntent::PollAuth => Some(CoreRequest::PollAuth),
        UiIntent::DismissBanner => {
            state.banner = None;
            None
        }
    }
}

fn open_browse(state: &mut UiState, loc: BrowseLoc) -> Option<CoreRequest> {
    state.screen = Screen::Browse;
    state.browse = Load::Loading;
    state.browse_loc = Some(loc.clone());
    Some(CoreRequest::Browse {
        addon_id: loc.addon_id,
        path: loc.path,
        cursor: None,
    })
}

/// Fold a core event into the UI state.
pub fn reduce(state: &mut UiState, event: CoreEvent) {
    match event {
        CoreEvent::Home {
            addons,
            favorites,
            recents,
        } => {
            state.home = Load::Loaded(HomeData {
                addons,
                favorites,
                recents,
            });
        }
        CoreEvent::Page { addon_id, path, .. }
            if state.browse_loc.as_ref().map(|l| (&l.addon_id, &l.path))
                != Some((&addon_id, &path)) =>
        {
            // Stale response for a location we already left; drop it.
        }
        CoreEvent::Page { page, .. } => {
            state.browse = Load::Loaded(page);
        }
        CoreEvent::SearchResults { results } => {
            state.search = Load::Loaded(results);
        }
        CoreEvent::PlaybackStarted { url, .. } => {
            state.screen = Screen::Player;
            state.player.now_playing = Some(url);
        }
        CoreEvent::Playback(PlaybackSnapshot {
            state: player_state,
            position_secs,
            duration_secs,
        }) => {
            state.player.state = Some(player_state);
            state.player.position_secs = position_secs;
            state.player.duration_secs = duration_secs;
        }
        CoreEvent::Permissions {
            addon_id,
            granted_json,
        } => {
            if state.permissions_addon.as_deref() == Some(addon_id.as_str()) {
                state.permissions = Load::Loaded(granted_json);
            }
        }
        CoreEvent::Settings { addon_id, entries } => {
            if state.settings_addon.as_deref() == Some(addon_id.as_str()) {
                state.settings = Load::Loaded(entries);
            }
        }
        CoreEvent::Diagnostics { events } => {
            state.diagnostics = Load::Loaded(events);
        }
        CoreEvent::Exported { path } => {
            state.export_status = Some(format!("exported to {}", path.display()));
        }
        CoreEvent::Auth {
            status,
            user_code,
            verification_url,
        } => {
            state.auth = Load::Loaded(AuthView {
                status,
                user_code,
                verification_url,
            });
        }
        CoreEvent::Failed {
            context,
            message,
            class,
            retryable,
        } => {
            let error = ErrorView {
                context: context.clone(),
                message,
                class,
                retryable,
            };
            // Route the failure into the slot it belongs to (FR-043) and
            // surface a banner for everything else.
            match context.as_str() {
                "browse" => state.browse = Load::Failed(error),
                "home" => state.home = Load::Failed(error),
                "search" => state.search = Load::Failed(error),
                "permissions" => state.permissions = Load::Failed(error),
                "settings" => state.settings = Load::Failed(error),
                "diagnostics" => state.diagnostics = Load::Failed(error),
                "auth" => state.auth = Load::Failed(error),
                _ => state.banner = Some(error),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvah_core::CoreError;
    use rvah_player::SourceRejection;

    fn browse_intent(state: &mut UiState, addon: &str, path: Option<&str>) -> CoreRequest {
        intent(
            state,
            UiIntent::OpenBrowse {
                addon_id: addon.into(),
                path: path.map(String::from),
            },
        )
        .expect("browse should produce a request")
    }

    #[test]
    fn browse_loading_then_loaded() {
        let mut state = UiState::default();
        let req = browse_intent(&mut state, "a", None);
        assert_eq!(state.screen, Screen::Browse);
        assert_eq!(state.browse, Load::Loading);
        assert!(matches!(req, CoreRequest::Browse { .. }));

        reduce(
            &mut state,
            CoreEvent::Page {
                addon_id: "a".into(),
                path: None,
                page: Page::default(),
            },
        );
        assert!(matches!(state.browse, Load::Loaded(_)));
    }

    #[test]
    fn stale_page_for_left_location_is_dropped() {
        let mut state = UiState::default();
        browse_intent(&mut state, "a", None);
        browse_intent(&mut state, "a", Some("/movies"));
        // Response for the first location arrives late.
        reduce(
            &mut state,
            CoreEvent::Page {
                addon_id: "a".into(),
                path: None,
                page: Page::default(),
            },
        );
        assert_eq!(state.browse, Load::Loading);
    }

    #[test]
    fn nav_history_back_and_forward_reissue_requests() {
        let mut state = UiState::default();
        browse_intent(&mut state, "a", None);
        browse_intent(&mut state, "a", Some("/movies"));

        let back = intent(&mut state, UiIntent::NavBack).unwrap();
        assert!(matches!(back, CoreRequest::Browse { ref path, .. } if path.is_none()));
        assert_eq!(state.browse_loc.as_ref().unwrap().path, None);

        let fwd = intent(&mut state, UiIntent::NavForward).unwrap();
        assert!(
            matches!(fwd, CoreRequest::Browse { ref path, .. } if path.as_deref() == Some("/movies"))
        );
        // Back at the deepest page; nothing further forward.
        assert_eq!(intent(&mut state, UiIntent::NavForward), None);
    }

    #[test]
    fn browse_error_routes_to_browse_slot_with_class() {
        let mut state = UiState::default();
        browse_intent(&mut state, "a", None);
        let err = CoreError::SourceRejected(SourceRejection::Expired);
        reduce(
            &mut state,
            CoreEvent::Failed {
                context: "browse".into(),
                message: err.to_string(),
                class: err.class(),
                retryable: err.retryable(),
            },
        );
        let Load::Failed(view) = &state.browse else {
            panic!("expected failed browse");
        };
        assert_eq!(view.class, ErrorClass::Recoverable);
        assert!(view.retryable);
        assert!(state.banner.is_none());
    }

    #[test]
    fn unrouted_error_becomes_banner() {
        let mut state = UiState::default();
        reduce(
            &mut state,
            CoreEvent::Failed {
                context: "play".into(),
                message: "no sources".into(),
                class: ErrorClass::UserActionable,
                retryable: false,
            },
        );
        assert!(state.banner.is_some());
        assert!(intent(&mut state, UiIntent::DismissBanner).is_none());
        assert!(state.banner.is_none());
    }

    #[test]
    fn empty_search_query_is_not_submitted() {
        let mut state = UiState::default();
        intent(&mut state, UiIntent::OpenSearch { addon_id: None });
        intent(&mut state, UiIntent::SetSearchQuery("   ".into()));
        assert_eq!(intent(&mut state, UiIntent::SubmitSearch), None);

        intent(&mut state, UiIntent::SetSearchQuery("naruto".into()));
        let req = intent(&mut state, UiIntent::SubmitSearch).unwrap();
        assert!(
            matches!(req, CoreRequest::Search { addon_id: None, ref query } if query == "naruto")
        );
    }

    #[test]
    fn settings_and_permissions_ignore_events_for_other_addons() {
        let mut state = UiState::default();
        intent(
            &mut state,
            UiIntent::OpenPermissions {
                addon_id: "a".into(),
            },
        );
        reduce(
            &mut state,
            CoreEvent::Permissions {
                addon_id: "b".into(),
                granted_json: Some("{}".into()),
            },
        );
        assert_eq!(state.permissions, Load::Loading);
        reduce(
            &mut state,
            CoreEvent::Permissions {
                addon_id: "a".into(),
                granted_json: None,
            },
        );
        assert_eq!(state.permissions, Load::Loaded(None));
    }
}
