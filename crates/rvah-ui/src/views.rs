//! Thin egui rendering of [`UiState`]. Everything here only reads state and
//! emits [`UiIntent`]s; all logic lives in [`crate::state`].

use std::time::{Duration, Instant};

use eframe::egui;
use rvah_addon_sdk::PageItem;
use rvah_core::ErrorClass;
use rvah_player::PlayerState;

use crate::service::{CoreHandle, CoreRequest, PlayerCommand};
use crate::state::{intent, reduce, ErrorView, Load, Screen, UiIntent, UiState};

/// The eframe application shell.
pub struct RvahApp {
    state: UiState,
    handle: CoreHandle,
    /// Shown when the app fell back to the mock player (mpv not found).
    pub backend_note: Option<String>,
    last_auth_poll: Instant,
}

impl RvahApp {
    pub fn new(handle: CoreHandle) -> Self {
        let mut state = UiState::default();
        // Initial load.
        if let Some(req) = intent(&mut state, UiIntent::OpenHome) {
            let _ = handle.requests.send(req);
        }
        Self {
            state,
            handle,
            backend_note: None,
            last_auth_poll: Instant::now(),
        }
    }

    fn dispatch(&mut self, intents: Vec<UiIntent>) {
        for i in intents {
            if let Some(req) = intent(&mut self.state, i) {
                let _ = self.handle.requests.send(req);
            }
        }
    }
}

impl eframe::App for RvahApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(event) = self.handle.events.try_recv() {
            reduce(&mut self.state, event);
        }

        // Keep playback position/state fresh while a session is active.
        if self.state.screen == Screen::Player
            && matches!(
                self.state.player.state,
                Some(PlayerState::Playing | PlayerState::Buffering | PlayerState::Seeking)
            )
        {
            let _ = self.handle.requests.send(CoreRequest::TickPlayback);
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        // Poll a pending device-code flow about once a second.
        if self.state.screen == Screen::Auth
            && self.last_auth_poll.elapsed() > Duration::from_secs(1)
        {
            if let Load::Loaded(auth) = &self.state.auth {
                if auth.status == "pending" {
                    let _ = self.handle.requests.send(CoreRequest::PollAuth);
                }
            }
            self.last_auth_poll = Instant::now();
            ctx.request_repaint_after(Duration::from_secs(1));
        }

        let mut intents = Vec::new();

        egui::TopBottomPanel::top("nav").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("⌂ Home").clicked() {
                    intents.push(UiIntent::OpenHome);
                }
                if ui.button("← Back").clicked() {
                    intents.push(UiIntent::NavBack);
                }
                if ui.button("→ Forward").clicked() {
                    intents.push(UiIntent::NavForward);
                }
                if ui.button("🔍 Search").clicked() {
                    intents.push(UiIntent::OpenSearch { addon_id: None });
                }
                if ui.button("🛠 Diagnostics").clicked() {
                    intents.push(UiIntent::OpenDiagnostics);
                }
                if ui.button("📦 Install").clicked() {
                    intents.push(UiIntent::OpenInstall);
                }
                if let Some(note) = &self.backend_note {
                    ui.separator();
                    ui.colored_label(egui::Color32::YELLOW, note);
                }
            });
        });

        if let Some(banner) = self.state.banner.clone() {
            egui::TopBottomPanel::top("banner").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    error_label(ui, &banner);
                    if ui.button("dismiss").clicked() {
                        intents.push(UiIntent::DismissBanner);
                    }
                });
            });
        }

        egui::CentralPanel::default().show(ctx, |ui| match self.state.screen {
            Screen::Home => home_view(ui, &self.state, &mut intents),
            Screen::Browse => browse_view(ui, &self.state, &mut intents),
            Screen::Search => search_view(ui, &mut self.state, &mut intents),
            Screen::Player => player_view(ui, &self.state, &mut intents),
            Screen::Permissions => permissions_view(ui, &self.state, &mut intents),
            Screen::Settings => settings_view(ui, &self.state, &mut intents),
            Screen::Diagnostics => diagnostics_view(ui, &self.state, &mut intents),
            Screen::Auth => auth_view(ui, &self.state, &mut intents),
            Screen::Install => install_view(ui, &mut self.state, &mut intents),
        });

        self.dispatch(intents);
    }
}

fn error_label(ui: &mut egui::Ui, error: &ErrorView) {
    let color = match error.class {
        ErrorClass::UserActionable => egui::Color32::LIGHT_BLUE,
        ErrorClass::Recoverable => egui::Color32::YELLOW,
        ErrorClass::Developer => egui::Color32::LIGHT_RED,
        ErrorClass::FatalHost => egui::Color32::RED,
    };
    ui.colored_label(color, format!("[{}] {}", error.context, error.message));
}

