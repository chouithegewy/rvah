//! `rvah-app` — the RVAH desktop application (Phase 2 MVP UX).

mod bootstrap;

use bootstrap::{bootstrap, default_data_dir};
use rvah_ui::RvahApp;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data_dir = args
        .iter()
        .position(|a| a == "--data-dir")
        .and_then(|i| args.get(i + 1))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(default_data_dir);

    let booted = bootstrap(&data_dir)?;
    let mut app = RvahApp::new(booted.handle);
    app.backend_note = booted.backend_note;

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_title("RVAH"),
        ..Default::default()
    };
    eframe::run_native("rvah", options, Box::new(move |_cc| Ok(Box::new(app))))
        .map_err(|e| e.to_string())
}
