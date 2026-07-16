use std::path::PathBuf;

use rvah_addon_sdk::{Page, PageItem};
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
    Install,
}

/// A signature-verified permission grant awaiting the user's decision
/// (UC-01, FR-005 in the GUI).
#[derive(Debug, Clone, PartialEq)]
pub struct InstallPreviewView {
    pub id: String,
    pub version: String,
    pub network: Vec<String>,
    pub storage_bytes: u64,
    pub secrets: Vec<String>,
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
    /// Subtitle track labels for the active source.
    pub subtitles: Vec<String>,
    pub subtitle_selected: Option<usize>,
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
    pub install_pkg: String,
    pub install_pubkey: String,
    pub install: Load<InstallPreviewView>,
    pub install_done: Option<String>,
    pub banner: Option<ErrorView>,
    /// Index into [`focus_targets`] for D-pad navigation (FR-045).
    pub focus: Option<usize>,
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
            install_pkg: String::new(),
            install_pubkey: String::new(),
            install: Load::Idle,
            install_done: None,
            banner: None,
            focus: None,
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
    OpenInstall,
    SetInstallPkg(String),
    SetInstallPubkey(String),
    PreviewInstall,
    ConfirmInstall,
    CancelInstall,
    DismissBanner,
    /// D-pad focus movement over [`focus_targets`] (FR-045).
    FocusNext,
    FocusPrev,
    /// Dispatch the focused target's intent.
    Activate,
}