/// Render a `Load` slot's non-loaded states; returns the payload when loaded.
fn loaded<'a, T>(ui: &mut egui::Ui, load: &'a Load<T>, what: &str) -> Option<&'a T> {
    match load {
        Load::Idle | Load::Loading => {
            ui.spinner();
            ui.label(format!("loading {what}…"));
            None
        }
        Load::Failed(error) => {
            error_label(ui, error);
            None
        }
        Load::Loaded(value) => Some(value),
    }
}

fn home_view(ui: &mut egui::Ui, state: &UiState, intents: &mut Vec<UiIntent>) {
    ui.heading("RVAH");
    if let Some(done) = &state.install_done {
        ui.colored_label(egui::Color32::GREEN, done);
    }
    let Some(home) = loaded(ui, &state.home, "home") else {
        return;
    };
    ui.separator();
    ui.heading("Add-ons");
    if home.addons.is_empty() {
        ui.label("No add-ons installed. Use `rvah install` to add one.");
    }
    for addon in &home.addons {
        ui.horizontal(|ui| {
            ui.label(format!(
                "{} v{} ({:?})",
                addon.id, addon.version, addon.status
            ));
            let enabled = matches!(addon.status, rvah_data::AddonStatus::Enabled);
            if enabled {
                if ui.button("Open").clicked() {
                    intents.push(UiIntent::OpenBrowse {
                        addon_id: addon.id.clone(),
                        path: None,
                    });
                }
                if ui.button("Disable").clicked() {
                    intents.push(UiIntent::SetEnabled {
                        addon_id: addon.id.clone(),
                        enabled: false,
                    });
                }
            } else if ui.button("Enable").clicked() {
                intents.push(UiIntent::SetEnabled {
                    addon_id: addon.id.clone(),
                    enabled: true,
                });
            }
            if ui.button("Permissions").clicked() {
                intents.push(UiIntent::OpenPermissions {
                    addon_id: addon.id.clone(),
                });
            }
            if ui.button("Settings").clicked() {
                intents.push(UiIntent::OpenSettings {
                    addon_id: addon.id.clone(),
                });
            }
            if ui.button("Sign in").clicked() {
                intents.push(UiIntent::OpenAuth {
                    addon_id: addon.id.clone(),
                });
            }
        });
    }
    ui.separator();
    ui.heading("Favorites");
    if home.favorites.is_empty() {
        ui.label("Nothing favorited yet.");
    }
    for fav in &home.favorites {
        ui.horizontal(|ui| {
            if ui.button(&fav.title).clicked() {
                intents.push(UiIntent::PlayItem {
                    addon_id: fav.addon_id.clone(),
                    item_id: fav.item_id.clone(),
                });
            }
            if ui.small_button("✕").clicked() {
                intents.push(UiIntent::RemoveFavorite {
                    addon_id: fav.addon_id.clone(),
                    item_id: fav.item_id.clone(),
                });
            }
        });
    }
    ui.separator();
    ui.heading("Continue watching");
    for recent in home.recents.iter().filter(|r| !r.watched) {
        if ui
            .button(format!(
                "{}:{} — {:.0}s of {:.0}s",
                recent.addon_id, recent.item_id, recent.position_secs, recent.duration_secs
            ))
            .clicked()
        {
            intents.push(UiIntent::PlayItem {
                addon_id: recent.addon_id.clone(),
                item_id: recent.item_id.clone(),
            });
        }
    }
}

fn browse_view(ui: &mut egui::Ui, state: &UiState, intents: &mut Vec<UiIntent>) {
    let Some(loc) = &state.browse_loc else {
        ui.label("Nothing open.");
        return;
    };
    ui.heading(format!(
        "{} — {}",
        loc.addon_id,
        loc.path.as_deref().unwrap_or("/")
    ));
    let Some(page) = loaded(ui, &state.browse, "page") else {
        return;
    };
    if page.items.is_empty() {
        ui.label("This page is empty.");
        return;
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for item in &page.items {
            match item {
                PageItem::Directory(dir) => {
                    if ui.button(format!("📁 {}", dir.title)).clicked() {
                        intents.push(UiIntent::OpenBrowse {
                            addon_id: loc.addon_id.clone(),
                            path: Some(dir.path.clone()),
                        });
                    }
                }
                PageItem::Media(media) => {
                    ui.horizontal(|ui| {
                        if ui.button(format!("▶ {}", media.title)).clicked() {
                            intents.push(UiIntent::PlayItem {
                                addon_id: loc.addon_id.clone(),
                                item_id: media.id.clone(),
                            });
                        }
                        if let Some(year) = media.year {
                            ui.weak(year.to_string());
                        }
                        if ui.small_button("☆").clicked() {
                            intents.push(UiIntent::AddFavorite {
                                addon_id: loc.addon_id.clone(),
                                item_id: media.id.clone(),
                                title: media.title.clone(),
                            });
                        }
                    });
                }
            }
        }
        if page.cursor_next.is_some() {
            ui.weak("more items available (pagination cursor present)");
        }
    });
}

