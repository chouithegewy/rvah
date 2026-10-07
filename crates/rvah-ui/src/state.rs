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
    /// The selected item is still being resolved by the add-on, before a
    /// backend player state exists.
    pub resolving: bool,
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
    /// `(row, column)` into [`focus_grid`] for D-pad navigation (FR-045).
    pub focus: Option<(usize, usize)>,
    /// A cursor-follow-up browse request is in flight (FR-024).
    pub loading_more: bool,
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
            loading_more: false,
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
    /// D-pad focus movement over [`focus_grid`] (FR-045): Next/Prev move
    /// rows, Left/Right move within a row.
    FocusNext,
    FocusPrev,
    FocusLeft,
    FocusRight,
    /// Dispatch the focused target's intent.
    Activate,
    /// Fetch the next browse page via `cursor_next` (FR-024).
    LoadMore,
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
            state.player.resolving = true;
            state.player.state = None;
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
            move_focus(state, 1, 0);
            None
        }
        UiIntent::FocusPrev => {
            move_focus(state, -1, 0);
            None
        }
        UiIntent::FocusLeft => {
            move_focus(state, 0, -1);
            None
        }
        UiIntent::FocusRight => {
            move_focus(state, 0, 1);
            None
        }
        UiIntent::Activate => {
            let (row, col) = state.focus?;
            let entry = focus_grid(state)
                .into_iter()
                .nth(row)?
                .into_iter()
                .nth(col)?;
            crate::state::intent(state, entry.intent)
        }
        UiIntent::LoadMore => {
            if state.loading_more {
                return None;
            }
            let Load::Loaded(page) = &state.browse else {
                return None;
            };
            let cursor = page.cursor_next.clone()?;
            let loc = state.browse_loc.clone()?;
            state.loading_more = true;
            Some(CoreRequest::Browse {
                addon_id: loc.addon_id,
                path: loc.path,
                cursor: Some(cursor),
            })
        }
    }
}

/// Move the D-pad focus by whole rows (`dr`) or within a row (`dc`),
/// clamping at the grid edges; an unfocused grid focuses its first cell.
fn move_focus(state: &mut UiState, dr: isize, dc: isize) {
    let grid = focus_grid(state);
    if grid.is_empty() {
        return;
    }
    let Some((row, col)) = state.focus else {
        state.focus = Some((0, 0));
        return;
    };
    let row = row
        .saturating_add_signed(dr)
        .min(grid.len().saturating_sub(1));
    let col = col
        .saturating_add_signed(dc)
        .min(grid[row].len().saturating_sub(1));
    state.focus = Some((row, col));
}

/// One action the D-pad can land on (FR-045).
#[derive(Debug, Clone, PartialEq)]
pub struct FocusEntry {
    pub label: String,
    pub intent: UiIntent,
}