/// Apply a user action; returns the request to send to the core, if any.
pub fn intent(state: &mut UiState, intent: UiIntent) -> Option<CoreRequest> {
    match intent {
        UiIntent::OpenHome => {
            state.screen = Screen::Home;
            state.home = Load::Loading;
            state.focus = None;
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
            state.focus = None;
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
            match &cmd {
                PlayerCommand::Volume(v) => state.player.volume = *v,
                PlayerCommand::Subtitle(index) => state.player.subtitle_selected = *index,
                _ => {}
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
        UiIntent::OpenInstall => {
            state.screen = Screen::Install;
            state.install = Load::Idle;
            state.install_done = None;
            None
        }
        UiIntent::SetInstallPkg(path) => {
            state.install_pkg = path;
            None
        }
        UiIntent::SetInstallPubkey(path) => {
            state.install_pubkey = path;
            None
        }
        UiIntent::PreviewInstall => {
            if state.install_pkg.trim().is_empty() || state.install_pubkey.trim().is_empty() {
                return None;
            }
            state.install = Load::Loading;
            Some(CoreRequest::PreviewInstall {
                pkg: PathBuf::from(state.install_pkg.trim()),
                pubkey: PathBuf::from(state.install_pubkey.trim()),
            })
        }
        UiIntent::ConfirmInstall => {
            // Approving is only possible from a verified preview (FR-005).
            if !matches!(state.install, Load::Loaded(_)) {
                return None;
            }
            state.install = Load::Loading;
            Some(CoreRequest::ConfirmInstall {
                pkg: PathBuf::from(state.install_pkg.trim()),
                pubkey: PathBuf::from(state.install_pubkey.trim()),
            })
        }
        UiIntent::CancelInstall => {
            state.install = Load::Idle;
            state.screen = Screen::Home;
            state.home = Load::Loading;
            Some(CoreRequest::LoadHome)
        }
        UiIntent::DismissBanner => {
            state.banner = None;
            None
        }
        UiIntent::FocusNext => {
            let len = focus_targets(state).len();
            if len > 0 {
                state.focus = Some(state.focus.map_or(0, |i| (i + 1).min(len - 1)));
            }
            None
        }
        UiIntent::FocusPrev => {
            let len = focus_targets(state).len();
            if len > 0 {
                state.focus = Some(state.focus.map_or(0, |i| i.saturating_sub(1)));
            }
            None
        }
        UiIntent::Activate => {
            let target = focus_targets(state).into_iter().nth(state.focus?)?;
            crate::state::intent(state, target.intent)
        }
    }
}

/// One row the D-pad can land on (FR-045). Order matches the view's render
/// order, so index `n` is the screen's `n`-th actionable row.
#[derive(Debug, Clone, PartialEq)]
pub struct FocusEntry {
    pub label: String,
    pub intent: UiIntent,
}

/// The focusable rows of the current screen, in render order.
pub fn focus_targets(state: &UiState) -> Vec<FocusEntry> {
    let mut out = Vec::new();
    match state.screen {
        Screen::Home => {
            if let Load::Loaded(home) = &state.home {
                for addon in &home.addons {
                    if matches!(addon.status, rvah_data::AddonStatus::Enabled) {
                        out.push(FocusEntry {
                            label: format!("Open {}", addon.id),
                            intent: UiIntent::OpenBrowse {
                                addon_id: addon.id.clone(),
                                path: None,
                            },
                        });
                    }
                }
                for fav in &home.favorites {
                    out.push(FocusEntry {
                        label: fav.title.clone(),
                        intent: UiIntent::PlayItem {
                            addon_id: fav.addon_id.clone(),
                            item_id: fav.item_id.clone(),
                        },
                    });
                }
                for recent in home.recents.iter().filter(|r| !r.watched) {
                    out.push(FocusEntry {
                        label: format!("{}:{}", recent.addon_id, recent.item_id),
                        intent: UiIntent::PlayItem {
                            addon_id: recent.addon_id.clone(),
                            item_id: recent.item_id.clone(),
                        },
                    });
                }
            }
        }
        Screen::Browse => {
            if let (Some(loc), Load::Loaded(page)) = (&state.browse_loc, &state.browse) {
                for item in &page.items {
                    out.push(match item {
                        PageItem::Directory(d) => FocusEntry {
                            label: d.title.clone(),
                            intent: UiIntent::OpenBrowse {
                                addon_id: loc.addon_id.clone(),
                                path: Some(d.path.clone()),
                            },
                        },
                        PageItem::Media(m) => FocusEntry {
                            label: m.title.clone(),
                            intent: UiIntent::PlayItem {
                                addon_id: loc.addon_id.clone(),
                                item_id: m.id.clone(),
                            },
                        },
                    });
                }
            }
        }
        Screen::Search => {
            if let Load::Loaded(results) = &state.search {
                for (addon_id, result) in results {
                    let Ok(page) = result else { continue };
                    for item in &page.items {
                        if let PageItem::Media(m) = item {
                            out.push(FocusEntry {
                                label: m.title.clone(),
                                intent: UiIntent::PlayItem {
                                    addon_id: addon_id.clone(),
                                    item_id: m.id.clone(),
                                },
                            });
                        }
                    }
                }
            }
        }
        _ => {}
    }
    out
}

/// The intent of the currently focused row, for views to highlight it.
pub fn focused_intent(state: &UiState) -> Option<UiIntent> {
    focus_targets(state)
        .into_iter()
        .nth(state.focus?)
        .map(|e| e.intent)
}

/// Remote-friendly key input (FR-045 direction), decoupled from any toolkit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavKey {
    Home,
    Back,
    Forward,
    PlayPause,
    Stop,
    Up,
    Down,
    Select,
}

/// Map a navigation key to an intent for the current screen. `None` means
/// the key has no meaning right now.
pub fn key_intent(state: &UiState, key: NavKey) -> Option<UiIntent> {
    match key {
        NavKey::Home => Some(UiIntent::OpenHome),
        NavKey::Back => match state.screen {
            Screen::Home => None,
            Screen::Install => Some(UiIntent::CancelInstall),
            Screen::Browse => Some(UiIntent::NavBack),
            _ => Some(UiIntent::OpenHome),
        },
        NavKey::Forward => (state.screen == Screen::Browse).then_some(UiIntent::NavForward),
        NavKey::PlayPause => {
            if state.screen != Screen::Player {
                return None;
            }
            match state.player.state? {
                PlayerState::Playing => Some(UiIntent::Player(PlayerCommand::Pause)),
                _ => Some(UiIntent::Player(PlayerCommand::Play)),
            }
        }
        NavKey::Stop => {
            (state.screen == Screen::Player).then_some(UiIntent::Player(PlayerCommand::Stop))
        }
        NavKey::Up => (!focus_targets(state).is_empty()).then_some(UiIntent::FocusPrev),
        NavKey::Down => (!focus_targets(state).is_empty()).then_some(UiIntent::FocusNext),
        NavKey::Select => state.focus.map(|_| UiIntent::Activate),
    }
}

fn open_browse(state: &mut UiState, loc: BrowseLoc) -> Option<CoreRequest> {
    state.screen = Screen::Browse;
    state.browse = Load::Loading;
    state.focus = None;
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
            state.focus = None;
        }
        CoreEvent::Page { addon_id, path, .. }
            if state.browse_loc.as_ref().map(|l| (&l.addon_id, &l.path))
                != Some((&addon_id, &path)) =>
        {
            // Stale response for a location we already left; drop it.
        }
        CoreEvent::Page { page, .. } => {
            state.browse = Load::Loaded(page);
            state.focus = None;
        }
        CoreEvent::SearchResults { results } => {
            state.search = Load::Loaded(results);
            state.focus = None;
        }
        CoreEvent::PlaybackStarted { url, subtitles, .. } => {
            state.screen = Screen::Player;
            state.player.now_playing = Some(url);
            state.player.subtitles = subtitles;
            state.player.subtitle_selected = None;
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
        CoreEvent::InstallPreview {
            id,
            version,
            network,
            storage_bytes,
            secrets,
        } => {
            state.install = Load::Loaded(InstallPreviewView {
                id,
                version,
                network,
                storage_bytes,
                secrets,
            });
        }
        CoreEvent::Installed { id, version } => {
            state.install = Load::Idle;
            state.install_done = Some(format!("installed {id} v{version}"));
            state.screen = Screen::Home;
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
                "install" => state.install = Load::Failed(error),
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

    fn media(id: &str, title: &str) -> rvah_addon_sdk::PageItem {
        rvah_addon_sdk::PageItem::Media(rvah_addon_sdk::MediaItem {
            id: id.into(),
            content_type: rvah_addon_sdk::ContentType::Movie,
            title: title.into(),
            year: None,
            season: None,
            episode: None,
            runtime_secs: None,
            synopsis: None,
            rating: None,
            artwork: Default::default(),
            provider_ids: Default::default(),
        })
    }

    fn dir(id: &str, title: &str, path: &str) -> rvah_addon_sdk::PageItem {
        rvah_addon_sdk::PageItem::Directory(rvah_addon_sdk::DirectoryItem {
            id: id.into(),
            title: title.into(),
            subtitle: None,
            artwork: Default::default(),
            path: path.into(),
            badges: Vec::new(),
        })
    }

    #[test]
    fn focus_moves_clamped_and_activates_on_browse() {
        let mut state = UiState::default();
        browse_intent(&mut state, "a", None);
        reduce(
            &mut state,
            CoreEvent::Page {
                addon_id: "a".into(),
                path: None,
                page: Page {
                    items: vec![
                        dir("d1", "Movies", "/movies"),
                        media("m1", "Big Buck Bunny"),
                    ],
                    ..Default::default()
                },
            },
        );
        assert_eq!(state.focus, None, "fresh data starts unfocused");

        let targets = focus_targets(&state);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].label, "Movies");

        assert_eq!(intent(&mut state, UiIntent::FocusNext), None);
        assert_eq!(state.focus, Some(0));
        intent(&mut state, UiIntent::FocusNext);
        intent(&mut state, UiIntent::FocusNext);
        assert_eq!(state.focus, Some(1), "clamped at the last target");

        // Activating the focused media item plays it.
        let req = intent(&mut state, UiIntent::Activate);
        assert!(
            matches!(req, Some(CoreRequest::Play { ref item_id, .. }) if item_id == "m1"),
            "{req:?}"
        );

        // Prev clamps at the top; activating a directory browses into it.
        let mut state = UiState::default();
        browse_intent(&mut state, "a", None);
        reduce(
            &mut state,
            CoreEvent::Page {
                addon_id: "a".into(),
                path: None,
                page: Page {
                    items: vec![dir("d1", "Movies", "/movies")],
                    ..Default::default()
                },
            },
        );
        intent(&mut state, UiIntent::FocusPrev);
        intent(&mut state, UiIntent::FocusPrev);
        assert_eq!(state.focus, Some(0));
        let req = intent(&mut state, UiIntent::Activate);
        assert!(
            matches!(req, Some(CoreRequest::Browse { ref path, .. }) if path.as_deref() == Some("/movies"))
        );
    }

    #[test]
    fn home_focus_targets_cover_enabled_addons_favorites_and_recents() {
        let mut state = UiState::default();
        intent(&mut state, UiIntent::OpenHome);
        reduce(
            &mut state,
            CoreEvent::Home {
                addons: vec![
                    rvah_data::AddonRecord {
                        id: "com.example.on".into(),
                        version: "1.0.0".into(),
                        package_hash: String::new(),
                        status: rvah_data::AddonStatus::Enabled,
                        install_source: String::new(),
                        sdk_range: String::new(),
                        capabilities: vec![],
                    },
                    rvah_data::AddonRecord {
                        id: "com.example.off".into(),
                        version: "1.0.0".into(),
                        package_hash: String::new(),
                        status: rvah_data::AddonStatus::Disabled,
                        install_source: String::new(),
                        sdk_range: String::new(),
                        capabilities: vec![],
                    },
                ],
                favorites: vec![rvah_data::FavoriteRecord {
                    addon_id: "com.example.on".into(),
                    item_id: "f1".into(),
                    title: "Fave".into(),
                    artwork_json: String::new(),
                }],
                recents: vec![rvah_data::WatchState {
                    addon_id: "com.example.on".into(),
                    item_id: "r1".into(),
                    position_secs: 10.0,
                    duration_secs: 100.0,
                    watched: false,
                    last_played_at: 0,
                }],
            },
        );
        let targets = focus_targets(&state);
        // Disabled add-on is not focusable; favorite and recent are.
        assert_eq!(targets.len(), 3);
        assert!(matches!(
            &targets[0].intent,
            UiIntent::OpenBrowse { addon_id, .. } if addon_id == "com.example.on"
        ));
        assert!(matches!(
            &targets[1].intent,
            UiIntent::PlayItem { item_id, .. } if item_id == "f1"
        ));
        assert!(matches!(
            &targets[2].intent,
            UiIntent::PlayItem { item_id, .. } if item_id == "r1"
        ));
    }

    #[test]
    fn nav_keys_map_to_focus_intents_only_with_targets() {
        let mut state = UiState::default();
        // Nothing loaded: no focus keys.
        assert_eq!(key_intent(&state, NavKey::Down), None);
        assert_eq!(key_intent(&state, NavKey::Select), None);

        browse_intent(&mut state, "a", None);
        reduce(
            &mut state,
            CoreEvent::Page {
                addon_id: "a".into(),
                path: None,
                page: Page {
                    items: vec![media("m1", "Item")],
                    ..Default::default()
                },
            },
        );
        assert_eq!(key_intent(&state, NavKey::Down), Some(UiIntent::FocusNext));
        assert_eq!(key_intent(&state, NavKey::Up), Some(UiIntent::FocusPrev));
        // Select only acts once something is focused.
        assert_eq!(key_intent(&state, NavKey::Select), None);
        intent(&mut state, UiIntent::FocusNext);
        assert_eq!(key_intent(&state, NavKey::Select), Some(UiIntent::Activate));

        // New page data invalidates the focus.
        reduce(
            &mut state,
            CoreEvent::Page {
                addon_id: "a".into(),
                path: None,
                page: Page::default(),
            },
        );
        assert_eq!(state.focus, None);
    }

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
    fn key_intents_depend_on_screen_and_player_state() {
        let mut state = UiState::default();
        // Home: Back is a no-op, Home key still allowed.
        assert_eq!(key_intent(&state, NavKey::Back), None);
        assert_eq!(key_intent(&state, NavKey::Home), Some(UiIntent::OpenHome));
        assert_eq!(key_intent(&state, NavKey::PlayPause), None);

        // Browse: back/forward map to history.
        browse_intent(&mut state, "a", None);
        assert_eq!(key_intent(&state, NavKey::Back), Some(UiIntent::NavBack));
        assert_eq!(
            key_intent(&state, NavKey::Forward),
            Some(UiIntent::NavForward)
        );

        // Player: space toggles by playback state; S stops.
        state.screen = Screen::Player;
        state.player.state = Some(rvah_player::PlayerState::Playing);
        assert_eq!(
            key_intent(&state, NavKey::PlayPause),
            Some(UiIntent::Player(PlayerCommand::Pause))
        );
        state.player.state = Some(rvah_player::PlayerState::Paused);
        assert_eq!(
            key_intent(&state, NavKey::PlayPause),
            Some(UiIntent::Player(PlayerCommand::Play))
        );
        assert_eq!(
            key_intent(&state, NavKey::Stop),
            Some(UiIntent::Player(PlayerCommand::Stop))
        );
        // Back from a non-browse screen returns home.
        assert_eq!(key_intent(&state, NavKey::Back), Some(UiIntent::OpenHome));

        // Install screen: Back cancels the pending install.
        state.screen = Screen::Install;
        assert_eq!(
            key_intent(&state, NavKey::Back),
            Some(UiIntent::CancelInstall)
        );
    }

    #[test]
    fn subtitle_selection_tracked_and_playback_started_resets_it() {
        let mut state = UiState::default();
        reduce(
            &mut state,
            CoreEvent::PlaybackStarted {
                addon_id: "a".into(),
                item_id: "m1".into(),
                url: "https://cdn.example.com/m1.mp4".into(),
                subtitles: vec!["English".into(), "Deutsch".into()],
            },
        );
        assert_eq!(state.player.subtitles.len(), 2);
        assert_eq!(state.player.subtitle_selected, None);

        let req = intent(
            &mut state,
            UiIntent::Player(PlayerCommand::Subtitle(Some(1))),
        );
        assert_eq!(
            req,
            Some(CoreRequest::Player(PlayerCommand::Subtitle(Some(1))))
        );
        assert_eq!(state.player.subtitle_selected, Some(1));
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