fn search_view(ui: &mut egui::Ui, state: &mut UiState, intents: &mut Vec<UiIntent>) {
    ui.heading(match &state.search_addon {
        Some(id) => format!("Search {id}"),
        None => "Search all add-ons".to_string(),
    });
    ui.horizontal(|ui| {
        let response = ui.text_edit_singleline(&mut state.search_query);
        let submitted = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui.button("Search").clicked() || submitted {
            intents.push(UiIntent::SubmitSearch);
        }
    });
    if matches!(state.search, Load::Idle) {
        return;
    }
    let Some(results) = loaded(ui, &state.search, "results") else {
        return;
    };
    for (addon_id, result) in results {
        ui.separator();
        ui.strong(addon_id);
        match result {
            Err(message) => {
                ui.colored_label(egui::Color32::LIGHT_RED, message);
            }
            Ok(page) if page.items.is_empty() => {
                ui.label("no results");
            }
            Ok(page) => {
                for item in &page.items {
                    if let PageItem::Media(media) = item {
                        if ui.button(format!("▶ {}", media.title)).clicked() {
                            intents.push(UiIntent::PlayItem {
                                addon_id: addon_id.clone(),
                                item_id: media.id.clone(),
                            });
                        }
                    }
                }
            }
        }
    }
}

fn player_view(ui: &mut egui::Ui, state: &UiState, intents: &mut Vec<UiIntent>) {
    ui.heading("Player");
    if let Some(now_playing) = &state.player.now_playing {
        ui.label(now_playing);
    }
    let player_state = state.player.state;
    ui.label(format!("state: {player_state:?}"));

    let duration = state.player.duration_secs.unwrap_or(0.0);
    let mut position = state.player.position_secs;
    ui.horizontal(|ui| {
        ui.label(format!("{:.0}s / {:.0}s", position, duration));
        if duration > 0.0 {
            let slider = ui.add(egui::Slider::new(&mut position, 0.0..=duration).show_value(false));
            if slider.drag_stopped() {
                intents.push(UiIntent::Player(PlayerCommand::Seek(position)));
            }
        }
    });
    ui.horizontal(|ui| {
        match player_state {
            Some(PlayerState::Playing) => {
                if ui.button("⏸ Pause").clicked() {
                    intents.push(UiIntent::Player(PlayerCommand::Pause));
                }
            }
            _ => {
                if ui.button("▶ Play").clicked() {
                    intents.push(UiIntent::Player(PlayerCommand::Play));
                }
            }
        }
        if ui.button("⏹ Stop").clicked() {
            intents.push(UiIntent::Player(PlayerCommand::Stop));
        }
        let mut volume = state.player.volume;
        if ui
            .add(egui::Slider::new(&mut volume, 0.0..=1.0).text("vol"))
            .changed()
        {
            intents.push(UiIntent::Player(PlayerCommand::Volume(volume)));
        }
    });
}

fn permissions_view(ui: &mut egui::Ui, state: &UiState, intents: &mut Vec<UiIntent>) {
    let Some(addon_id) = state.permissions_addon.clone() else {
        return;
    };
    ui.heading(format!("Permissions — {addon_id}"));
    let Some(granted) = loaded(ui, &state.permissions, "permissions") else {
        return;
    };
    match granted {
        Some(json) => {
            ui.label("Granted permission set:");
            ui.code(json);
            if ui.button("Revoke").clicked() {
                intents.push(UiIntent::Revoke {
                    addon_id: addon_id.clone(),
                });
            }
        }
        None => {
            ui.label("No permissions granted (revoked or never approved).");
            if ui.button("Re-approve manifest permissions").clicked() {
                intents.push(UiIntent::Grant {
                    addon_id: addon_id.clone(),
                });
            }
        }
    }
}

