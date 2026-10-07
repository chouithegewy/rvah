//! `rvah-app` — the RVAH desktop application (Phase 2 MVP UX).

mod bootstrap;
mod embedded_video;

use bootstrap::{bootstrap_embedded, bootstrap_without_video, default_data_dir};
use embedded_video::EmbeddedVideo;
use rvah_ui::RvahApp;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data_dir = args
        .iter()
        .position(|a| a == "--data-dir")
        .and_then(|i| args.get(i + 1))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(default_data_dir);

    let mut options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_title("RVAH"),
        ..Default::default()
    };
    // mpv's native `--wid` embedding works with an X11 child window. Force
    // eframe onto X11 when Xwayland/X11 is available, even inside a Wayland
    // desktop session.
    #[cfg(target_os = "linux")]
    if std::env::var_os("DISPLAY").is_some() {
        use winit::platform::x11::EventLoopBuilderExtX11;
        options.event_loop_builder = Some(Box::new(|builder| {
            builder.with_x11();
        }));
    }

    eframe::run_native(
        "rvah",
        options,
        Box::new(move |cc| {
            let video = EmbeddedVideo::create(cc);
            let booted = match &video {
                Ok(video) => bootstrap_embedded(&data_dir, video.window_id())?,
                Err(reason) => bootstrap_without_video(&data_dir, reason.clone())?,
            };
            let mut app = RvahApp::new(booted.handle);
            app.backend_note = booted.backend_note;
            if let Ok(video) = video {
                app.set_video_surface(Box::new(video));
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| e.to_string())
}
