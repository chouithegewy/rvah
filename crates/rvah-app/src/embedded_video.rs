//! Native video child surface used by mpv's `--wid` embedding mode.

#[cfg(target_os = "linux")]
mod platform {
    use std::ptr;

    use eframe::egui;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use rvah_ui::VideoSurface;
    use x11_dl::xlib;

    pub struct EmbeddedVideo {
        xlib: xlib::Xlib,
        display: *mut xlib::Display,
        window: xlib::Window,
        visible: bool,
        last_bounds: Option<(i32, i32, u32, u32)>,
    }

    impl EmbeddedVideo {
        pub fn create(cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
            let parent = match cc
                .window_handle()
                .map_err(|e| format!("app window handle unavailable: {e}"))?
                .as_raw()
            {
                RawWindowHandle::Xlib(handle) => handle.window,
                _ => return Err("embedded video currently requires the X11 window backend".into()),
            };

            let xlib = xlib::Xlib::open().map_err(|e| format!("could not load X11: {e}"))?;
            // A dedicated connection avoids sharing Xlib state with winit.
            let display = unsafe { (xlib.XOpenDisplay)(ptr::null()) };
            if display.is_null() {
                return Err("could not connect to the X11 display".into());
            }
            let screen = unsafe { (xlib.XDefaultScreen)(display) };
            let black = unsafe { (xlib.XBlackPixel)(display, screen) };
            let window =
                unsafe { (xlib.XCreateSimpleWindow)(display, parent, 0, 0, 1, 1, 0, black, black) };
            if window == 0 {
                unsafe {
                    (xlib.XCloseDisplay)(display);
                }
                return Err("could not create the embedded X11 video surface".into());
            }
            unsafe {
                (xlib.XFlush)(display);
            }
            Ok(Self {
                xlib,
                display,
                window,
                visible: false,
                last_bounds: None,
            })
        }

        pub fn window_id(&self) -> u64 {
            self.window as u64
        }
    }

    impl VideoSurface for EmbeddedVideo {
        fn place(&mut self, rect: egui::Rect, pixels_per_point: f32) {
            let scale = pixels_per_point.max(0.01);
            let bounds = (
                (rect.min.x * scale).round() as i32,
                (rect.min.y * scale).round() as i32,
                (rect.width() * scale).round().max(1.0) as u32,
                (rect.height() * scale).round().max(1.0) as u32,
            );
            unsafe {
                if self.last_bounds != Some(bounds) {
                    (self.xlib.XMoveResizeWindow)(
                        self.display,
                        self.window,
                        bounds.0,
                        bounds.1,
                        bounds.2,
                        bounds.3,
                    );
                    self.last_bounds = Some(bounds);
                }
                if !self.visible {
                    (self.xlib.XMapWindow)(self.display, self.window);
                    self.visible = true;
                }
                (self.xlib.XFlush)(self.display);
            }
        }

        fn hide(&mut self) {
            if self.visible {
                unsafe {
                    (self.xlib.XUnmapWindow)(self.display, self.window);
                    (self.xlib.XFlush)(self.display);
                }
                self.visible = false;
            }
        }
    }

    impl Drop for EmbeddedVideo {
        fn drop(&mut self) {
            unsafe {
                (self.xlib.XDestroyWindow)(self.display, self.window);
                (self.xlib.XCloseDisplay)(self.display);
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub use platform::EmbeddedVideo;

#[cfg(not(target_os = "linux"))]
pub struct EmbeddedVideo;

#[cfg(not(target_os = "linux"))]
impl EmbeddedVideo {
    pub fn create(_cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        Err("embedded video is not implemented for this platform".into())
    }

    pub fn window_id(&self) -> u64 {
        0
    }
}

#[cfg(not(target_os = "linux"))]
impl rvah_ui::VideoSurface for EmbeddedVideo {
    fn place(&mut self, _rect: eframe::egui::Rect, _pixels_per_point: f32) {}
    fn hide(&mut self) {}
}