/// The focusable actions of the current screen: outer index is the row in
/// render order, inner index the row's actions left-to-right.
pub fn focus_grid(state: &UiState) -> Vec<Vec<FocusEntry>> {
    let mut out = Vec::new();
    let entry = |label: &str, intent: UiIntent| FocusEntry {
        label: label.to_string(),
        intent,
    };
    match state.screen {
        Screen::Home => {
            if let Load::Loaded(home) = &state.home {
                for addon in &home.addons {
                    let mut row = Vec::new();
                    if matches!(addon.status, rvah_data::AddonStatus::Enabled) {
                        row.push(entry(
                            "Open",
                            UiIntent::OpenBrowse {
                                addon_id: addon.id.clone(),
                                path: None,
                            },
                        ));
                        row.push(entry(
                            "Disable",
                            UiIntent::SetEnabled {
                                addon_id: addon.id.clone(),
                                enabled: false,
                            },
                        ));
                    } else {
                        row.push(entry(
                            "Enable",
                            UiIntent::SetEnabled {
                                addon_id: addon.id.clone(),
                                enabled: true,
                            },
                        ));
                    }
                    row.push(entry(
                        "Permissions",
                        UiIntent::OpenPermissions {
                            addon_id: addon.id.clone(),
                        },
                    ));
                    row.push(entry(
                        "Settings",
                        UiIntent::OpenSettings {
                            addon_id: addon.id.clone(),
                        },
                    ));
                    row.push(entry(
                        "Sign in",
                        UiIntent::OpenAuth {
                            addon_id: addon.id.clone(),
                        },
                    ));
                    out.push(row);
                }
                for fav in &home.favorites {
                    out.push(vec![
                        entry(
                            &fav.title,
                            UiIntent::PlayItem {
                                addon_id: fav.addon_id.clone(),
                                item_id: fav.item_id.clone(),
                            },
                        ),
                        entry(
                            "✕",
                            UiIntent::RemoveFavorite {
                                addon_id: fav.addon_id.clone(),
                                item_id: fav.item_id.clone(),
                            },
                        ),
                    ]);
                }
                for recent in home.recents.iter().filter(|r| !r.watched) {
                    out.push(vec![entry(
                        &format!("{}:{}", recent.addon_id, recent.item_id),
                        UiIntent::PlayItem {
                            addon_id: recent.addon_id.clone(),
                            item_id: recent.item_id.clone(),
                        },
                    )]);
                }
                out.push(vec![
                    entry("Install add-on", UiIntent::OpenInstall),
                    entry("Diagnostics", UiIntent::OpenDiagnostics),
                ]);
            }
        }
        Screen::Browse => {
            if let (Some(loc), Load::Loaded(page)) = (&state.browse_loc, &state.browse) {
                for item in &page.items {
                    out.push(match item {
                        PageItem::Directory(d) => vec![entry(
                            &d.title,
                            UiIntent::OpenBrowse {
                                addon_id: loc.addon_id.clone(),
                                path: Some(d.path.clone()),
                            },
                        )],
                        PageItem::Media(m) => vec![
                            entry(
                                &m.title,
                                UiIntent::PlayItem {
                                    addon_id: loc.addon_id.clone(),
                                    item_id: m.id.clone(),
                                },
                            ),
                            entry(
                                "☆",
                                UiIntent::AddFavorite {
                                    addon_id: loc.addon_id.clone(),
                                    item_id: m.id.clone(),
                                    title: m.title.clone(),
                                },
                            ),
                        ],
                    });
                }
                if page.cursor_next.is_some() {
                    out.push(vec![entry("Load more", UiIntent::LoadMore)]);
                }
            }
        }
        Screen::Search => {
            if let Load::Loaded(results) = &state.search {
                for (addon_id, result) in results {
                    let Ok(page) = result else { continue };
                    for item in &page.items {
                        if let PageItem::Media(m) = item {
                            out.push(vec![entry(
                                &m.title,
                                UiIntent::PlayItem {
                                    addon_id: addon_id.clone(),
                                    item_id: m.id.clone(),
                                },
                            )]);
                        }
                    }
                }
            }
        }
        Screen::Player => {
            let mut row = Vec::new();
            match state.player.state {
                Some(PlayerState::Playing | PlayerState::Buffering | PlayerState::Seeking) => {
                    row.push(entry("Pause", UiIntent::Player(PlayerCommand::Pause)));
                }
                Some(PlayerState::Loading | PlayerState::Paused) => {
                    row.push(entry("Play", UiIntent::Player(PlayerCommand::Play)));
                }
                _ => {}
            }
            if matches!(
                state.player.state,
                Some(
                    PlayerState::Loading
                        | PlayerState::Buffering
                        | PlayerState::Playing
                        | PlayerState::Paused
                        | PlayerState::Seeking
                        | PlayerState::Ended
                        | PlayerState::Error
                )
            ) {
                row.push(entry("Stop", UiIntent::Player(PlayerCommand::Stop)));
            }
            if !row.is_empty() {
                out.push(row);
            }
        }
        _ => {}
    }
    out
}

/// The intent of the currently focused action, for views to highlight it.
pub fn focused_intent(state: &UiState) -> Option<UiIntent> {
    let (row, col) = state.focus?;
    focus_grid(state)
        .into_iter()
        .nth(row)?
        .into_iter()
        .nth(col)
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
    Left,
    Right,
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
                PlayerState::Playing | PlayerState::Buffering | PlayerState::Seeking => {
                    Some(UiIntent::Player(PlayerCommand::Pause))
                }
                PlayerState::Loading | PlayerState::Paused => {
                    Some(UiIntent::Player(PlayerCommand::Play))
                }
                PlayerState::Idle | PlayerState::Ended | PlayerState::Error => None,
            }
        }
        NavKey::Stop => (state.screen == Screen::Player
            && matches!(
                state.player.state,
                Some(
                    PlayerState::Loading
                        | PlayerState::Buffering
                        | PlayerState::Playing
                        | PlayerState::Paused
                        | PlayerState::Seeking
                        | PlayerState::Ended
                        | PlayerState::Error
                )
            ))
        .then_some(UiIntent::Player(PlayerCommand::Stop)),
        NavKey::Up => (!focus_grid(state).is_empty()).then_some(UiIntent::FocusPrev),
        NavKey::Down => (!focus_grid(state).is_empty()).then_some(UiIntent::FocusNext),
        NavKey::Left => (!focus_grid(state).is_empty()).then_some(UiIntent::FocusLeft),
        NavKey::Right => (!focus_grid(state).is_empty()).then_some(UiIntent::FocusRight),
        NavKey::Select => state.focus.map(|_| UiIntent::Activate),
    }
}