fn settings_view(ui: &mut egui::Ui, state: &UiState, intents: &mut Vec<UiIntent>) {
    let Some(addon_id) = &state.settings_addon else {
        return;
    };
    ui.heading(format!("Settings — {addon_id}"));
    let Some(entries) = loaded(ui, &state.settings, "settings") else {
        return;
    };
    if entries.is_empty() {
        ui.label("This add-on declares no settings.");
        return;
    }
    for entry in entries {
        ui.horizontal(|ui| {
            ui.label(&entry.key);
            if !entry.options.is_empty() {
                for option in &entry.options {
                    let selected = *option == entry.value;
                    if ui.selectable_label(selected, option).clicked() && !selected {
                        intents.push(UiIntent::SaveSetting {
                            key: entry.key.clone(),
                            value: option.clone(),
                        });
                    }
                }
            } else if entry.value_type == "bool" {
                let mut on = entry.value == "true";
                if ui.checkbox(&mut on, "").changed() {
                    intents.push(UiIntent::SaveSetting {
                        key: entry.key.clone(),
                        value: on.to_string(),
                    });
                }
            } else {
                let mut value = entry.value.clone();
                let response = ui.text_edit_singleline(&mut value);
                if response.lost_focus() && value != entry.value {
                    intents.push(UiIntent::SaveSetting {
                        key: entry.key.clone(),
                        value,
                    });
                }
            }
        });
    }
}

fn diagnostics_view(ui: &mut egui::Ui, state: &UiState, intents: &mut Vec<UiIntent>) {
    ui.heading("Diagnostics");
    ui.horizontal(|ui| {
        if ui.button("Export bundle").clicked() {
            let path = std::env::temp_dir().join(format!("rvah-diagnostics-{}.json", {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            }));
            intents.push(UiIntent::ExportDiagnostics { path });
        }
        if let Some(status) = &state.export_status {
            ui.weak(status);
        }
    });
    let Some(events) = loaded(ui, &state.diagnostics, "events") else {
        return;
    };
    if events.is_empty() {
        ui.label("No diagnostic events recorded.");
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for event in events {
            ui.label(format!(
                "[{}] {} {} — {}",
                event.level,
                event.addon_id.as_deref().unwrap_or("-"),
                event.code,
                event.message
            ));
        }
    });
}

fn install_view(ui: &mut egui::Ui, state: &mut UiState, intents: &mut Vec<UiIntent>) {
    ui.heading("Install add-on");
    ui.horizontal(|ui| {
        ui.label("Package (.rvah):");
        let mut pkg = state.install_pkg.clone();
        if ui.text_edit_singleline(&mut pkg).changed() {
            intents.push(UiIntent::SetInstallPkg(pkg));
        }
    });
    ui.horizontal(|ui| {
        ui.label("Publisher key (.pub):");
        let mut key = state.install_pubkey.clone();
        if ui.text_edit_singleline(&mut key).changed() {
            intents.push(UiIntent::SetInstallPubkey(key));
        }
    });
    ui.horizontal(|ui| {
        if ui.button("Verify & preview").clicked() {
            intents.push(UiIntent::PreviewInstall);
        }
        if ui.button("Cancel").clicked() {
            intents.push(UiIntent::CancelInstall);
        }
    });
    match &state.install {
        Load::Idle => {}
        Load::Loading => {
            ui.spinner();
        }
        Load::Failed(error) => error_label(ui, error),
        Load::Loaded(preview) => {
            ui.separator();
            ui.strong(format!("{} v{}", preview.id, preview.version));
            ui.label("This add-on requests:");
            ui.label(format!("• network: {:?}", preview.network));
            ui.label(format!("• storage: {} bytes", preview.storage_bytes));
            ui.label(format!("• secrets: {:?}", preview.secrets));
            ui.horizontal(|ui| {
                if ui.button("✓ Approve & install").clicked() {
                    intents.push(UiIntent::ConfirmInstall);
                }
                if ui.button("✕ Deny").clicked() {
                    intents.push(UiIntent::CancelInstall);
                }
            });
        }
    }
}

fn auth_view(ui: &mut egui::Ui, state: &UiState, _intents: &mut Vec<UiIntent>) {
    ui.heading("Sign in");
    let Some(auth) = loaded(ui, &state.auth, "auth") else {
        return;
    };
    match auth.status.as_str() {
        "pending" => {
            ui.label("Visit the verification URL and enter the code:");
            if let Some(url) = &auth.verification_url {
                ui.hyperlink(url);
            }
            if let Some(code) = &auth.user_code {
                ui.heading(code);
            }
            ui.spinner();
        }
        "authorized" => {
            ui.colored_label(
                egui::Color32::GREEN,
                "Signed in — token stored in the vault.",
            );
        }
        other => {
            ui.colored_label(egui::Color32::LIGHT_RED, format!("Auth {other}."));
        }
    }
}