fn open_browse(state: &mut UiState, loc: BrowseLoc) -> Option<CoreRequest> {
    state.screen = Screen::Browse;
    state.browse = Load::Loading;
    state.focus = None;
    state.loading_more = false;
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
            if state.loading_more {
                // Cursor follow-up (FR-024): grow the page in place so
                // focus and scroll position stay valid.
                state.loading_more = false;
                if let Load::Loaded(existing) = &mut state.browse {
                    existing.items.extend(page.items);
                    existing.cursor_next = page.cursor_next;
                    existing.total_hint = page.total_hint;
                    return;
                }
            }
            state.browse = Load::Loaded(page);
            state.focus = None;
        }
        CoreEvent::SearchResults { results } => {
            state.search = Load::Loaded(results);
            state.focus = None;
        }
        CoreEvent::PlaybackStarted { url, subtitles, .. } => {
            state.screen = Screen::Player;
            state.player.resolving = false;
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
            if state.screen == Screen::Player
                && state.focus.is_none()
                && !focus_grid(state).is_empty()
            {
                state.focus = Some((0, 0));
            }
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
            if context == "play" {
                state.player.resolving = false;
            }
            let error = ErrorView {
                context: context.clone(),
                message,
                class,
                retryable,
            };
            // Route the failure into the slot it belongs to (FR-043) and
            // surface a banner for everything else.
            match context.as_str() {
                // A failed load-more must not nuke the loaded page.
                "browse" if state.loading_more => {
                    state.loading_more = false;
                    state.banner = Some(error);
                }
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

        let grid = focus_grid(&state);
        assert_eq!(grid.len(), 2);
        assert_eq!(grid[0][0].label, "Movies");
        // A media row has two actions: play and favorite.
        assert_eq!(grid[1].len(), 2);

        assert_eq!(intent(&mut state, UiIntent::FocusNext), None);
        assert_eq!(state.focus, Some((0, 0)));
        intent(&mut state, UiIntent::FocusNext);
        intent(&mut state, UiIntent::FocusNext);
        assert_eq!(state.focus, Some((1, 0)), "clamped at the last row");

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
        assert_eq!(state.focus, Some((0, 0)));
        let req = intent(&mut state, UiIntent::Activate);
        assert!(
            matches!(req, Some(CoreRequest::Browse { ref path, .. }) if path.as_deref() == Some("/movies"))
        );
    }

    #[test]
    fn focus_moves_within_rows_and_clamps_columns() {
        let mut state = UiState::default();
        browse_intent(&mut state, "a", None);
        reduce(
            &mut state,
            CoreEvent::Page {
                addon_id: "a".into(),
                path: None,
                page: Page {
                    items: vec![media("m1", "Two Actions"), dir("d1", "One", "/one")],
                    ..Default::default()
                },
            },
        );
        // Right with nothing focused lands on the first cell.
        intent(&mut state, UiIntent::FocusRight);
        assert_eq!(state.focus, Some((0, 0)));
        // Move to the favorite action; clamp at the row edge.
        intent(&mut state, UiIntent::FocusRight);
        intent(&mut state, UiIntent::FocusRight);
        assert_eq!(state.focus, Some((0, 1)));
        // Activating the favorite action favorites, not plays.
        let req = intent(&mut state, UiIntent::Activate);
        assert!(
            matches!(req, Some(CoreRequest::AddFavorite { ref item_id, .. }) if item_id == "m1"),
            "{req:?}"
        );
        // Moving down into a shorter row clamps the column.
        state.focus = Some((0, 1));
        intent(&mut state, UiIntent::FocusNext);
        assert_eq!(state.focus, Some((1, 0)));
        // Left clamps at zero.
        intent(&mut state, UiIntent::FocusLeft);
        assert_eq!(state.focus, Some((1, 0)));
    }

    #[test]
    fn load_more_appends_and_keeps_focus() {
        let mut state = UiState::default();
        browse_intent(&mut state, "a", None);
        reduce(
            &mut state,
            CoreEvent::Page {
                addon_id: "a".into(),
                path: None,
                page: Page {
                    items: vec![media("m1", "First")],
                    cursor_next: Some("c1".into()),
                    ..Default::default()
                },
            },
        );
        // The pending cursor adds a Load-more row after the items.
        let grid = focus_grid(&state);
        assert_eq!(grid.len(), 2);
        assert_eq!(grid[1][0].intent, UiIntent::LoadMore);

        intent(&mut state, UiIntent::FocusNext); // focus (0,0)
        let req = intent(&mut state, UiIntent::LoadMore);
        assert!(
            matches!(
                req,
                Some(CoreRequest::Browse { ref cursor, .. }) if cursor.as_deref() == Some("c1")
            ),
            "{req:?}"
        );
        assert!(state.loading_more);
        // Duplicate requests are swallowed while one is in flight.
        assert_eq!(intent(&mut state, UiIntent::LoadMore), None);

        // The next page appends; focus and earlier items survive.
        reduce(
            &mut state,
            CoreEvent::Page {
                addon_id: "a".into(),
                path: None,
                page: Page {
                    items: vec![media("m2", "Second")],
                    cursor_next: None,
                    ..Default::default()
                },
            },
        );
        assert!(!state.loading_more);
        assert_eq!(state.focus, Some((0, 0)));
        let Load::Loaded(page) = &state.browse else {
            panic!("browse not loaded");
        };
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.cursor_next, None);
        // Cursor consumed: the Load-more row is gone.
        assert_eq!(focus_grid(&state).len(), 2);

        // Without a cursor, LoadMore is inert.
        assert_eq!(intent(&mut state, UiIntent::LoadMore), None);
    }

    #[test]
    fn failed_load_more_keeps_page_and_banners() {
        let mut state = UiState::default();
        browse_intent(&mut state, "a", None);
        reduce(
            &mut state,
            CoreEvent::Page {
                addon_id: "a".into(),
                path: None,
                page: Page {
                    items: vec![media("m1", "First")],
                    cursor_next: Some("c1".into()),
                    ..Default::default()
                },
            },
        );
        intent(&mut state, UiIntent::LoadMore);
        reduce(
            &mut state,
            CoreEvent::Failed {
                context: "browse".into(),
                message: "provider hiccup".into(),
                class: ErrorClass::Recoverable,
                retryable: true,
            },
        );
        assert!(!state.loading_more);
        assert!(state.banner.is_some(), "failure surfaces as a banner");
        assert!(
            matches!(state.browse, Load::Loaded(_)),
            "loaded page survives a failed load-more"
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
        let grid = focus_grid(&state);
        // Two add-on rows, one favorite row, one recent row, one tools row.
        assert_eq!(grid.len(), 5);
        // Enabled add-on: Open, Disable, Permissions, Settings, Sign in.
        assert_eq!(grid[0].len(), 5);
        assert!(matches!(
            &grid[0][0].intent,
            UiIntent::OpenBrowse { addon_id, .. } if addon_id == "com.example.on"
        ));
        assert!(matches!(
            &grid[0][1].intent,
            UiIntent::SetEnabled { enabled: false, .. }
        ));
        // Disabled add-on: Enable, Permissions, Settings, Sign in — no Open.
        assert_eq!(grid[1].len(), 4);
        assert!(matches!(
            &grid[1][0].intent,
            UiIntent::SetEnabled { addon_id, enabled: true } if addon_id == "com.example.off"
        ));
        // Favorite row: play + remove.
        assert!(matches!(
            &grid[2][0].intent,
            UiIntent::PlayItem { item_id, .. } if item_id == "f1"
        ));
        assert!(matches!(
            &grid[2][1].intent,
            UiIntent::RemoveFavorite { item_id, .. } if item_id == "f1"
        ));
        // Recent row: play.
        assert!(matches!(
            &grid[3][0].intent,
            UiIntent::PlayItem { item_id, .. } if item_id == "r1"
        ));
        assert_eq!(grid[4][0].intent, UiIntent::OpenInstall);
        assert_eq!(grid[4][1].intent, UiIntent::OpenDiagnostics);
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
        assert_eq!(key_intent(&state, NavKey::Left), Some(UiIntent::FocusLeft));
        assert_eq!(
            key_intent(&state, NavKey::Right),
            Some(UiIntent::FocusRight)
        );
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

    #[test]
    fn player_controls_are_focusable_for_remote_select() {
        let mut state = UiState::default();
        state.screen = Screen::Player;
        reduce(
            &mut state,
            CoreEvent::Playback(PlaybackSnapshot {
                state: PlayerState::Buffering,
                position_secs: 0.0,
                duration_secs: None,
            }),
        );
        assert_eq!(state.focus, Some((0, 0)));

        let grid = focus_grid(&state);
        assert_eq!(grid.len(), 1);
        assert_eq!(grid[0].len(), 2);
        assert_eq!(grid[0][0].label, "Pause");
        assert_eq!(grid[0][1].label, "Stop");
        assert_eq!(key_intent(&state, NavKey::Select), Some(UiIntent::Activate));

        let req = intent(&mut state, UiIntent::Activate);
        assert_eq!(req, Some(CoreRequest::Player(PlayerCommand::Pause)));

        state.player.state = Some(PlayerState::Paused);
        assert_eq!(focus_grid(&state)[0][0].label, "Play");
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
        state.player.state = Some(rvah_player::PlayerState::Buffering);
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
        state.player.state = Some(rvah_player::PlayerState::Idle);
        assert_eq!(key_intent(&state, NavKey::PlayPause), None);
        assert_eq!(key_intent(&state, NavKey::Stop), None);
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
                session_id: 1,
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
